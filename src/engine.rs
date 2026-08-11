//! The turn, running whether or not anybody is looking.
//!
//! Ward is worn rather than watched. The window spends most of its life
//! minimized behind a game, and for a while that made Ward stop: the turn ran
//! inside the window's paint function, and a minimized window does not paint.
//! Holding the key was heard, transcribed, and then dropped into a channel
//! nobody read. The journal went unread in the same place, so auto-honk and
//! route callouts went quiet too — four symptoms of one cause.
//!
//! So the engine is the application and the window is a viewer of it. Nothing
//! here waits to be drawn. It advances on its own inputs — what the microphone
//! heard, what the model said, what the game wrote, what the Commander asked
//! for — and publishes a picture of itself afterwards.
//!
//! # Nothing here may name the toolkit
//!
//! That is the whole guarantee, and it is a compile error rather than a promise.
//! When there is a window to nudge, the engine calls a `wake` closure it was
//! handed, exactly the way the listening thread already does. `check.sh` holds
//! the rule, because the honest failure mode is not somebody arguing for a
//! dependency on the window — it is somebody reaching for `ctx` because it was
//! within reach.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::anthropic::{Client, Event, Message, Role};
use crate::capability::Registry;
use crate::captions::Captions;
use crate::config::Settings;
use crate::speech::{Act, Arbiter, Body, Register, Speaker};
use crate::{capabilities, config, diag, journal, listen, shown, voice};

/// How long the voice service gets to answer before Ward gives up on a piece.
///
/// Generous, because synthesis of a long piece legitimately takes seconds, and
/// finite because the alternative turned out to be forever: an unanswered
/// connection held the speech stream open and every later reply arrived on
/// screen in silence.
const PATIENCE: Duration = Duration::from_secs(20);

/// How often the game's journal is read.
///
/// Polled rather than watched, and not as fast as a screen refreshes: the
/// journal changes a few times a minute at most, and reading a file sixty times
/// a second to find nothing is work nobody asked for.
const JOURNAL: Duration = Duration::from_millis(900);

/// How often the speech stream is looked at when nothing else has happened.
///
/// This is the backstop that retires a caption when no voice ever reported
/// finishing. It is not how captions are paced — the voice does that, from
/// inside playback — so this can be slow enough to be free.
const UPKEEP: Duration = Duration::from_millis(200);

/// Somewhere to send a nudge when there is something new to look at.
///
/// The engine does not know whether a window exists, and must keep running when
/// one does not.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

type Running = Pin<Box<dyn Future<Output = ()> + Send>>;

/// How a turn is started.
///
/// A call rather than the client itself, so the engine can be driven with the
/// model stood in for. That is not a convenience: the claim worth testing is
/// that a turn completes with nothing painting, and the only way to test it
/// without a network is to be able to answer for the model.
pub type Ask = Arc<dyn Fn(Vec<Message>, String, UnboundedSender<Event>) -> Running + Send + Sync>;

/// How something gets said.
///
/// Returns a channel that fires once when speaking has stopped, however it
/// stopped. The engine waits on that rather than on a clock, which is what
/// keeps the speech stream moving at the pace of the voice.
///
/// The receiver carries the rest of a reply that is still being written, and is
/// absent for anything Ward already has whole.
pub type Speak = Arc<
    dyn Fn(&Act, Option<UnboundedReceiver<String>>, &Settings) -> UnboundedReceiver<()>
        + Send
        + Sync,
>;

