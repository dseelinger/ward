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
mod panel;
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
mod surface;
mod transcribe;
mod update;
mod voice;
mod vr;

use anthropic::Client;
use std::sync::Arc;

use config::{Settings, State};
use eframe::egui;
use engine::Intent;

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

struct Ward {
    state: State,
    /// Kept alive here because it is what the engine runs on. Dropping it is how
    /// everything Ward is doing stops, which is what closing the window should
    /// mean and nothing sooner.
    ///
    /// Never read, and that is the whole job: it is held for its `Drop`. Nothing asks the runtime
    /// for anything any more, because everything that used to be spawned from the window is now
    /// asked of the engine — which is what let the settings page reach the headset.
    #[expect(
        dead_code,
        reason = "held for its Drop, which is what stops the engine"
    )]
    runtime: tokio::runtime::Runtime,
    /// The engine's end of the conversation. Everything Ward knows is read
    /// through this, and everything the Commander does is sent through it.
    engine: engine::Handle,
    /// Where the window is, in the units the window system uses.
    placement: Option<egui::Pos2>,
    /// What is half-typed and which tab is open. The window's copy — the headset
    /// has its own, and neither is the truth about anything.
    local: surface::Local,
    window: egui::Vec2,
    /// What the window last set its own scaling to, so a change made in the
    /// headset reaches the window and an unchanged setting is not reapplied
    /// sixty times a second.
    zoom: f32,
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

        let client = match secrets::load(&path) {
            Ok(Some(key)) => {
                tracing::info!(target: "ward::secrets", "stored key loaded");
                Some(Client::new(key, model.clone(), registry.clone()))
            }
            Ok(None) => {
                tracing::info!(target: "ward::secrets", "no key stored yet");
                None
            }
            // A stored key that will not decrypt is the ordinary consequence of
            // copying the data folder between machines or accounts. Say so and
            // offer the field again rather than refusing to start.
            Err(e) => {
                tracing::warn!(target: "ward::secrets", error = %e, "stored key unreadable");
                None
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
            // Nothing waits on this. It asks the window to draw what the engine
            // has already been told, and the engine carries on whether or not
            // there is a window to ask.
            {
                let ctx = ctx.clone();
                move || ctx.request_repaint()
            },
        ));

        let wake: engine::Wake = {
            let ctx = ctx.clone();
            Arc::new(move || ctx.request_repaint())
        };

        let captions = std::sync::Arc::new(std::sync::Mutex::new(captions::Captions::default()));

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
                captions.clone(),
                hush,
                wake,
            )
        };

        // Started whether or not there is a headset, and after the engine
        // because the panel draws from it. It keeps asking, because SteamVR is
        // usually not running yet when Ward is.
        overlay::spawn(captions, engine.clone());

        Self {
            state,
            runtime,
            engine,
            placement: None,
            local: surface::Local::default(),
            window: egui::Vec2::new(980.0, 720.0),
            zoom: settings.f32("text size", 0.5, 4.0),
        }
    }

    /// Everything the surface asked for, on its way to the engine.
    ///
    /// **Everything here takes effect now.** A setting that needs a restart is
    /// a setting somebody changes, hears no difference from, and changes back —
    /// and in a headset a restart means taking it off.
    fn apply(&mut self, wants: Vec<Intent>) {
        for intent in wants {
            self.engine.tell(intent);
        }
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

        // Last, and only if the Commander already said yes and the download
        // already proved to be the file the release published. Here rather than
        // when it arrived, because an installer window over a running game is
        // worse than the update waiting — and in a headset it is a window
        // nobody can reach.
        //
        // Failing here costs the update and nothing else. Ward is closing.
        if let engine::Update::Ready { version, installer } = &self.engine.view().update {
            match update::install(installer) {
                Ok(()) => {
                    tracing::info!(target: "ward::update", %version, "installing on the way out")
                }
                Err(e) => {
                    tracing::warn!(target: "ward::update", error = %e, "could not start the installer")
                }
            }
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

        // Applied here because it is the window's own scaling, and read from the
        // picture rather than from a copy of the settings, so a text size changed
        // in the headset reaches the window without either of them owning it.
        let wanted = view.settings.f32("text size", 0.5, 4.0);
        if (wanted - self.zoom).abs() > f32::EPSILON {
            ui.ctx().set_zoom_factor(wanted);
            self.zoom = wanted;
        }

        // The window is always the big mode. Mini is the headset's answer to
        // being in the way of flying, and a window has a title bar for that.
        // The strip the headset picks the panel up by is drawn here too, because there is one
        // tree — it is simply a title bar the window has no use for.
        let drawn = surface::show(ui, &view, &mut self.local, surface::Mode::Big);
        self.apply(drawn.intents);
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
