//! Ward — a voice companion for Elite Dangerous.
//!
//! This is the typed turn: ask a question, read the answer. Speech arrives
//! next, on both ends. Typing is not scaffolding on the way to voice — it stays
//! for good, because Ward has to be reachable without a microphone or a
//! speaker.

// A console window behind the app is noise for a released build, and in VR it
// is a window you cannot get to. Kept in debug builds, where panics are worth
// reading.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anthropic;
mod capabilities;
mod capability;
mod config;
mod diag;
mod secrets;
mod speech;
mod sse;
mod voice;

use anthropic::{Client, Event, Message, Role};
use std::sync::Arc;

use config::{Settings, State};
use eframe::egui;
use serde_json::json;
use speech::{Act, Arbiter, Body, Register, Speaker};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

fn main() -> eframe::Result<()> {
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
    let registry = capabilities::registry();
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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_min_inner_size([520.0, 400.0])
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
            Ok(Box::new(Ward::new(settings, state, registry)) as Box<dyn eframe::App>)
        }),
    )
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
    registry: Arc<capability::Registry>,
    state: State,
    runtime: tokio::runtime::Runtime,
    screen: Screen,
    client: Option<Client>,

    history: Vec<Message>,
    prompt: String,

    /// The reply as it arrives. Held apart from `history` so a turn that fails
    /// halfway leaves no half-message behind pretending to be an answer.
    pending: Option<String>,
    events: Option<UnboundedReceiver<Event>>,
    problem: Option<String>,
    /// A capability currently running, so a pause has a reason on screen.
    using: Option<String>,
    /// Everything Ward emits goes through here. Nothing speaks yet; captions
    /// and the transcript already render off it, so when a voice arrives it
    /// attaches to the same stream rather than beside it.
    speech: Arbiter,
    /// When the current act started holding the caption.
    spoke_at: std::time::Duration,
    /// Signals that speaking finished, so the stream can move on. Silence
    /// falls back to the caption's own timing, which is what keeps a failure
    /// to speak from also freezing what is on screen.
    spoken: Option<UnboundedReceiver<()>>,
    window: egui::Vec2,
}

impl Ward {
    fn new(settings: Settings, state: State, registry: capability::Registry) -> Self {
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

        Self {
            settings,
            registry,
            state,
            runtime,
            screen,
            client,
            history: Vec::new(),
            prompt: String::new(),
            pending: None,
            events: None,
            problem: None,
            using: None,
            speech: Arbiter::default(),
            spoke_at: std::time::Duration::ZERO,
            spoken: None,
            window: egui::Vec2::new(980.0, 720.0),
        }
    }

    fn streaming(&self) -> bool {
        self.events.is_some()
    }

    fn send(&mut self, ctx: &egui::Context) {
        let text = self.prompt.trim().to_string();

        if text.is_empty() || self.streaming() {
            return;
        }

        let Some(client) = self.client.clone() else {
            return;
        };

        self.prompt.clear();
        self.problem = None;
        self.history.push(Message {
            role: Role::User,
            text,
        });
        self.pending = Some(String::new());

        let (tx, rx): (UnboundedSender<Event>, UnboundedReceiver<Event>) = unbounded_channel();
        self.events = Some(rx);

        tracing::info!(target: "ward::turn", messages = self.history.len(), "turn started");

        let history = self.history.clone();
        let ctx = ctx.clone();

        self.runtime.spawn(async move {
            client.stream(&history, tx).await;
            // The window is not otherwise waiting on anything, so nudge it or
            // the last fragment sits in the channel until the mouse moves.
            ctx.request_repaint();
        });
    }

    /// Speaks an act, if it has words and a voice can be had.
    ///
    /// Failure here costs the voice and nothing else. The answer is already on
    /// screen, the caption still advances on its own timing, and the reason is
    /// written down. A companion that goes quiet is worse than one that never
    /// spoke, but a companion that stops working because it could not speak is
    /// worse than both.
    fn say(&mut self, act: &Act, ctx: &egui::Context) {
        let Body::Text(text) = &act.body else {
            return;
        };

        let text = text.clone();
        let voice = self.settings.string("voice");
        let rate = self.settings.string("speaking rate");
        let ctx = ctx.clone();

        let (tx, rx) = unbounded_channel();
        self.spoken = Some(rx);

        self.runtime.spawn(async move {
            let started = std::time::Instant::now();

            match voice::synthesize(&text, &voice, &rate).await {
                Ok(speech) => {
                    tracing::info!(
                        target: "ward::voice",
                        ms = started.elapsed().as_millis() as u64,
                        bytes = speech.mp3.len(),
                        "synthesized"
                    );

                    // Playback blocks for as long as the words take, so it goes
                    // somewhere that is allowed to block.
                    let played = tokio::task::spawn_blocking(move || voice::play(speech.mp3)).await;

                    if let Ok(Err(e)) = played {
                        tracing::warn!(target: "ward::voice", error = %e, "could not play");
                    }
                }
                Err(e) => {
                    tracing::warn!(target: "ward::voice", error = %e, "could not speak");
                }
            }

            let _ = tx.send(());
            ctx.request_repaint();
        });
    }

