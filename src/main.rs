//! Ward — a voice companion for Elite Dangerous.
//!
//! This file is the window, and the window is a viewer. What Ward actually does
//! lives in [`engine`], on a task of its own, because Ward is worn rather than
//! watched: the window spends most of its life minimized behind a game, and a
//! minimized window does not paint. Everything here reads a picture the engine
//! published and sends back what the Commander asked for.

// A console window behind the app is noise for a released build, and in VR it
// is a window you cannot get to. Kept in debug builds, where panics are worth
// reading.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anthropic;
mod bindings;
mod capabilities;
mod capability;
mod captions;
mod checklist;
mod config;
mod diag;
mod engine;
mod honk;
mod journal;
mod listen;
mod mic;
mod outside;
mod overlay;
mod page;
mod press;
mod render;
// Test infrastructure rather than application code: its whole purpose is to
// stand in for the game. It compiles only for tests, so it cannot quietly
// become something the running application depends on.
#[cfg(test)]
mod replay;
mod schema;
mod secrets;
mod shown;
mod speech;
mod sse;
mod transcribe;
mod voice;
mod vr;

use anthropic::{Client, Role};
use std::sync::Arc;

use config::{Settings, State};
use eframe::egui;
use engine::Intent;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

fn main() -> eframe::Result<()> {
    // Answered before anything is loaded, because the thing asking is usually
    // the release gate checking that the binary it just built is the one it
    // meant to build. A version that needs a window open to read is a version
    // nothing can assert on.
    if std::env::args().any(|argument| argument == "--version") {
        // Written rather than printed, and a failure to write is ignored. A
        // released build is a windowed program with no console of its own, so
        // there may be nothing on the other end of stdout at all - and `println`
        // panics when it cannot write. Crashing while being asked what version
        // you are is a worse answer than saying nothing.
        use std::io::Write;
        let _ = writeln!(std::io::stdout(), "Ward {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let data_dir = config::data_dir();

    // Logging failing is not a reason for Ward not to run. Say so on the
    // console and carry on without it, rather than refusing to start over a
    // diagnostic.
    let _log = match diag::init(&data_dir) {
        Ok(guard) => Some(guard),
        Err(e) => {
            eprintln!("ward: could not start logging: {e:#}");
            None
        }
    };

    tracing::info!(
        target: "ward::app",
        version = env!("CARGO_PKG_VERSION"),
        data_dir = %data_dir.display(),
        "starting"
    );

    // A settings file Ward cannot make sense of stops it here, on purpose: a
    // rejected file means nothing was applied, and starting anyway would run
    // with defaults while the Commander believes their choices are in effect.
    let settings = match Settings::load(&Settings::path(&data_dir)) {
        Ok(settings) => {
            tracing::info!(target: "ward::config", changed = settings.changed(), "settings loaded");
            settings
        }
        Err(e) => {
            tracing::error!(target: "ward::config", error = %e, "settings rejected");
            eprintln!("ward: {e:#}");
            return Ok(());
        }
    };

    // What is actually wired, named in the log. The question "did I remember to
    // register it" should be answerable by reading a file rather than by
    // reasoning about the composition point.
    let registry = capabilities::registry(&settings, &data_dir);
    for capability in registry.capabilities() {
        tracing::info!(
            target: "ward::capability",
            id = capability.id(),
            group = capability.group(),
            tools = capability.tools().len(),
            summary = capability.one_liner(),
            "registered"
        );
    }
    tracing::info!(
        target: "ward::capability",
        capabilities = registry.capabilities().len(),
        tools = registry.tools().len(),
        "registry composed"
    );

    let state = State::load(&State::path(&data_dir));
    let size = [
        state.f32("window width").unwrap_or(980.0),
        state.f32("window height").unwrap_or(720.0),
    ];

    // Where it was last time, if it is anywhere believable.
    //
    // A remembered position can outlive the screen it was on - a monitor
    // unplugged, a laptop away from its dock - and a window restored onto a
    // screen that is no longer there is one nobody can reach. The bound below
    // catches nonsense rather than that case, because a second monitor to the
    // left is a legitimate negative coordinate and refusing it would break a
    // setup that works. If a window ever does come back somewhere unreachable,
    // deleting data/state.json puts it back in the middle of the screen, which
    // is the whole reason running state is a file you can throw away.
    let believable = |value: f32| value.abs() < 32_000.0;

    let placement = state
        .f32("window x")
        .zip(state.f32("window y"))
        .filter(|(x, y)| believable(*x) && believable(*y));

    let mut viewport = egui::ViewportBuilder::default().with_inner_size(size);

    if let Some((x, y)) = placement {
        viewport = viewport.with_position([x, y]);
    }

    let options = eframe::NativeOptions {
        viewport: viewport
            // Two surfaces side by side need room for both. Below this the
            // checklist takes so much of the width that the conversation is a
            // strip, which is a window that is technically usable and actually
            // not.
            .with_min_inner_size([760.0, 480.0])
            .with_title("Ward"),
        ..Default::default()
    };

    eframe::run_native(
        "ward",
        options,
        Box::new(|cc| {
            // The toolkit's defaults are sized for reading at a desk with full
            // attention. Ward is read at a glance, mid-flight, and later at
            // distance through a headset, so it starts larger.
            //
            // This multiplies the display's own scale factor rather than
            // replacing it: a Commander who has already told Windows to render
            // at 150% still gets 150%, and this on top. Making it adjustable is
            // a separate piece of work.
            cc.egui_ctx
                .set_zoom_factor(settings.f32("text size", 0.5, 4.0));
            Ok(Box::new(Ward::new(settings, state, registry, &cc.egui_ctx))
                as Box<dyn eframe::App>)
        }),
    )
}

/// Converts a window size from what the toolkit reports to what the window
/// system is asked for.
///
/// These are two different units and they look identical, which is what makes
/// this worth a function and a name. The toolkit lays out in points, and a
/// point is smaller than a pixel by both the display's own scaling **and**
/// Ward's text size. The window system knows only about the display's scaling.
///
/// Remembering the toolkit's number and handing it back to the window system
/// shrinks the window by the text size on every close. It is invisible in one
/// run and unmistakable after four: at a text size of 1.35 the window is down
/// to a quarter of its area, and by then it is sitting on its own minimum with
/// the checklist filling it. That is not a window somebody chose; it is a
/// window that decayed.
fn as_the_window_system_counts_it(points: egui::Vec2, zoom: f32) -> egui::Vec2 {
    points * zoom
}

/// Checks the things that fail quietly.
///
/// Every one of these stands for an evening somebody could otherwise lose. A
/// microphone that was never chosen, a key that was refused, a journal folder
/// pointing at nothing — none of them announce themselves. They present as Ward
/// simply not working, in a cockpit, with no way to tell which of five things
/// is wrong.
///
/// Each check does the real thing rather than inspecting a setting. Opening the
/// device is the only way to learn that it opens.
async fn check_setup(settings: &Settings) -> Vec<page::Check> {
    let mut checks = Vec::new();

    checks.push(match crate::mic::Microphone::open() {
        Ok(_) => page::Check {
            what: "Microphone",
            ok: true,
            detail: "Windows handed Ward a recording device and it started.".to_string(),
        },
        Err(e) => page::Check {
            what: "Microphone",
            ok: false,
            detail: format!("{e:#}"),
        },
    });

    checks.push(match crate::voice::speakers() {
        Ok(name) => page::Check {
            what: "Speakers",
            ok: true,
            detail: format!("Ward will speak through {name}."),
        },
        Err(e) => page::Check {
            what: "Speakers",
            ok: false,
            detail: format!("{e:#}"),
        },
    });

    let model = settings.file("speech model", &config::data_dir());
    checks.push(page::Check {
        what: "Speech model",
        ok: model.is_file(),
        detail: match model.is_file() {
            true => format!("Found at {}.", model.display()),
            false => format!(
                "Nothing at {}. Ward cannot hear you until a model file is there.",
                model.display()
            ),
        },
    });

    let folder = std::path::PathBuf::from(settings.string("journal folder"));
    checks.push(match crate::journal::newest(&folder) {
        Some(file) => page::Check {
            what: "Elite's journal",
            ok: true,
            detail: format!(
                "Reading {}.",
                file.file_name().unwrap_or_default().to_string_lossy()
            ),
        },
        None => page::Check {
            what: "Elite's journal",
            ok: false,
            detail: format!(
                "No journal in {}. Ward will not know where you are. This is \
                 expected if the game has never run on this machine.",
                folder.display()
            ),
        },
    });

    // Last, because it is the only one that touches the network, and a
    // Commander reading down the list should have the local answers already.
    let stored = secrets::load(&secrets::key_path(&config::data_dir()));

    checks.push(match stored {
        Ok(Some(key)) => match anthropic::models(&key).await {
            Ok(models) => page::Check {
                what: "API key",
                ok: true,
                detail: format!("Accepted. {} models available to it.", models.len()),
            },
            Err(why) => page::Check {
                what: "API key",
                ok: false,
                detail: why,
            },
        },
        Ok(None) => page::Check {
            what: "API key",
            ok: false,
            detail: "No key stored. Ward can hear you and cannot answer.".to_string(),
        },
        Err(e) => page::Check {
            what: "API key",
            ok: false,
            detail: format!("{e:#}"),
        },
    });

    checks
}

enum Screen {
    /// No key stored yet, or the stored one could not be read.
    NeedKey {
        entry: String,
        problem: Option<String>,
    },
    Talking,
}

struct Ward {
    settings: Settings,
    state: State,
    /// Kept alive here because it is what the engine runs on. Dropping it is how
    /// everything Ward is doing stops, which is what closing the window should
    /// mean and nothing sooner.
    runtime: tokio::runtime::Runtime,
    screen: Screen,
    /// The engine's end of the conversation. Everything Ward knows is read
    /// through this, and everything the Commander does is sent through it.
    engine: engine::Handle,
    /// Whether a key has been stored, which is all the settings page needs to
    /// know about it. The key itself never comes back out.
    key_stored: bool,
    /// Where the window is, in the units the window system uses.
    placement: Option<egui::Pos2>,
    /// A new checklist item being typed. Held here rather than in the panel so
    /// a half-typed line survives the frame it was typed on.
    adding: String,
    /// What is being typed to Ward. The engine holds the conversation; this is
    /// only the box it is typed into.
    prompt: String,
    /// The settings page, and whether it is what the window is showing.
    page: page::Page,
    settings_open: bool,
    /// Answers to things the settings page asked for, which all take long
    /// enough that asking on the drawing thread would stop the window. These
    /// stay here rather than going to the engine because they serve the
    /// settings page and nothing else, and the settings page only exists while
    /// somebody is looking at it.
    errands: UnboundedReceiver<Errand>,
    errand: UnboundedSender<Errand>,
    window: egui::Vec2,
}

/// Something the settings page asked for, come back.
enum Errand {
    Voices(Result<Vec<voice::Voice>, String>),
    Models(Result<Vec<String>, String>),
    Checks(Vec<page::Check>),
}

impl Ward {
    fn new(
        settings: Settings,
        state: State,
        registry: capability::Registry,
        ctx: &egui::Context,
    ) -> Self {
        let registry = Arc::new(registry);
        let model = settings.string("model");

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("could not start the async runtime");

        let path = secrets::key_path(&config::data_dir());

        let (screen, client) = match secrets::load(&path) {
            Ok(Some(key)) => {
                tracing::info!(target: "ward::secrets", "stored key loaded");
                (
                    Screen::Talking,
                    Some(Client::new(key, model.clone(), registry.clone())),
                )
            }
            Ok(None) => {
                tracing::info!(target: "ward::secrets", "no key stored yet");
                (
                    Screen::NeedKey {
                        entry: String::new(),
                        problem: None,
                    },
                    None,
                )
            }
            // A stored key that will not decrypt is the ordinary consequence of
            // copying the data folder between machines or accounts. Say so and
            // offer the field again rather than refusing to start.
            Err(e) => {
                tracing::warn!(target: "ward::secrets", error = %e, "stored key unreadable");
                (
                    Screen::NeedKey {
                        entry: String::new(),
                        problem: Some(e.to_string()),
                    },
                    None,
                )
            }
        };

        let key_stored = client.is_some();

        // Started before anything else needs it, because the slow parts happen
        // once and on that thread: half a gigabyte of speech model to load and
        // a capture device to open. By the time a key can be held, there is
        // nothing left to set up on the path between pressing it and recording.
        let hush = listen::Hush::default();
        let heard = Some(listen::spawn(
            settings.string("push to talk key"),
            settings.file("speech model", &config::data_dir()),
            std::path::PathBuf::from(settings.string("bindings folder")),
            hush.clone(),
            // Nothing waits on this. It asks the window to draw what the engine
            // has already been told, and the engine carries on whether or not
            // there is a window to ask.
            {
                let ctx = ctx.clone();
                move || ctx.request_repaint()
            },
        ));

        let (errand_tx, errand_rx) = unbounded_channel();

        // Started whether or not there is a headset. It keeps asking, because
        // SteamVR is usually not running yet when Ward is.
        let captions = std::sync::Arc::new(std::sync::Mutex::new(captions::Captions::default()));
        overlay::spawn(captions.clone());

        let wake: engine::Wake = {
            let ctx = ctx.clone();
            Arc::new(move || ctx.request_repaint())
        };

        let engine = {
            // The engine spawns onto this runtime, so it has to be started from
            // inside it. The guard is dropped before the runtime is moved into
            // the struct below.
            let _inside = runtime.enter();

            engine::Engine::start(
                settings.clone(),
                registry,
                client,
                heard,
                captions,
                hush,
                wake,
            )
        };

        Self {
            settings,
            state,
            runtime,
            screen,
            engine,
            key_stored,
            adding: String::new(),
            prompt: String::new(),
            placement: None,
            page: page::Page::default(),
            settings_open: false,
            errands: errand_rx,
            errand: errand_tx,
            window: egui::Vec2::new(980.0, 720.0),
        }
    }

    /// Hands what the settings page asked for to whoever owns it.
    ///
    /// **Everything here takes effect now.** A setting that needs a restart is
    /// a setting somebody changes, hears no difference from, and changes back —
    /// and in a headset a restart means taking it off.
    fn apply(&mut self, wants: page::Wants, ctx: &egui::Context) {
        if let Some(key) = wants.store_key {
            match secrets::store(&secrets::key_path(&config::data_dir()), &key) {
                Ok(()) => {
                    tracing::info!(target: "ward::secrets", "key stored");
                    self.engine.tell(Intent::Key(key));
                    self.key_stored = true;
                    self.screen = Screen::Talking;
                    self.page.key_stored();
                    // A new key means a different list of models, and the old
                    // one was answered for a key that is no longer in use.
                    self.page.models = page::Options::Unasked;
                }
                Err(e) => {
                    tracing::error!(target: "ward::secrets", error = %e, "could not store key");
                    self.page.key_problem = Some(e.to_string());
                }
            }
        }

        if wants.changed {
            // Applied here because it is the window's own scaling, and sent on
            // because everything else built from settings lives in the engine.
            ctx.set_zoom_factor(self.settings.f32("text size", 0.5, 4.0));
            self.engine
                .tell(Intent::Settings(Box::new(self.settings.clone())));
        }

        if let Some(text) = wants.preview {
            self.engine.tell(Intent::Preview(text));
        }

        if wants.fetch_voices {
            self.page.voices = page::Options::Asking;
            let errand = self.errand.clone();
            let ctx = ctx.clone();

            self.runtime.spawn(async move {
                let got = voice::voices().await.map_err(|e| e.to_string());
                let _ = errand.send(Errand::Voices(got));
                ctx.request_repaint();
            });
        }

        if wants.fetch_models {
            self.page.models = page::Options::Asking;
            let errand = self.errand.clone();
            let ctx = ctx.clone();
            let key = secrets::load(&secrets::key_path(&config::data_dir()))
                .ok()
                .flatten();

            self.runtime.spawn(async move {
                let got = match key {
                    Some(key) => anthropic::models(&key).await,
                    None => Err("there is no key stored yet".to_string()),
                };
                let _ = errand.send(Errand::Models(got));
                ctx.request_repaint();
            });
        }

        if wants.run_setup_test {
            self.page.testing = true;
            self.page.checks.clear();

            let errand = self.errand.clone();
            let ctx = ctx.clone();
            let settings = self.settings.clone();

            self.runtime.spawn(async move {
                let checks = check_setup(&settings).await;
                let _ = errand.send(Errand::Checks(checks));
                ctx.request_repaint();
            });
        }
    }

    /// Takes answers to what the settings page asked for.
    fn pump_errands(&mut self) {
        while let Ok(errand) = self.errands.try_recv() {
            match errand {
                Errand::Voices(Ok(voices)) => {
                    tracing::info!(target: "ward::voice", voices = voices.len(), "voice list");
                    self.page.voices = page::Options::Have(voices);
                }
                Errand::Voices(Err(why)) => {
                    tracing::warn!(target: "ward::voice", error = %why, "no voice list");
                    self.page.voices = page::Options::Refused(why);
                }
                Errand::Models(Ok(models)) => {
                    self.page.models = page::Options::Have(models);
                }
                Errand::Models(Err(why)) => {
                    tracing::warn!(target: "ward::model", error = %why, "no model list");
                    self.page.models = page::Options::Refused(why);
                }
                Errand::Checks(checks) => {
                    self.page.testing = false;
                    self.page.checks = checks;
                }
            }
        }
    }

    fn key_screen(&mut self, ui: &mut egui::Ui) {
        // Pulled out so the borrow of `self.screen` ends before anything else
        // on `self` is touched.
        let (mut entry, mut problem) = match &self.screen {
            Screen::NeedKey { entry, problem } => (entry.clone(), problem.clone()),
            Screen::Talking => return,
        };

        ui.add_space(24.0);
        ui.heading("Ward needs an API key");
        ui.add_space(8.0);
        ui.label(
            "Stored encrypted, readable only by this Windows account on this machine. \
             It is never written anywhere else and never read from the environment.",
        );
        ui.add_space(16.0);

        if let Some(problem) = problem.as_ref() {
            ui.colored_label(ui.visuals().error_fg_color, problem);
            ui.add_space(8.0);
        }

        // Masked, because a key on screen is a key in a screenshot or a stream.
        let field = ui.add(
            egui::TextEdit::singleline(&mut entry)
                .password(true)
                .hint_text("sk-ant-…")
                .desired_width(f32::INFINITY),
        );

        let submitted = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        ui.add_space(8.0);
        let saved = ui.button("Save").clicked() || submitted;

        if saved && !entry.trim().is_empty() {
            let key = entry.trim().to_string();

            match secrets::store(&secrets::key_path(&config::data_dir()), &key) {
                Ok(()) => {
                    tracing::info!(target: "ward::secrets", "key stored");
                    self.engine.tell(Intent::Key(key));
                    self.key_stored = true;
                    self.screen = Screen::Talking;
                    return;
                }
                Err(e) => {
                    tracing::error!(target: "ward::secrets", error = %e, "could not store key");
                    problem = Some(e.to_string());
                }
            }
        }

        self.screen = Screen::NeedKey { entry, problem };
    }

    fn compose(&mut self, ui: &mut egui::Ui, view: &engine::View) {
        let busy = view.streaming;

        ui.horizontal(|ui| {
            let send = ui.add_enabled(!busy, egui::Button::new("Send"));

            let hint = match (busy, view.listening) {
                // Listening wins: it is the state the Commander is currently
                // holding a key to be in, and it is the one they are waiting
                // for confirmation of.
                (_, true) => "Listening…",
                (true, _) => "Ward is answering…",
                _ => "Ask Ward something, or hold your push-to-talk key",
            };

            let field = ui.add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut self.prompt)
                    .hint_text(hint)
                    .desired_width(f32::INFINITY),
            );

            let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            if (send.clicked() || entered) && !self.prompt.trim().is_empty() {
                self.engine
                    .tell(Intent::Ask(std::mem::take(&mut self.prompt)));
                // Keep focus in the box so a second question does not need the
                // mouse.
                field.request_focus();
            }
        });
    }

    /// Draws whatever the capabilities currently had to show when the engine
    /// last published.
    ///
    /// Every edit here goes through the same tool the model calls, with the
    /// same words. The panel cannot make a change the model could not, and it
    /// cannot make one in a way the checklist's own rules would refuse.
    fn panel(&mut self, ui: &mut egui::Ui, view: &engine::View) {
        // What the Commander asked for, decided while drawing and sent after.
        // Running a tool mid-draw would mean the rest of the frame renders a
        // list that has already changed underneath it.
        let mut edit: Option<(&'static str, String)> = None;

        for display in &view.panels {
            let title = display.title().unwrap_or_default().to_string();

            let shown::Shown::List { rows, .. } = display else {
                // Only lists have a panel today. The rest of the taxonomy grows
                // with the capability that needs it, rather than ahead of it.
                continue;
            };

            ui.add_space(8.0);
            ui.heading(&title);
            ui.add_space(4.0);

            if rows.is_empty() {
                ui.weak("Nothing on it yet.");
            }

            for row in rows {
                // Top aligned, because a wrapped item is taller than the tick
                // box beside it and a centered box floats in the middle of a
                // two-line item.
                ui.horizontal_top(|ui| {
                    let mut done = row.done;

                    // Only ever forward. Unticking is a different question from
                    // the one this issue answers, and putting a control on
                    // screen with no spoken equivalent is how the two surfaces
                    // start disagreeing about what can be done.
                    if ui
                        .add_enabled(!done, egui::Checkbox::new(&mut done, ""))
                        .on_hover_text("Mark as done")
                        .clicked()
                    {
                        edit = Some(("checklist_complete", row.text.clone()));
                    }

                    let label = match row.done {
                        true => egui::RichText::new(&row.text).weak().strikethrough(),
                        false => egui::RichText::new(&row.text),
                    };

                    // The button is placed first and the text fills what is
                    // left, wrapping onto as many lines as it needs.
                    //
                    // Laid out the other way round, a long item made the row
                    // wider than the panel instead of taller: the text ran off
                    // the edge, the button went with it, and the heading was
                    // pushed out of view on the other side. Commanders write
                    // items like "go back to Tod McQuin and engineer the
                    // remaining multicannons", so this is the normal case
                    // rather than the extreme one.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        if ui
                            .small_button("Remove")
                            .on_hover_text("Take this off the list entirely")
                            .clicked()
                        {
                            edit = Some(("checklist_remove", row.text.clone()));
                        }

                        // Turned back around before the text goes in. The outer
                        // layout exists to park the button on the right; laying
                        // the words out in it too would set them against the
                        // right edge, which for a wrapped item reads as a
                        // ragged left margin that moves line to line.
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
                            ui.add(egui::Label::new(label).wrap());
                        });
                    });
                });
            }

            ui.add_space(8.0);

            let field =
                ui.add(egui::TextEdit::singleline(&mut self.adding).hint_text("Add something"));

            let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            if entered && !self.adding.trim().is_empty() {
                edit = Some(("checklist_add", self.adding.trim().to_string()));
                self.adding.clear();
                field.request_focus();
            }
        }

        if let Some((tool, item)) = edit {
            self.engine.tell(Intent::Tool(tool, item));
        }
    }

    fn transcript(&mut self, ui: &mut egui::Ui, view: &engine::View) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if view.history.is_empty() && view.pending.is_none() {
                    ui.add_space(24.0);
                    ui.label("Ask Ward something to begin.");
                }

                for message in &view.history {
                    let who = match message.role {
                        Role::User => "Commander",
                        Role::Assistant => "Ward",
                    };

                    ui.add_space(10.0);
                    ui.label(egui::RichText::new(who).strong());
                    ui.label(&message.text);
                }

                if let Some(pending) = view.pending.as_ref() {
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("Ward").strong());

                    if let Some(tool) = view.using.as_ref() {
                        ui.weak(format!("using {tool}…"));
                    }

                    if !pending.is_empty() {
                        ui.label(pending);
                    } else if view.using.is_none() {
                        ui.label("…");
                    }
                }

                if let Some(problem) = view.problem.as_ref() {
                    ui.add_space(10.0);
                    ui.colored_label(ui.visuals().error_fg_color, problem);
                }

                // No caption here. There used to be one, as proof that the
                // speech stream reached a surface at all, and now that it
                // reaches the headset it has a real home. On the desktop it was
                // printing every reply a second time in a fainter color - the
                // conversation is already on this window, and a caption of what
                // is on the screen you are looking at is not a caption.
            });
    }
}