/// What the Commander asked for, on its way in from the window.
pub enum Intent {
    /// A typed question.
    Ask(String),
    /// The settings changed. The whole set, because the engine rebuilds from
    /// them rather than tracking which one moved.
    Settings(Box<Settings>),
    /// A key was stored, so there is a model to talk to now.
    Key(String),
    /// Speak this so it can be heard before being committed to. Straight to the
    /// voice rather than onto the speech stream: this is the Commander
    /// auditioning a voice, not Ward saying something.
    Preview(String),
    /// Run one of a capability's tools, as the panel does when a box is ticked.
    Tool(&'static str, String),
}

/// What the window draws.
///
/// A picture rather than the thing itself. The window holds no conversation, no
/// checklist and no journal of its own, so there is nothing for the two of them
/// to disagree about.
#[derive(Default)]
pub struct View {
    pub history: Vec<Message>,
    /// The reply as it arrives. Held apart from `history` so a turn that fails
    /// halfway leaves no half-message behind pretending to be an answer.
    pub pending: Option<String>,
    /// A capability currently running, so a pause has a reason on screen.
    pub using: Option<String>,
    pub problem: Option<String>,
    /// The key is down and the Commander is talking.
    pub listening: bool,
    pub streaming: bool,
    /// What the capabilities currently have to show, as of the last change.
    pub panels: Vec<shown::Shown>,
}

/// The window's end of the engine.
#[derive(Clone)]
pub struct Handle {
    view: Arc<Mutex<Arc<View>>>,
    intents: UnboundedSender<Intent>,
}

impl Handle {
    /// The latest picture. Cheap enough to call every frame, because it clones
    /// a pointer rather than a conversation.
    pub fn view(&self) -> Arc<View> {
        // Recovered rather than propagated. A panic somewhere holding this lock
        // must not also take away the window's ability to draw - the picture is
        // stale at worst, and a Commander in a headset can see a stale picture
        // and cannot see a poisoned mutex.
        match self.view.lock() {
            Ok(view) => view.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn tell(&self, intent: Intent) {
        // Dropped silently if the engine is gone, which happens only while Ward
        // is closing. There is nothing useful to do about it and nobody left to
        // tell.
        let _ = self.intents.send(intent);
    }
}

/// The pieces the voice needs, gathered so the speaking closure can hold them.
struct Gear {
    playing: voice::Playing,
    hush: listen::Hush,
    captions: Arc<Mutex<Captions>>,
    wake: Wake,
}

pub struct Engine {
    settings: Settings,
    registry: Arc<Registry>,
    /// Kept so the client can be rebuilt when the model or the registry
    /// changes. The key never travels back out to the window to make that
    /// happen: rotating a model is not an occasion to handle a secret.
    client: Option<Client>,
    ask: Option<Ask>,
    speak: Speak,

    history: Vec<Message>,
    pending: Option<String>,
    using: Option<String>,
    problem: Option<String>,
    listening: bool,
    /// Whether this turn has put anything on screen yet.
    showed_something: bool,

    /// The turn in flight, if there is one.
    events: Option<UnboundedReceiver<Event>>,
    /// When it started, for measuring the wait the Commander actually sits
    /// through rather than the part that is convenient to time.
    turn_started: Option<std::time::Instant>,
    /// The reply as it is being written, with whatever has already been handed
    /// to the voice taken off the front.
    sayable: String,
    /// Where a sentence goes once it is finished enough to say. Dropping this
    /// is how the voice is told there is no more coming.
    saying: Option<UnboundedSender<String>>,
    /// The other end, until the speech stream is ready to take it.
    arriving: Option<UnboundedReceiver<String>>,
    /// Whether this reply has already been handed to the speech stream. It is
    /// offered once, when its first sentence is ready, and grows after that.
    offered: bool,
    /// Signals that speaking finished, so the stream can move on. Silence falls
    /// back to the caption's own timing, which is what keeps a failure to speak
    /// from also freezing what is on screen.
    spoken: Option<UnboundedReceiver<()>>,
    /// When the current act started holding the caption.
    spoke_at: Duration,
    speech: Arbiter,

    /// Follows the game's journal. Absent until a folder with one in it turns
    /// up, and re-checked periodically, because Elite starts a new file every
    /// session and may not be running when Ward is.
    watcher: Option<journal::Watcher>,
    situation: journal::Situation,

    gear: Gear,
    view: Arc<Mutex<Arc<View>>>,
}

impl Engine {
    /// Starts the engine on the runtime it is called from, and hands back the
    /// window's end of it.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        settings: Settings,
        registry: Arc<Registry>,
        client: Option<Client>,
        heard: Option<UnboundedReceiver<listen::Voiced>>,
        captions: Arc<Mutex<Captions>>,
        hush: listen::Hush,
        wake: Wake,
    ) -> Handle {
        let gear = Gear {
            playing: voice::Playing::default(),
            hush,
            captions,
            wake,
        };

        launch(Engine::new(settings, registry, client, gear), heard)
    }

    fn new(
        settings: Settings,
        registry: Arc<Registry>,
        client: Option<Client>,
        gear: Gear,
    ) -> Self {
        let speak = speaking(Gear {
            playing: gear.playing.clone(),
            hush: gear.hush.clone(),
            captions: gear.captions.clone(),
            wake: gear.wake.clone(),
        });

        let mut engine = Self {
            settings,
            registry,
            client,
            ask: None,
            speak,
            history: Vec::new(),
            pending: None,
            using: None,
            problem: None,
            listening: false,
            showed_something: false,
            events: None,
            turn_started: None,
            sayable: String::new(),
            saying: None,
            arriving: None,
            offered: false,
            spoken: None,
            spoke_at: Duration::ZERO,
            speech: Arbiter::default(),
            watcher: None,
            situation: journal::Situation::default(),
            gear,
            view: Arc::new(Mutex::new(Arc::new(View::default()))),
        };

        engine.rewire();
        engine
    }

    /// The client, wrapped in the call the engine actually makes.
    ///
    /// Only replaced when there is a client to make it from, so a stood-in
    /// model survives a settings change.
    fn rewire(&mut self) {
        if let Some(client) = self.client.clone() {
            self.ask = Some(Arc::new(move |history, situation, out| {
                let client = client.clone();
                Box::pin(async move { client.stream(&history, situation, out).await })
            }));
        }
    }

    /// The loop. Nothing in here waits on a window.
    async fn run(
        mut self,
        mut intents: UnboundedReceiver<Intent>,
        mut heard: Option<UnboundedReceiver<listen::Voiced>>,
    ) {
        let mut journal = tokio::time::interval(JOURNAL);
        let mut upkeep = tokio::time::interval(UPKEEP);

        // Missed ticks are skipped rather than made up. Waking from a sleeping
        // machine would otherwise fire a burst of journal reads back to back,
        // and every one of them would find the same thing.
        journal.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        upkeep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        tracing::info!(target: "ward::engine", "running");

        self.publish();

        loop {
            let changed = tokio::select! {
                said = next(&mut heard) => match said {
                    Some(voiced) => self.voiced(voiced),
                    // The listening thread ended. Dropped so this branch stops
                    // being waited on - a closed channel answers instantly and
                    // forever, and left in place it would spin the loop as fast
                    // as the machine goes. Typing still works, which is the
                    // whole reason it was kept.
                    None => { heard = None; false }
                },
                intent = intents.recv() => match intent {
                    Some(intent) => self.intent(intent),
                    // Every window is gone, and with it any way to be asked
                    // anything. There is nothing left for the engine to be.
                    None => break,
                },
                event = next(&mut self.events) => match event {
                    Some(event) => self.event(event),
                    None => { self.events = None; true }
                },
                _ = finished(&mut self.spoken) => {
                    self.spoken = None;
                    self.speech.finished();
                    true
                },
                _ = journal.tick() => self.read_journal(),
                _ = upkeep.tick() => false,
            };

            // Always, and after every branch. The stream has to be looked at
            // when a turn ends and when a voice stops, and it also has to be
            // looked at when neither happened - which is what the upkeep tick
            // is for, and the only reason it exists.
            let moved = self.step_speech();

            if changed || moved {
                self.publish();
                (self.gear.wake)();
            }
        }

        tracing::info!(target: "ward::engine", "stopped");
    }

    /// Decides what is said next, and retires what has finished being said.
    fn step_speech(&mut self) -> bool {
        let now = diag::since_start();

        match self.speech.speaking() {
            // The caption's own timing is the backstop. If a voice is speaking
            // it will say so first; if synthesis failed, this is what stops the
            // caption sitting there forever.
            Some(act)
                if self.spoken.is_none() && now.saturating_sub(self.spoke_at) >= act.dwell() =>
            {
                self.speech.finished();
                true
            }
            Some(_) => false,
            None => match self.speech.next(now) {
                Some(act) => {
                    self.spoke_at = now;
                    // Taken rather than borrowed: the words still being written
                    // belong to this act and to nothing after it. Anything else
                    // on the stream - an alert, a callout - carries its own text
                    // and gets nothing here.
                    self.spoken = Some((self.speak)(&act, self.arriving.take(), &self.settings));
                    true
                }
                None => false,
            },
        }
    }

    /// Something the microphone noticed.
    fn voiced(&mut self, voiced: listen::Voiced) -> bool {
        match voiced {
            listen::Voiced::Listening => {
                self.listening = true;
                // Barge-in. The Commander pressing the key is the whole of the
                // decision - Ward stops talking and does not carry on where it
                // left off afterwards.
                self.gear.playing.stop();
                self.speech.quiet();
                self.problem = None;
            }
            listen::Voiced::Quiet => self.listening = false,
            listen::Voiced::Words(text) => {
                self.listening = false;
                tracing::info!(target: "ward::listen", chars = text.len(), "heard");

                // A reply already arriving owns the turn. Said out loud rather
                // than dropped, because in a headset there is no text box to
                // look at and silence is indistinguishable from not being heard.
                if self.events.is_some() {
                    tracing::info!(target: "ward::listen", "a turn was already running");
                    self.problem = Some("Ward was still answering, so that was not sent.".into());
                } else {
                    self.start_turn(text);
                }
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
            }
        }

        true
    }

    /// Something the window asked for.
    fn intent(&mut self, intent: Intent) -> bool {
        match intent {
            Intent::Ask(text) => {
                let text = text.trim().to_string();

                if text.is_empty() || self.events.is_some() {
                    return false;
                }

                self.start_turn(text);
            }
            Intent::Settings(settings) => {
                self.settings = *settings;

                if let Err(e) = self.settings.save(&Settings::path(&config::data_dir())) {
                    tracing::warn!(target: "ward::config", error = %e, "could not save settings");
                    self.problem = Some(format!("That change did not save: {e}"));
                }

                // Rebuilt rather than nudged. A capability reads its settings
                // when it is made, so the honest way to change one is to make
                // it again. Not while a turn is in flight, because a tool is
                // running against the registry that exists now.
                if self.events.is_none() {
                    self.registry =
                        Arc::new(capabilities::registry(&self.settings, &config::data_dir()));

                    if let Some(client) = self.client.as_ref() {
                        self.client =
                            Some(client.with(self.settings.string("model"), self.registry.clone()));
                        self.rewire();
                    }

                    tracing::info!(
                        target: "ward::config",
                        changed = self.settings.changed(),
                        "settings applied"
                    );
                }
            }
            Intent::Key(key) => {
                self.client = Some(Client::new(
                    key,
                    self.settings.string("model"),
                    self.registry.clone(),
                ));
                self.rewire();
            }
            Intent::Preview(text) => {
                let voice = self.settings.string("voice");
                let rate = self.settings.string("speaking rate");
                let playing = self.gear.playing.clone();
                let hush = self.gear.hush.clone();

                playing.stop();

                tokio::spawn(async move {
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

                return false;
            }
            Intent::Tool(tool, item) => {
                let answer = self
                    .registry
                    .run(tool, &serde_json::json!({ "item": item }));
                tracing::info!(target: "ward::checklist", from = "panel", tool, %answer, "edited");
            }
        }

        true
    }

    fn start_turn(&mut self, text: String) {
        let Some(ask) = self.ask.clone() else {
            self.problem = Some("Ward has no key stored, so it cannot answer.".into());
            return;
        };

        self.problem = None;
        self.history.push(Message {
            role: Role::User,
            text,
        });
        self.pending = Some(String::new());
        self.showed_something = false;

        // Cleared rather than assumed empty. A reply the speech stream refused
        // would otherwise leave its half-written words here to be spoken as
        // though they belonged to the question just asked.
        self.sayable.clear();
        self.offered = false;
        self.turn_started = Some(std::time::Instant::now());

        let (words, arriving) = unbounded_channel();
        self.saying = Some(words);
        self.arriving = Some(arriving);

        let (tx, rx) = unbounded_channel();
        self.events = Some(rx);

        tracing::info!(target: "ward::turn", messages = self.history.len(), "turn started");

        let history = self.history.clone();
        let situation = self.situation.brief();

        tokio::spawn(async move { ask(history, situation, tx).await });
    }

    /// Something the model said.
    fn event(&mut self, event: Event) -> bool {
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
                self.sayable.push_str(&chunk);
                self.say_what_is_finished();
            }
            Event::Using(tool) => self.using = Some(tool),
            Event::Done { stop_reason } => {
                let text = self.pending.take().unwrap_or_default();

                // A refusal arrives as an ordinary success with little or no
                // content. Left alone it would render as Ward simply saying
                // nothing, which reads as a bug.
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

                    match self.offered {
                        // Already talking. What is left is the rest of a
                        // sentence and whatever came after it, and it goes to
                        // the voice that is part way through saying the front
                        // of the same reply.
                        true => {
                            let rest = std::mem::take(&mut self.sayable);

                            if let Some(saying) = self.saying.as_ref() {
                                for piece in voice::pieces_after_the_start(&rest) {
                                    let _ = saying.send(piece);
                                }
                            }
                        }
                        // Short enough, or written without a sentence ending in
                        // it, that nothing was ever finished enough to start on.
                        // Said the way it always was.
                        false => {
                            // Dropped before the act is offered. This channel
                            // was opened for a reply that turned out to arrive
                            // whole, and handing it over would give the voice a
                            // closed and empty source to read the reply from -
                            // which is silence, with the answer on screen.
                            self.arriving = None;

                            let now = diag::since_start();
                            self.speech.offer(
                                Act::text(Speaker::Ward, Register::Reply, text.clone(), now),
                                now,
                            );
                        }
                    }

                    self.history.push(Message {
                        role: Role::Assistant,
                        text,
                    });
                }

                // Dropped, which is how the voice is told the reply is complete
                // and it can stop waiting for more.
                self.saying = None;
                self.sayable.clear();
                self.using = None;
                self.events = None;
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
                // Drop the partial reply. Keeping it would leave a fragment on
                // screen that looks like an answer and is not one.
                self.pending = None;
                // And stop the voice waiting for the rest of a reply that is not
                // coming. A turn can fail after Ward has already started saying
                // it: what was said was true when it was said, and the sentence
                // in the middle of being written is not.
                self.saying = None;
                self.arriving = None;
                self.sayable.clear();
                self.using = None;
                // Take the question back out of the history too, so a retry does
                // not send it twice.
                self.history.pop();
                self.events = None;
            }
        }

        true
    }