    /// Drains whatever the turn has produced since the last frame.
    fn pump(&mut self) {
        let Some(rx) = self.events.as_mut() else {
            return;
        };

        let mut finished = false;

        while let Ok(event) = rx.try_recv() {
            match event {
                Event::Text(chunk) => {
                    self.using = None;
                    if let Some(pending) = self.pending.as_mut() {
                        pending.push_str(&chunk);
                    }
                }
                Event::Using(tool) => {
                    self.using = Some(tool);
                }
                Event::Done { stop_reason } => {
                    let text = self.pending.take().unwrap_or_default();

                    // A refusal arrives as an ordinary success with little or
                    // no content. Left alone it would render as Ward simply
                    // saying nothing, which reads as a bug.
                    if stop_reason.as_deref() == Some("refusal") && text.trim().is_empty() {
                        self.problem = Some("Ward declined to answer that.".to_string());
                    } else if text.trim().is_empty() {
                        self.problem = Some("The reply came back empty.".to_string());
                    } else {
                        tracing::info!(
                            target: "ward::turn",
                            chars = text.len(),
                            stop_reason = stop_reason.as_deref().unwrap_or("none"),
                            "turn finished"
                        );

                        let now = diag::since_start();
                        self.speech.offer(
                            Act::text(Speaker::Ward, Register::Reply, text.clone(), now),
                            now,
                        );

                        self.history.push(Message {
                            role: Role::Assistant,
                            text,
                        });
                    }

                    self.using = None;
                    finished = true;
                }
                Event::Failed(message) => {
                    tracing::warn!(target: "ward::turn", error = %message, "turn failed");

                    let now = diag::since_start();
                    self.speech.offer(
                        Act::text(Speaker::System, Register::Alert, message.clone(), now)
                            .about("turn failure"),
                        now,
                    );

                    self.problem = Some(message);
                    // Drop the partial reply. Keeping it would leave a fragment
                    // on screen that looks like an answer and is not one.
                    self.pending = None;
                    self.using = None;
                    // Take the question back out of the history too, so a retry
                    // does not send it twice.
                    self.history.pop();
                    finished = true;
                }
            }
        }

        if finished {
            self.events = None;
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
                    self.client = Some(Client::new(
                        key,
                        self.settings.string("model"),
                        self.registry.clone(),
                    ));
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

    fn compose(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let busy = self.streaming();

        ui.horizontal(|ui| {
            let send = ui.add_enabled(!busy, egui::Button::new("Send"));

            let field = ui.add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut self.prompt)
                    .hint_text(if busy {
                        "Ward is answering…"
                    } else {
                        "Ask Ward something"
                    })
                    .desired_width(f32::INFINITY),
            );

            let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            if send.clicked() || entered {
                self.send(&ctx);
                // Keep focus in the box so a second question does not need the
                // mouse.
                field.request_focus();
            }
        });
    }

    fn transcript(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if self.history.is_empty() && self.pending.is_none() {
                    ui.add_space(24.0);
                    ui.label("Ask Ward something to begin.");
                }

                for message in &self.history {
                    let who = match message.role {
                        Role::User => "Commander",
                        Role::Assistant => "Ward",
                    };

                    ui.add_space(10.0);
                    ui.label(egui::RichText::new(who).strong());
                    ui.label(&message.text);
                }

                if let Some(pending) = self.pending.as_ref() {
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("Ward").strong());

                    if let Some(tool) = self.using.as_ref() {
                        ui.weak(format!("using {tool}…"));
                    }

                    if !pending.is_empty() {
                        ui.label(pending);
                    } else if self.using.is_none() {
                        ui.label("…");
                    }
                }

                if let Some(problem) = self.problem.as_ref() {
                    ui.add_space(10.0);
                    ui.colored_label(ui.visuals().error_fg_color, problem);
                }

                // The caption line comes off the speech stream rather than off
                // the model's output, which is what will later keep a caption
                // on screen for exactly as long as something is being said.
                if let Some(caption) = self.speech.speaking().and_then(Act::caption) {
                    ui.add_space(10.0);
                    ui.weak(caption);
                }
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
        self.state.set("window width", json!(self.window.x));
        self.state.set("window height", json!(self.window.y));

        if let Err(e) = self.state.save(&State::path(&config::data_dir())) {
            tracing::warn!(target: "ward::config", error = %e, "could not remember the window");
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Recorded every frame so the size at the moment of closing is the one
        // remembered, without having to ask the window system after the window
        // has gone. The viewport rect is the window itself, not the area left
        // inside it after decoration.
        if let Some(rect) = ui.ctx().input(|i| i.viewport().inner_rect) {
            self.window = rect.size();
        }

        self.pump();

        // Nothing plays audio yet, so an act holds for as long as it would take
        // to read, then makes way for the next. When a voice arrives it takes
        // over this pacing - the act ends when the words do - and nothing above
        // here changes.
        let now = diag::since_start();

        // Speaking finished, so the stream may move on.
        if self.spoken.as_mut().is_some_and(|rx| rx.try_recv().is_ok()) {
            self.spoken = None;
            self.speech.finished();
        }

        match self.speech.speaking() {
            // The caption's own timing is the backstop. If a voice is speaking
            // it will say so first; if synthesis failed, this is what stops the
            // caption sitting there forever.
            Some(act)
                if self.spoken.is_none() && now.saturating_sub(self.spoke_at) >= act.dwell() =>
            {
                self.speech.finished();
            }
            Some(_) => {}
            None => {
                if let Some(act) = self.speech.next(now) {
                    self.spoke_at = now;
                    self.say(&act, ui.ctx());
                }
            }
        }
        if self.speech.speaking().is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }

        if self.streaming() {
            // Tokens arrive between frames; ask for the next one rather than
            // waiting for input that may never come.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }

        let need_key = matches!(self.screen, Screen::NeedKey { .. });

        egui::CentralPanel::default().show(ui, |ui| {
            if need_key {
                self.key_screen(ui);
                return;
            }

            // Compose is laid out first so it pins to the bottom; the
            // transcript then takes whatever height is left.
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                self.compose(ui);
                ui.separator();

                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    self.transcript(ui);
                });
            });
        });
    }
}
