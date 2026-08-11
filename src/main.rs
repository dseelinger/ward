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
mod bindings;
mod capabilities;
mod capability;
mod captions;
mod checklist;
mod config;
mod diag;
mod honk;
mod journal;
mod listen;
mod mic;
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

use anthropic::{Client, Event, Message, Role};
use std::sync::Arc;

use config::{Settings, State};
use eframe::egui;
use serde_json::json;
use speech::{Act, Arbiter, Body, Register, Speaker};
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
    /// Follows the game's journal. Absent until a folder with one in it turns
    /// up, and re-checked periodically, because Elite starts a new file every
    /// session and may not be running when Ward is.
    watcher: Option<journal::Watcher>,
    situation: journal::Situation,
    last_looked: std::time::Duration,
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
    /// Speech in progress, so holding the key can cut it off mid-word.
    playing: voice::Playing,
    /// What is on screen in the headset. Shared with the thread that draws it,
    /// which runs on SteamVR's clock rather than the window's.
    captions: std::sync::Arc<std::sync::Mutex<captions::Captions>>,
    /// Tells the microphone to ignore Ward's own voice coming back through the
    /// room. The gate half of barge-in; the handle above is the duck half.
    hush: listen::Hush,
    /// What the listening thread has noticed. Absent if it could not start,
    /// which costs the microphone and nothing else - typing still works.
    heard: Option<UnboundedReceiver<listen::Voiced>>,
    /// The key is down and the Commander is talking.
    listening: bool,
    /// Whether this turn has put anything on screen yet.
    showed_something: bool,
    /// Where the window is, in the units the window system uses.
    placement: Option<egui::Pos2>,
    /// A new checklist item being typed. Held here rather than in the panel so
    /// a half-typed line survives the frame it was typed on.
    adding: String,
    /// The settings page, and whether it is what the window is showing.
    page: page::Page,
    settings_open: bool,
    /// Answers to things the settings page asked for, which all take long
    /// enough that asking on the drawing thread would stop the window.
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
            {
                // The window is not otherwise repainting while Elite has focus,
                // so what the microphone hears would sit in the channel until
                // somebody moved the mouse.
                let ctx = ctx.clone();
                move || ctx.request_repaint()
            },
        ));

        let (errand_tx, errand_rx) = unbounded_channel();

        // Started whether or not there is a headset. It keeps asking, because
        // SteamVR is usually not running yet when Ward is.
        let captions = std::sync::Arc::new(std::sync::Mutex::new(captions::Captions::default()));
        overlay::spawn(captions.clone());

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
            watcher: None,
            situation: journal::Situation::default(),
            last_looked: std::time::Duration::ZERO,
            speech: Arbiter::default(),
            spoke_at: std::time::Duration::ZERO,
            spoken: None,
            playing: voice::Playing::default(),
            captions,
            hush,
            heard,
            listening: false,
            showed_something: false,
            adding: String::new(),
            placement: None,
            page: page::Page::default(),
            settings_open: false,
            errands: errand_rx,
            errand: errand_tx,
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
        self.showed_something = false;

        let (tx, rx): (UnboundedSender<Event>, UnboundedReceiver<Event>) = unbounded_channel();
        self.events = Some(rx);

        tracing::info!(target: "ward::turn", messages = self.history.len(), "turn started");

        let history = self.history.clone();
        let ctx = ctx.clone();

        let situation = self.situation.brief();

        self.runtime.spawn(async move {
            client.stream(&history, situation, tx).await;
            // The window is not otherwise waiting on anything, so nudge it or
            // the last fragment sits in the channel until the mouse moves.
            ctx.request_repaint();
        });
    }

    /// Reads whatever the game has written since last time.
    ///
    /// Polled rather than watched, and not on every frame: the journal changes
    /// a few times a minute at most, and reading a file sixty times a second to
    /// find nothing is work nobody asked for.
    fn read_journal(&mut self, now: std::time::Duration) {
        if now.saturating_sub(self.last_looked) < std::time::Duration::from_millis(900) {
            return;
        }
        self.last_looked = now;

        let folder = std::path::PathBuf::from(self.settings.string("journal folder"));

        // The game starts a new file every session, so the newest one is
        // re-checked rather than chosen once at startup. Without this, Ward
        // stops learning anything the moment the Commander restarts the game.
        if let Some(newest) = journal::newest(&folder) {
            let changed = self
                .watcher
                .as_ref()
                .map(|w| w.path() != newest)
                .unwrap_or(true);

            if changed {
                tracing::info!(
                    target: "ward::journal",
                    file = %newest.file_name().unwrap_or_default().to_string_lossy(),
                    "following"
                );
                self.watcher = Some(journal::Watcher::new(newest));
            }
        }

        let Some(watcher) = self.watcher.as_mut() else {
            return;
        };

        let read = watcher.poll();

        // The session so far builds state without being announced. Only what
        // happened since Ward started looking is news.
        for record in read.backlog.iter().chain(read.fresh.iter()) {
            self.situation.absorb(record);
        }

        if !read.backlog.is_empty() {
            tracing::info!(
                target: "ward::journal",
                events = read.backlog.len(),
                "caught up on the session so far"
            );
        }

        for record in &read.fresh {
            // The game's own time for the event, not Ward's. Lining an event up
            // against when the game says it happened is the whole point of
            // reading the timestamp rather than stamping it on arrival.
            tracing::debug!(
                target: "ward::journal",
                event = %record.event,
                at = record.timestamp.map(|t| t.to_string()).unwrap_or_default(),
                "observed"
            );

            // Only what happened since Ward started looking. Replaying the
            // backlog through here would fire the scanner for every arrival of
            // the session at once, every time Ward starts.
            self.registry.on_event(record);
        }
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
        let playing = self.playing.clone();
        let hush = self.hush.clone();
        let captions = self.captions.clone();
        let speaker = match act.speaker {
            Speaker::Ward => None,
            Speaker::System => Some("Ward"),
        };

        let (tx, rx) = unbounded_channel();
        self.spoken = Some(rx);

        self.runtime.spawn(async move {
            let started = std::time::Instant::now();

            // Spoken in pieces, and the first piece is short.
            //
            // Synthesizing the whole reply before playing any of it cost a
            // median of six seconds of silence with the answer already on
            // screen, and fifteen on a long one - measured, not guessed. The
            // service produces audio faster than it plays, so the opening
            // sentence was ready almost immediately every time and then waited
            // for the rest of the paragraph to be made.
            //
            // Each piece is fetched while the one before it is playing, so
            // after the first the queue stays ahead of the voice and the seams
            // fall at sentence ends where a speaker pauses anyway.
            let pieces = voice::into_pieces(&text);

            playing.begin();
            hush.speaking();

            let mut first_audio: Option<u64> = None;

            // One piece is fetched while the one before it is playing, which
            // is what makes the seams silent. Written as a loop that fetched
            // and then played, the wait for the next piece fell into the gap
            // between two sentences and was plainly audible - a pause the
            // length of a network round trip in the middle of a reply.
            //
            // A channel of one is the whole mechanism: the fetcher runs a piece
            // ahead and then waits, so at most one extra piece is ever bought
            // and paid for before an interruption throws it away.
            let (ready, mut waiting) = tokio::sync::mpsc::channel::<(String, voice::Speech)>(1);

            {
                let playing = playing.clone();
                let pieces = pieces.clone();

                tokio::spawn(async move {
                    for piece in pieces {
                        if playing.cut_off() {
                            break;
                        }

                        match voice::synthesize(&piece, &voice, &rate).await {
                            // The receiver is gone, which means playback stopped
                            // and nothing is waiting for this.
                            Ok(speech) => {
                                if ready.send((piece, speech)).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(target: "ward::voice", error = %e, "could not speak");
                                break;
                            }
                        }
                    }
                });
            }

            let mut spoken = 0usize;

            while let Some((piece, speech)) = waiting.recv().await {
                if playing.cut_off() {
                    tracing::info!(target: "ward::voice", spoken, of = pieces.len(), "cut off");
                    break;
                }

                // The captions for this piece, told how long the voice will take
                // over it. Handed over here rather than from the window, because
                // the window is not painted while Ward is minimized - which in a
                // headset is most of the time, and a caption clock that stops
                // there is one that leaves a caption on screen forever.
                if let Ok(mut captions) = captions.lock() {
                    captions.speaking(&piece, speaker, speech.duration(), diag::since_start());
                }

                if first_audio.is_none() {
                    let ms = started.elapsed().as_millis() as u64;
                    first_audio = Some(ms);
                    tracing::info!(
                        target: "ward::voice",
                        ms,
                        pieces = pieces.len(),
                        chars = piece.chars().count(),
                        "first audio ready"
                    );
                }

                // Playback blocks for as long as the words take, so it goes
                // somewhere that is allowed to block.
                let playing = playing.clone();
                let played =
                    tokio::task::spawn_blocking(move || voice::play(speech.audio, &playing)).await;

                spoken += 1;

                if let Ok(Err(e)) = played {
                    tracing::warn!(target: "ward::voice", error = %e, "could not play");
                    break;
                }
            }

            // Unconditional, and before anything else. A hush left in place is
            // a microphone that never comes back, and the symptom is a
            // Commander holding the key and being ignored for the rest of the
            // session.
            hush.stopped(diag::since_start());

            if let Ok(mut captions) = captions.lock() {
                captions.finished(diag::since_start());
            }

            tracing::info!(
                target: "ward::voice",
                ms = started.elapsed().as_millis() as u64,
                to_first_audio = first_audio.unwrap_or_default(),
                "finished speaking"
            );

            let _ = tx.send(());
            ctx.request_repaint();
        });
    }

    /// Carries out what the settings page asked for.
    ///
    /// **Everything here takes effect now.** A setting that needs a restart is
    /// a setting somebody changes, hears no difference from, and changes back —
    /// and in a headset a restart means taking it off. Rebuilding the registry
    /// and the client is cheaper than teaching every piece of Ward to notice a
    /// setting moving underneath it, and it means one path applies all of them
    /// rather than one path per setting that somebody has to remember to add.
    fn apply(&mut self, wants: page::Wants, ctx: &egui::Context) {
        if let Some(key) = wants.store_key {
            match secrets::store(&secrets::key_path(&config::data_dir()), &key) {
                Ok(()) => {
                    tracing::info!(target: "ward::secrets", "key stored");
                    self.client = Some(Client::new(
                        key,
                        self.settings.string("model"),
                        self.registry.clone(),
                    ));
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
            if let Err(e) = self.settings.save(&Settings::path(&config::data_dir())) {
                tracing::warn!(target: "ward::config", error = %e, "could not save settings");
                self.problem = Some(format!("That change did not save: {e}"));
            }

            ctx.set_zoom_factor(self.settings.f32("text size", 0.5, 4.0));

            // Rebuilt rather than nudged. A capability reads its settings when
            // it is made, so the honest way to change one is to make it again.
            // Not while a turn is in flight, because a tool is running against
            // the registry that exists now.
            if !self.streaming() {
                self.registry =
                    Arc::new(capabilities::registry(&self.settings, &config::data_dir()));

                if let Some(client) = self.client.as_ref() {
                    self.client =
                        Some(client.with(self.settings.string("model"), self.registry.clone()));
                }

                tracing::info!(
                    target: "ward::config",
                    changed = self.settings.changed(),
                    "settings applied"
                );
            }
        }

        if let Some(text) = wants.preview {
            // Straight to the voice rather than onto the speech stream. This is
            // the Commander auditioning a voice, not Ward saying something, and
            // it must not queue behind a reply or land in the transcript.
            let voice = self.settings.string("voice");
            let rate = self.settings.string("speaking rate");
            let playing = self.playing.clone();
            let hush = self.hush.clone();

            playing.stop();

            self.runtime.spawn(async move {
                match voice::synthesize(&text, &voice, &rate).await {
                    Ok(speech) => {
                        hush.speaking();
                        let _ = tokio::task::spawn_blocking(move || {
                            voice::play(speech.audio, &playing)
                        })
                        .await;
                        hush.stopped(diag::since_start());
                    }
                    Err(e) => {
                        tracing::warn!(target: "ward::voice", error = %e, "could not preview");
                    }
                }
            });
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

    /// Takes whatever the listening thread has noticed since the last frame.
    fn pump_voice(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.heard.as_mut() else {
            return;
        };

        let mut said: Option<String> = None;
        let mut gone = false;

        while let Ok(voiced) = rx.try_recv() {
            match voiced {
                listen::Voiced::Listening => {
                    self.listening = true;
                    // Barge-in. The Commander pressing the key is the whole of
                    // the decision - Ward stops talking and does not carry on
                    // where it left off afterwards.
                    self.playing.stop();
                    self.speech.quiet();
                    self.problem = None;
                }
                listen::Voiced::Quiet => {
                    self.listening = false;
                }
                listen::Voiced::Words(text) => {
                    self.listening = false;
                    said = Some(text);
                }
                listen::Voiced::Clash { key, actions } => {
                    self.problem = Some(format!(
                        "{key} is your push-to-talk key and is also bound in Elite to \
                         {}. Holding it to talk will do that too - change one of them.",
                        actions.join(", ")
                    ));
                }
                listen::Voiced::Deaf(why) => {
                    tracing::warn!(target: "ward::listen", reason = %why, "not listening");
                    self.problem = Some(format!("Ward cannot hear you. {why}"));
                    gone = true;
                }
            }
        }

        if gone {
            self.heard = None;
        }

        let Some(text) = said else {
            return;
        };

        tracing::info!(target: "ward::listen", chars = text.len(), "heard");

        self.prompt = text;

        // A reply already arriving owns the turn. Rather than lose what was
        // said, it is left in the box where the Commander can see it and send
        // it themselves - which is the one place the typed path earns its keep
        // twice.
        if self.streaming() {
            tracing::info!(target: "ward::listen", "a turn was already running");
            return;
        }

        self.send(ctx);
    }

    /// Drains whatever the turn has produced since the last frame.
    fn pump(&mut self) {
        let Some(rx) = self.events.as_mut() else {
            return;
        };

        let mut finished = false;

        while let Ok(event) = rx.try_recv() {
            // The moment anything appears, which is where the Commander starts
            // counting. The reply streams in, so this is the first token rather
            // than the last one - measuring from the end of the reply told a
            // flattering story about a wait that begins here.
            if matches!(event, Event::Text(_) | Event::Using(_)) && !self.showed_something {
                self.showed_something = true;
                tracing::info!(target: "ward::turn", "first text on screen");
            }

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

            let hint = match (busy, self.listening) {
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

            if send.clicked() || entered {
                self.send(&ctx);
                // Keep focus in the box so a second question does not need the
                // mouse.
                field.request_focus();
            }
        });
    }

    /// Draws whatever the capabilities currently have to show.
    ///
    /// Read fresh every frame rather than remembered, which is what keeps this
    /// from going stale when something changes it by voice.
    ///
    /// Every edit here goes through the same tool the model calls, with the
    /// same words. The panel cannot make a change the model could not, and it
    /// cannot make one in a way the checklist's own rules would refuse.
    fn panel(&mut self, ui: &mut egui::Ui) {
        // Collected first so the borrow of the registry ends before anything
        // below runs a tool against it.
        let displays: Vec<shown::Shown> = self
            .registry
            .capabilities()
            .iter()
            .filter_map(|c| c.display())
            .collect();

        // What the Commander asked for, decided while drawing and carried out
        // after. Running a tool mid-draw would mean the rest of the frame
        // renders a list that has already changed underneath it.
        let mut edit: Option<(&'static str, String)> = None;

        for display in displays {
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

            for row in &rows {
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
            let answer = self.registry.run(tool, &json!({ "item": item }));
            tracing::info!(target: "ward::checklist", from = "panel", tool, %answer, "edited");
        }
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

        self.state.set("window width", json!(self.window.x));
        self.state.set("window height", json!(self.window.y));

        if let Some(corner) = self.placement {
            self.state.set("window x", json!(corner.x));
            self.state.set("window y", json!(corner.y));
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

        self.pump();
        self.pump_voice(&ui.ctx().clone());

        let now = diag::since_start();
        self.read_journal(now);

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
                let stored = self.client.is_some();
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
                        .show(ui, |ui| self.panel(ui));
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
                self.compose(ui);
                ui.separator();

                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    self.transcript(ui);
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