    /// Hands the voice whatever of the reply is finished enough to say.
    ///
    /// Ward used to wait for the model to stop writing before synthesizing a
    /// word of the answer, and that wait is dead air the Commander sits through
    /// with the reply already on a screen they cannot see from inside a headset.
    /// It is one to two seconds on a short answer and several on a long one, and
    /// none of it buys anything: a sentence that is finished is finished,
    /// whatever is still being written after it.
    ///
    /// The speech stream is offered the reply **once**, when its first sentence
    /// is ready. The rest arrives on a channel to the voice already saying it,
    /// which is what keeps one reply one act — so barge-in still cuts the whole
    /// of it, and the pieces still stay a fetch ahead of the words being spoken
    /// rather than pausing at every sentence.
    fn say_what_is_finished(&mut self) {
        let (ready, used) = voice::settled(&self.sayable, !self.offered);

        if ready.is_empty() {
            return;
        }

        self.sayable.drain(..used);

        let Some(saying) = self.saying.as_ref() else {
            return;
        };

        if !self.offered {
            self.offered = true;

            // What the Commander waits through, measured where they start
            // waiting. This used to be the same instant as "turn finished",
            // which is the whole of what this change moves.
            if let Some(started) = self.turn_started {
                tracing::info!(
                    target: "ward::turn",
                    ms = started.elapsed().as_millis() as u64,
                    chars = ready[0].chars().count(),
                    "first sentence handed to the voice"
                );
            }

            // The act carries only what is ready. Nothing reads its text except
            // the backstop that retires a caption when no voice ever reported
            // finishing, and a voice is about to be given this one.
            let now = diag::since_start();
            self.speech.offer(
                Act::text(Speaker::Ward, Register::Reply, ready.join(" "), now),
                now,
            );
        }

        for piece in ready {
            // Failure here means the voice stopped listening - cut off, or never
            // started. The turn carries on either way; what it is writing still
            // has to reach the screen and the history.
            let _ = saying.send(piece);
        }
    }