impl eframe::App for Ward {
    /// Remembers the window on the way out.
    ///
    /// This is running state, not a setting: if it cannot be written, Ward says
    /// so and closes anyway. Refusing to exit over a forgotten window size
    /// would be worse than forgetting it.
    fn on_exit(&mut self) {
        // Before anything else, and unconditionally. A key left down when Ward
        // closes is a throttle in Elite that will not stop, and the Commander
        // cannot fix it by pressing the key themselves - the game already
        // believes it is held.
        let holding = press::held();
        press::release_all();

        if holding > 0 {
            tracing::warn!(target: "ward::act", keys = holding, "released keys on the way out");
        }

        self.state
            .set("window width", serde_json::json!(self.window.x));
        self.state
            .set("window height", serde_json::json!(self.window.y));

        if let Some(corner) = self.placement {
            self.state.set("window x", serde_json::json!(corner.x));
            self.state.set("window y", serde_json::json!(corner.y));
        }

        if let Err(e) = self.state.save(&State::path(&config::data_dir())) {
            tracing::warn!(target: "ward::config", error = %e, "could not remember the window");
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Recorded every frame so the size at the moment of closing is the one
        // remembered, without having to ask the window system after the window
        // has gone. The viewport rect is the window itself, not the area left
        // inside it after decoration.
        //
        // Converted on the way out, because the two ends of this count in
        // different units. See [`as_the_window_system_counts_it`].
        let zoom = ui.ctx().zoom_factor();

        if let Some(rect) = ui.ctx().input(|i| i.viewport().inner_rect) {
            self.window = as_the_window_system_counts_it(rect.size(), zoom);
        }

        // The outer rect, because that is what the window system is handed back:
        // it includes the title bar, and remembering where the drawing started
        // instead would walk the window up the screen by the height of its own
        // decoration on every launch.
        if let Some(rect) = ui.ctx().input(|i| i.viewport().outer_rect) {
            let corner = as_the_window_system_counts_it(rect.min.to_vec2(), zoom);
            self.placement = Some(corner.to_pos2());
        }

        // One picture for the whole frame. Read again halfway down and the top
        // of the window could be drawn from one turn and the bottom from the
        // next.
        let view = self.engine.view();

        self.pump_errands();

        let need_key = matches!(self.screen, Screen::NeedKey { .. });

        // A bar rather than a button tucked somewhere: settings is the second
        // half of what this window is for, and a Commander whose microphone is
        // wrong needs to reach it without hunting.
        if !need_key {
            egui::Panel::top("where").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(!self.settings_open, "Conversation")
                        .clicked()
                    {
                        self.settings_open = false;
                    }

                    if ui
                        .selectable_label(self.settings_open, "Settings")
                        .clicked()
                    {
                        // Read from the settings again on the way in, so a box
                        // left half-typed last time does not come back.
                        self.page.reread();
                        self.settings_open = true;
                    }

                    if self.settings.changed() > 0 {
                        ui.weak(format!("{} changed", self.settings.changed()));
                    }
                });
                ui.add_space(4.0);
            });
        }

        if self.settings_open && !need_key {
            egui::CentralPanel::default().show(ui, |ui| {
                let stored = self.key_stored;
                let mut settings = self.settings.clone();
                let wants = self.page.show(ui, &mut settings, stored);
                self.settings = settings;
                self.apply(wants, ui.ctx());
            });
            return;
        }

        // Beside the conversation rather than inside it. What Ward is holding
        // for the Commander is standing state, and standing state scrolling
        // away with the transcript is the thing that makes a panel useless.
        if !need_key {
            egui::Panel::right("standing")
                .resizable(true)
                .default_size(300.0)
                // Bounded, because the panel sizes itself around what is in it
                // and a long checklist item would otherwise push it as wide as
                // the sentence, leaving the conversation a strip down the side.
                .size_range(220.0..=460.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.panel(ui, &view));
                });
        }

        egui::CentralPanel::default().show(ui, |ui| {
            if need_key {
                self.key_screen(ui);
                return;
            }

            // Compose is laid out first so it pins to the bottom; the
            // transcript then takes whatever height is left.
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                self.compose(ui, &view);
                ui.separator();

                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    self.transcript(ui, &view);
                });
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remembered_window_is_the_same_size_when_it_reopens() {
        // The toolkit reports points and the window system is handed pixels,
        // and Ward's text size sits between them. Skip the conversion and the
        // window shrinks by that factor every time it is closed.
        let asked_for = egui::Vec2::new(1200.0, 800.0);
        let zoom = 1.35;

        // What the toolkit reports for a window of that size.
        let reported = asked_for / zoom;

        let remembered = as_the_window_system_counts_it(reported, zoom);

        assert!(
            (remembered.x - asked_for.x).abs() < 0.01,
            "reopened at {} after asking for {}",
            remembered.x,
            asked_for.x
        );
        assert!((remembered.y - asked_for.y).abs() < 0.01);
    }

    #[test]
    fn four_closes_do_not_shrink_the_window() {
        // The way this was noticed: not on the first run, when it is invisible,
        // but after a few, when the window is on its own minimum and every
        // surface in it is squeezed.
        let zoom = 1.35;
        let mut size = egui::Vec2::new(1200.0, 800.0);

        for _ in 0..4 {
            size = as_the_window_system_counts_it(size / zoom, zoom);
        }

        assert!(
            (size.x - 1200.0).abs() < 0.01,
            "four closes took it to {}",
            size.x
        );
    }
}
