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
mod secrets;
mod sse;

use std::path::PathBuf;

use anthropic::{Client, Event, Message, Role};
use eframe::egui;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Everything writable lives in one folder beside the executable, so there is
/// one thing to delete for a genuine first run and one thing to copy. The
/// override exists so a test can point somewhere disposable.
fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("WARD_DATA_DIR") {
        return PathBuf::from(dir);
    }

    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("data")))
        .unwrap_or_else(|| PathBuf::from("data"))
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 720.0])
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
            cc.egui_ctx.set_zoom_factor(1.35);
            Ok(Box::new(Ward::new()) as Box<dyn eframe::App>)
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
}

impl Ward {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("could not start the async runtime");

        let path = secrets::key_path(&data_dir());

        let (screen, client) = match secrets::load(&path) {
            Ok(Some(key)) => (Screen::Talking, Some(Client::new(key))),
            Ok(None) => (
                Screen::NeedKey {
                    entry: String::new(),
                    problem: None,
                },
                None,
            ),
            // A stored key that will not decrypt is the ordinary consequence of
            // copying the data folder between machines or accounts. Say so and
            // offer the field again rather than refusing to start.
            Err(e) => (
                Screen::NeedKey {
                    entry: String::new(),
                    problem: Some(e.to_string()),
                },
                None,
            ),
        };

        Self {
            runtime,
            screen,
            client,
            history: Vec::new(),
            prompt: String::new(),
            pending: None,
            events: None,
            problem: None,
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

        let history = self.history.clone();
        let ctx = ctx.clone();

        self.runtime.spawn(async move {
            client.stream(&history, tx).await;
            // The window is not otherwise waiting on anything, so nudge it or
            // the last fragment sits in the channel until the mouse moves.
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
                    if let Some(pending) = self.pending.as_mut() {
                        pending.push_str(&chunk);
                    }
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
                        self.history.push(Message {
                            role: Role::Assistant,
                            text,
                        });
                    }

                    finished = true;
                }
                Event::Failed(message) => {
                    self.problem = Some(message);
                    // Drop the partial reply. Keeping it would leave a fragment
                    // on screen that looks like an answer and is not one.
                    self.pending = None;
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

            match secrets::store(&secrets::key_path(&data_dir()), &key) {
                Ok(()) => {
                    self.client = Some(Client::new(key));
                    self.screen = Screen::Talking;
                    return;
                }
                Err(e) => problem = Some(e.to_string()),
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
                    ui.label(if pending.is_empty() { "…" } else { pending });
                }

                if let Some(problem) = self.problem.as_ref() {
                    ui.add_space(10.0);
                    ui.colored_label(ui.visuals().error_fg_color, problem);
                }
            });
    }
}

impl eframe::App for Ward {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.pump();

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