    /// Reads whatever the game has written since last time.
    fn read_journal(&mut self) -> bool {
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
            return false;
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

        !read.backlog.is_empty() || !read.fresh.is_empty()
    }

    /// Puts a fresh picture where the window can find it.
    fn publish(&self) {
        let view = Arc::new(View {
            history: self.history.clone(),
            pending: self.pending.clone(),
            using: self.using.clone(),
            problem: self.problem.clone(),
            listening: self.listening,
            streaming: self.events.is_some(),
            // Read from the capabilities rather than remembered, which is what
            // keeps an edit made by voice and an edit made on the panel from
            // disagreeing about what is on the list.
            panels: self
                .registry
                .capabilities()
                .iter()
                .filter_map(|c| c.display())
                .collect(),
        });

        match self.view.lock() {
            Ok(mut held) => *held = view,
            Err(poisoned) => *poisoned.into_inner() = view,
        }
    }
}

/// Wires the channels and puts the engine on the runtime it is called from.
///
/// One place, so the end the window keeps and the end the loop waits on are
/// always the two halves of the same channel.
fn launch(engine: Engine, heard: Option<UnboundedReceiver<listen::Voiced>>) -> Handle {
    let (intents, asked) = unbounded_channel();

    let handle = Handle {
        view: engine.view.clone(),
        intents,
    };

    tokio::spawn(engine.run(asked, heard));

    handle
}

/// Waits on a receiver that may not exist.
///
/// A branch of the loop that has nothing to wait on has to wait forever rather
/// than complete, or it would spin the whole loop as fast as the machine goes.
async fn next<T>(rx: &mut Option<UnboundedReceiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// The same, for the signal that speaking stopped.
///
/// Separate because the channel closing means the same thing as a message on
/// it: the task that was speaking is gone, one way or another.
async fn finished(rx: &mut Option<UnboundedReceiver<()>>) {
    match rx {
        Some(rx) => {
            rx.recv().await;
        }
        None => std::future::pending().await,
    }
}

/// Says an act, if it has words and a voice can be had.
///
/// Failure here costs the voice and nothing else. The answer is already on
/// screen, the caption still advances on its own timing, and the reason is
/// written down. A companion that goes quiet is worse than one that never
/// spoke, but a companion that stops working because it could not speak is
/// worse than both.
fn speaking(gear: Gear) -> Speak {
    Arc::new(move |act, arriving, settings| {
        let (tx, rx) = unbounded_channel();

        let Body::Text(text) = &act.body else {
            // Nothing to say, so nothing to wait for. Reported immediately
            // rather than left for the caption clock to time out.
            let _ = tx.send(());
            return rx;
        };

        let text = text.clone();
        let voice = settings.string("voice");
        let rate = settings.string("speaking rate");
        let playing = gear.playing.clone();
        let hush = gear.hush.clone();
        let captions = gear.captions.clone();
        let wake = gear.wake.clone();
        let speaker = match act.speaker {
            Speaker::Ward => None,
            Speaker::System => Some("Ward"),
        };

        tokio::spawn(async move {
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
            // Where the words come from is the difference between a reply and
            // everything else Ward says. A reply is still being written when it
            // starts being spoken, so its pieces arrive one finished sentence at
            // a time; an alert or a callout is a whole line already and is
            // broken up here.
            let mut words = match arriving {
                Some(arriving) => arriving,
                None => {
                    let (whole, one_at_a_time) = unbounded_channel();

                    for piece in voice::into_pieces(&text) {
                        let _ = whole.send(piece);
                    }

                    one_at_a_time
                }
            };

            playing.begin();
            hush.speaking();

            let mut first_audio: Option<u64> = None;

            // One piece is fetched while the one before it is playing, which is
            // what makes the seams silent. Written as a loop that fetched and
            // then played, the wait for the next piece fell into the gap between
            // two sentences and was plainly audible - a pause the length of a
            // network round trip in the middle of a reply.
            //
            // A channel of one is the whole mechanism: the fetcher runs a piece
            // ahead and then waits, so at most one extra piece is ever bought
            // and paid for before an interruption throws it away.
            let (ready, mut waiting) = tokio::sync::mpsc::channel::<(String, voice::Speech)>(1);

            {
                let playing = playing.clone();

                tokio::spawn(async move {
                    // Ends when the reply does: the engine drops its end of this
                    // once the model has stopped writing, and a closed channel
                    // is how the fetcher learns there is nothing more coming.
                    while let Some(piece) = words.recv().await {
                        if playing.cut_off() {
                            break;
                        }

                        // Bounded, because it was not. A websocket that connects
                        // and then says nothing left this task waiting forever,
                        // and the stream above waits for this task to report -
                        // so one stalled connection silenced Ward for the rest
                        // of the session while the answers kept arriving on
                        // screen.
                        let attempt = tokio::time::timeout(
                            PATIENCE,
                            voice::synthesize(&piece, &voice, &rate),
                        )
                        .await;

                        let spoke = match attempt {
                            Ok(spoke) => spoke,
                            Err(_) => {
                                tracing::warn!(
                                    target: "ward::voice",
                                    seconds = PATIENCE.as_secs(),
                                    "the voice service stopped answering"
                                );
                                break;
                            }
                        };

                        match spoke {
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
                    tracing::info!(target: "ward::voice", spoken, "cut off");
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
                    // No total to report any more. A reply is spoken while it is
                    // still being written, so how many pieces it will come to is
                    // not known yet - and a count that was only ever a
                    // denominator is not worth waiting for the answer to.
                    tracing::info!(
                        target: "ward::voice",
                        ms,
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

            // Unconditional, and before anything else. A hush left in place is a
            // microphone that never comes back, and the symptom is a Commander
            // holding the key and being ignored for the rest of the session.
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
            wake();
        });

        rx
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Everything the engine reported saying, in order.
    type Said = Arc<Mutex<Vec<String>>>;

    /// A model that answers without a network, and without a model.
    ///
    /// The claim these tests exist to make is that a turn completes with
    /// nothing drawing it. Standing in for the model is the only way to make
    /// that claim without also asking whether the network was up.
    fn answering(events: Vec<Event>) -> Ask {
        let events = Arc::new(events);

        Arc::new(move |_history, _situation, out| {
            let events = events.clone();

            Box::pin(async move {
                for event in events.iter() {
                    let _ = out.send(event.clone());
                }
            })
        })
    }

    /// A model that says one thing, waits to be let go, and then finishes.
    ///
    /// The waiting is the point. A reply that arrives all at once cannot tell
    /// whether Ward spoke while it was still being written or merely spoke
    /// quickly, and that difference is the whole of what is being claimed.
    fn dawdling(first: &str, rest: &str) -> (Ask, Arc<tokio::sync::Notify>) {
        let go = Arc::new(tokio::sync::Notify::new());
        let waits = go.clone();

        let first = first.to_string();
        let rest = rest.to_string();

        let ask: Ask = Arc::new(move |_history, _situation, out| {
            let first = first.clone();
            let rest = rest.clone();
            let waits = waits.clone();

            Box::pin(async move {
                let _ = out.send(Event::Text(first));
                waits.notified().await;
                let _ = out.send(Event::Text(rest));
                let _ = out.send(Event::Done { stop_reason: None });
            })
        });

        (ask, go)
    }

    /// A voice that reports finishing immediately, and writes down what it was
    /// given.
    ///
    /// A reply arrives as pieces on a channel while it is still being written,
    /// and everything else arrives whole. Both are written down the same way, so
    /// a test can ask what Ward said without knowing which it was.
    fn listening_voice() -> (Speak, Said) {
        let said: Said = Arc::new(Mutex::new(Vec::new()));
        let heard = said.clone();

        let speak: Speak = Arc::new(move |act, arriving, _settings| {
            let (tx, rx) = unbounded_channel();

            match arriving {
                Some(mut arriving) => {
                    let heard = heard.clone();

                    tokio::spawn(async move {
                        while let Some(piece) = arriving.recv().await {
                            heard.lock().unwrap().push(piece);
                        }
                        let _ = tx.send(());
                    });
                }
                None => {
                    if let Body::Text(text) = &act.body {
                        heard.lock().unwrap().push(text.clone());
                    }
                    let _ = tx.send(());
                }
            }

            rx
        });

        (speak, said)
    }

    /// An engine with the model and the voice stood in for, and no window
    /// anywhere in the process.
    fn engine(
        events: Vec<Event>,
    ) -> (
        Handle,
        Said,
        UnboundedSender<listen::Voiced>,
        Arc<AtomicUsize>,
    ) {
        engine_asking(answering(events))
    }

    fn engine_asking(
        ask: Ask,
    ) -> (
        Handle,
        Said,
        UnboundedSender<listen::Voiced>,
        Arc<AtomicUsize>,
    ) {
        let mut settings = Settings::default();

        // Pointed at nothing on purpose. The journal tick still runs - that is
        // half of what these tests are about - and it must not wander into a
        // real Elite folder and start absorbing somebody's session.
        settings.set(
            "journal folder",
            serde_json::json!(
                std::env::temp_dir()
                    .join("ward-no-journal")
                    .to_string_lossy()
            ),
        );

        let woken = Arc::new(AtomicUsize::new(0));
        let counter = woken.clone();

        let (speak, said) = listening_voice();
        let (mic, heard) = unbounded_channel();

        let mut engine = Engine::new(
            settings,
            Arc::new(Registry::new(Vec::new())),
            None,
            Gear {
                playing: voice::Playing::default(),
                hush: listen::Hush::default(),
                captions: Arc::new(Mutex::new(Captions::default())),
                wake: Arc::new(move || {
                    counter.fetch_add(1, Ordering::Relaxed);
                }),
            },
        );

        engine.ask = Some(ask);
        engine.speak = speak;

        (launch(engine, Some(heard)), said, mic, woken)
    }

    /// Waits for something to become true, or gives up.
    ///
    /// Polled rather than signalled, because what is being waited for is the
    /// engine having got somewhere on its own - and a signal to wait on would
    /// have to be one the engine sends, which is the thing under test.
    async fn until(what: impl Fn() -> bool) -> bool {
        for _ in 0..500 {
            if what() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        false
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_turn_runs_to_a_spoken_answer_with_nothing_drawing_it() {
        // The whole point. Ward used to run its turn inside the window's paint
        // function, so minimizing the window made it deaf: the words were
        // heard, transcribed, and left in a channel nobody read. There is no
        // window in this process at all, and no paint to wait for.
        let (ward, said, mic, woken) = engine(vec![
            Event::Text("Forty".into()),
            Event::Text(" tons remaining.".into()),
            Event::Done {
                stop_reason: Some("end_turn".into()),
            },
        ]);

        mic.send(listen::Voiced::Words("how much fuel".into()))
            .unwrap();

        assert!(
            until(|| said.lock().unwrap().len() == 1).await,
            "nothing was ever spoken; the view was {:?}",
            ward.view().problem
        );

        assert_eq!(said.lock().unwrap()[0], "Forty tons remaining.");

        let view = ward.view();
        assert_eq!(view.history.len(), 2, "the exchange should be remembered");
        assert_eq!(view.history[0].text, "how much fuel");
        assert_eq!(view.history[1].text, "Forty tons remaining.");
        assert!(!view.streaming, "the turn should have finished");
        assert_eq!(view.problem, None);

        // And the window was told there was something new to look at, which is
        // the only thing the engine ever asks of it.
        assert!(
            woken.load(Ordering::Relaxed) > 0,
            "the window was never woken"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_opening_sentence_is_spoken_while_the_rest_is_still_being_written() {
        // Ward used to wait for the model to stop writing before synthesizing a
        // word, and a Commander in a headset sat through all of it in silence
        // with the answer on a screen behind the game.
        let (ask, go) = dawdling(
            "Forty tons remaining. ",
            "That is enough for three more jumps.",
        );

        let (ward, said, mic, _) = engine_asking(ask);

        mic.send(listen::Voiced::Words("how much fuel".into()))
            .unwrap();

        assert!(
            until(|| !said.lock().unwrap().is_empty()).await,
            "the opening sentence never reached the voice"
        );

        assert_eq!(said.lock().unwrap()[0], "Forty tons remaining.");

        // The assertion the rest of this test exists for. Without it the one
        // above passes just as well against a Ward that waited for the whole
        // reply and was simply quick about it.
        assert!(
            ward.view().streaming,
            "the model had already finished writing, so speaking early proves nothing"
        );

        go.notify_one();

        assert!(
            until(|| said.lock().unwrap().len() == 2).await,
            "the rest of the reply never followed: {:?}",
            said.lock().unwrap()
        );

        assert_eq!(
            said.lock().unwrap()[1],
            "That is enough for three more jumps."
        );

        // And what is remembered is the whole reply, not the part of it that
        // happened to be finished first.
        assert!(until(|| ward.view().history.len() == 2).await);
        assert_eq!(
            ward.view().history[1].text,
            "Forty tons remaining. That is enough for three more jumps."
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_turn_is_said_out_loud_rather_than_only_shown() {
        // A Commander in a headset cannot read a window that is behind a game.
        // A failure that only ever reaches the screen is a failure they
        // experience as Ward ignoring them.
        let (ward, said, mic, _) = engine(vec![Event::Failed("the model service said 529".into())]);

        mic.send(listen::Voiced::Words("what is my fuel".into()))
            .unwrap();

        assert!(
            until(|| !said.lock().unwrap().is_empty()).await,
            "the failure was never spoken"
        );

        assert!(
            said.lock().unwrap()[0].contains("529"),
            "the reason should be what is said: {:?}",
            said.lock().unwrap()
        );

        let view = ward.view();
        assert!(
            view.history.is_empty(),
            "a question that failed should not be left in the history to be sent twice"
        );
        assert_eq!(view.problem.as_deref(), Some("the model service said 529"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn holding_the_key_stops_ward_talking() {
        // Barge-in, on the engine's side of things. This used to need a frame
        // to be drawn between the key going down and the voice being cut.
        let (ward, said, mic, _) = engine(vec![
            Event::Text("A long answer nobody wants to sit through.".into()),
            Event::Done { stop_reason: None },
        ]);

        mic.send(listen::Voiced::Words("tell me everything".into()))
            .unwrap();

        assert!(until(|| !said.lock().unwrap().is_empty()).await);

        mic.send(listen::Voiced::Listening).unwrap();

        assert!(
            until(|| ward.view().listening).await,
            "the engine never noticed the key going down"
        );
    }
}
