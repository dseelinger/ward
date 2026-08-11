//! Hearing the Commander.
//!
//! Push-to-talk is **one gate policy over an audio stream**, not an
//! architecture of its own. The loop below never reads a key; it asks a [`Gate`]
//! whether the Commander is speaking and derives everything else from the
//! answer. Voice activity detection and a wake word are later answers to that
//! same question, so they arrive as another implementation rather than as a
//! rewrite of the loop.
//!
//! **Never a low-level keyboard hook**, for the reason Ward does not use one to
//! send keys either: that is the API keyloggers use, and Ward ships unsigned.
//! The key is polled, which reads the same physical state without installing
//! anything, and works while Elite has focus because it never needed focus.
//!
//! Barge-in is duck-and-gate. Ducking is cancelling playback mid-word; gating is
//! [`Hush`], which throws away what the microphone hears while Ward is speaking
//! and for a moment after. Without the gate, Ward's own voice comes back through
//! the room, gets transcribed, and Ward answers itself. Subtracting the speaker
//! from the microphone properly is echo cancellation, and it drops in behind
//! [`crate::mic::Ears`] without any of this changing.
//!
//! Like the journal watcher, [`Listener`] is **pull-based with no thread or
//! timer of its own**. That is what makes a listening loop testable with no
//! sound card, no model and no Commander. [`spawn`] is the thin part that turns
//! it into a running thread.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::mic::{self, Ears};
use crate::transcribe::{self, Transcriber};

/// How often the gate is asked. Fast enough that no syllable is lost to the
/// asking, slow enough to be free.
const POLL: Duration = Duration::from_millis(15);

/// Audio kept from before the gate opened.
///
/// A Commander starts talking as they reach for the key, not after it
/// registers. Without this the first word arrives clipped, and a clipped first
/// word is the failure that is hardest to notice — the transcription is a
/// plausible sentence that is not the one that was spoken.
const PREROLL: Duration = Duration::from_millis(300);

/// Shorter than this was a brush of the key, not a sentence.
const LEAST_SPEECH: Duration = Duration::from_millis(350);

/// A ceiling on one utterance, so a key held down by accident — or stuck — is
/// bounded trouble rather than a session's worth of audio in memory.
const MOST_SPEECH: Duration = Duration::from_secs(30);

/// Quieter than this at its loudest was a room, not a voice.
const QUIETEST: f32 = 0.01;

/// How long the microphone stays ignored after Ward stops talking.
///
/// The speaker keeps moving after the audio ends and the room keeps returning
/// it, and both would otherwise be transcribed as the Commander.
const TAIL: Duration = Duration::from_millis(250);

fn samples(span: Duration) -> usize {
    (span.as_secs_f64() * mic::RATE as f64) as usize
}

/// Whether the Commander is speaking, right now.
///
/// The question the loop asks. Today a key answers it. Later a voice activity
/// detector or a wake word spotter answers it, and nothing above here changes.
pub trait Gate {
    fn open(&mut self, now: Duration) -> bool;
}

/// A gate that is open while a key is held.
pub struct PushToTalk {
    /// The virtual key the physical key produces on this layout.
    key: u16,
}

impl PushToTalk {
    /// Resolves the game's name for a key into something that can be polled.
    ///
    /// Two translations, deliberately going through the table Ward already has.
    /// Elite names a key by where it sits on the keyboard; Windows reports it
    /// held by what it produces. Going position → produces, on the Commander's
    /// own layout, is what makes the setting mean the same key it names on an
    /// AZERTY keyboard as on a QWERTY one.
    /// The virtual key a name resolves to, or zero for one Ward does not know.
    ///
    /// Zero rather than an option, because the caller is the keyboard hook and there is no key
    /// numbered zero - so an unresolvable name simply spares nothing rather than needing a branch
    /// on the path every keystroke on the machine takes.
    #[must_use]
    pub fn key_for(name: &str) -> u16 {
        Self::on(name).map_or(0, |key| key.key)
    }

    pub fn on(name: &str) -> Option<Self> {
        let scancode = crate::press::scancode(name)?;

        // The extended form, because the plain one reports both shift keys as
        // simply "shift" — and the default push-to-talk key is the right one.
        let key = unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::MapVirtualKeyW(
                scancode as u32,
                windows_sys::Win32::UI::Input::KeyboardAndMouse::MAPVK_VSC_TO_VK_EX,
            )
        };

        if key == 0 {
            return None;
        }

        Some(Self { key: key as u16 })
    }
}

impl Gate for PushToTalk {
    fn open(&mut self, _now: Duration) -> bool {
        // The high bit is "down now". The low bit is "was pressed since last
        // asked", which is a different question and the wrong one: it would
        // report a key that was tapped and released between two polls as still
        // being held.
        let state = unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(self.key as i32)
        };

        (state as u16 & 0x8000) != 0
    }
}

/// Tells the microphone to ignore what it hears for a while.
///
/// Shared with whatever is speaking, and read by the loop. A clone is the same
/// hush, not a copy of it.
#[derive(Clone, Default)]
pub struct Hush(Arc<AtomicU64>);

/// Held until something says otherwise, rather than until a moment that will
/// arrive on its own. Speech takes as long as it takes.
const UNTIL_TOLD: u64 = u64::MAX;

impl Hush {
    /// Ward has started speaking.
    pub fn speaking(&self) {
        self.0.store(UNTIL_TOLD, Ordering::Relaxed);
    }

    /// Ward has stopped, whether it finished or was cut off. The room has not
    /// stopped yet, so this does not release immediately.
    pub fn stopped(&self, now: Duration) {
        self.0
            .store((now + TAIL).as_millis() as u64, Ordering::Relaxed);
    }

    fn holds(&self, now: Duration) -> bool {
        let until = self.0.load(Ordering::Relaxed);
        until == UNTIL_TOLD || (now.as_millis() as u64) < until
    }
}

/// What the loop noticed.
#[derive(Debug, PartialEq)]
pub enum Heard {
    /// The gate opened. Capture is running.
    Opened,
    /// A whole utterance, one channel at [`mic::RATE`].
    Said(Vec<f32>),
    /// The gate closed with nothing in it worth reading.
    Nothing,
}

/// Loudest sample, which is what tells a voice from a room.
fn loudest(audio: &[f32]) -> f32 {
    audio.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

/// Whether an utterance is worth handing to the speech model.
///
/// Both tests exist because of what the model does with audio that fails them:
/// it does not return nothing, it returns a confident sentence. Silence comes
/// back as a caption of the silence, and a quarter-second of key noise comes
/// back as a short common word. Either would be sent to Ward as a question.
fn worth_reading(audio: &[f32]) -> bool {
    audio.len() >= samples(LEAST_SPEECH) && loudest(audio) >= QUIETEST
}

/// The listening loop, without a thread.
pub struct Listener {
    gate: Box<dyn Gate>,
    ears: Box<dyn Ears>,
    hush: Hush,
    /// Audio from before the gate opened, kept rolling and thrown away
    /// continuously.
    preroll: Vec<f32>,
    captured: Vec<f32>,
    open: bool,
}

impl Listener {
    pub fn new(gate: Box<dyn Gate>, ears: Box<dyn Ears>, hush: Hush) -> Self {
        Self {
            gate,
            ears,
            hush,
            preroll: Vec::new(),
            captured: Vec::new(),
            open: false,
        }
    }

    /// Collects whatever arrived, and reports a transition if there was one.
    pub fn tick(&mut self, now: Duration) -> Option<Heard> {
        let arrived = self.ears.drain();
        let hushed = self.hush.holds(now);
        let open = self.gate.open(now);
        let was = std::mem::replace(&mut self.open, open);

        if hushed {
            // Every sample of this is Ward's own voice on its way back through
            // the room. Held audio goes too, not just new audio: the hush can
            // begin part way through something already being collected.
            self.preroll.clear();
            self.captured.clear();
        }

        let arrived: &[f32] = if hushed { &[] } else { &arrived };

        match (was, open) {
            (false, false) => {
                self.remember(arrived);
                None
            }
            (false, true) => {
                // Nothing is awaited here, and nothing may be added that is.
                // Everything needed to start keeping audio was already running.
                self.captured = std::mem::take(&mut self.preroll);
                self.keep(arrived);
                Some(Heard::Opened)
            }
            (true, true) => {
                self.keep(arrived);
                None
            }
            (true, false) => {
                self.keep(arrived);
                self.preroll.clear();

                let said = std::mem::take(&mut self.captured);

                Some(match worth_reading(&said) {
                    true => Heard::Said(said),
                    false => Heard::Nothing,
                })
            }
        }
    }

    /// Keeps the last [`PREROLL`] of audio and forgets the rest.
    fn remember(&mut self, arrived: &[f32]) {
        self.preroll.extend_from_slice(arrived);

        let keep = samples(PREROLL);
        if self.preroll.len() > keep {
            self.preroll.drain(..self.preroll.len() - keep);
        }
    }

    fn keep(&mut self, arrived: &[f32]) {
        let room = samples(MOST_SPEECH).saturating_sub(self.captured.len());

        if room == 0 {
            return;
        }

        self.captured
            .extend_from_slice(&arrived[..arrived.len().min(room)]);
    }
}

/// What reaches the rest of Ward from the listening thread.
#[derive(Debug)]
pub enum Voiced {
    /// The key is down and Ward is collecting.
    Listening,
    /// The key came up with nothing in it. Said nowhere out loud: an accidental
    /// press should not make a noise.
    Quiet,
    /// What the Commander said.
    Words(String),
    /// The push-to-talk key is also bound to something in the game, so holding
    /// it to talk will do that too.
    Clash { key: String, actions: Vec<String> },
    /// Hearing is not available, and why. Said once, and then the thread ends —
    /// typing still works, which is the whole reason it was kept.
    Deaf(String),
}

/// Starts listening on a thread of its own.
///
/// Everything slow happens here, before the first poll: the model is loaded and
/// the device is opened while nobody is waiting, so that by the time a key can
/// be held there is nothing left to set up.
pub fn spawn(
    key: String,
    model: PathBuf,
    options: PathBuf,
    hush: Hush,
    wake: impl Fn() + Send + 'static,
) -> UnboundedReceiver<Voiced> {
    let (tx, rx) = unbounded_channel();

    std::thread::Builder::new()
        .name("ward-listen".to_string())
        .spawn(move || {
            let say = |what: Voiced| {
                let _ = tx.send(what);
                wake();
            };

            let Some(gate) = PushToTalk::on(&key) else {
                say(Voiced::Deaf(format!(
                    "\"{key}\" is not a key Ward can watch for. \
                     Set \"push to talk key\" to a key name from your bindings, \
                     such as Key_RightShift."
                )));
                return;
            };

            // Reported rather than resolved. A Commander who has deliberately
            // put both on one key should not be argued with, but a Commander
            // who has not needs to know before they hold it in the cockpit and
            // something happens.
            if let Some(actions) = clashes(&options, &key) {
                tracing::warn!(
                    target: "ward::listen",
                    key = %key,
                    actions = %actions.join(", "),
                    "the push-to-talk key is bound in the game as well"
                );
                say(Voiced::Clash {
                    key: key.clone(),
                    actions,
                });
            }

            let model = match transcribe::Whisper::load(&model) {
                Ok(model) => model,
                Err(e) => {
                    tracing::error!(target: "ward::listen", error = %e, "no speech model");
                    say(Voiced::Deaf(format!("{e:#}")));
                    return;
                }
            };

            let ears = match mic::Microphone::open() {
                Ok(ears) => ears,
                Err(e) => {
                    tracing::error!(target: "ward::listen", error = %e, "no microphone");
                    say(Voiced::Deaf(format!("{e:#}")));
                    return;
                }
            };

            run(
                Listener::new(Box::new(gate), Box::new(ears), hush),
                model,
                &tx,
                &wake,
            );
        })
        // A thread that will not start is not a reason for Ward to stop. The
        // typed turn is reachable without any of this.
        .map(|_| ())
        .unwrap_or_else(|e| {
            tracing::error!(target: "ward::listen", error = %e, "could not start listening");
        });

    rx
}

/// Polls until nobody is listening any more, which is Ward closing.
///
/// Transcription runs here rather than on a thread of its own, on purpose. A
/// second utterance cannot start until the first has been read, which is
/// correct: two of them at once would compete for the same cores that Elite is
/// drawing frames with, and arrive out of order.
fn run(
    mut listener: Listener,
    mut model: impl Transcriber,
    tx: &tokio::sync::mpsc::UnboundedSender<Voiced>,
    wake: impl Fn(),
) {
    let say = |what: Voiced| {
        let _ = tx.send(what);
        wake();
    };

    // The window has gone. Without this the thread would carry on polling a
    // microphone nobody is reading, and transcribing into a channel with no
    // other end - work that costs a core and shows up in a log looking like a
    // fault.
    while !tx.is_closed() {
        match listener.tick(crate::diag::since_start()) {
            Some(Heard::Opened) => say(Voiced::Listening),
            Some(Heard::Nothing) => say(Voiced::Quiet),
            Some(Heard::Said(audio)) => match model.read(&audio) {
                Ok(text) if text.is_empty() => say(Voiced::Quiet),
                Ok(text) => say(Voiced::Words(text)),
                Err(e) => {
                    tracing::warn!(target: "ward::listen", error = %e, "could not transcribe");
                    say(Voiced::Quiet);
                }
            },
            None => {}
        }

        std::thread::sleep(POLL);
    }

    tracing::info!(target: "ward::listen", "stopped listening");
}

/// Which game actions are on the same key as push-to-talk.
///
/// Fails soft in every direction. A Commander who has never run the game, or
/// has it installed somewhere Ward was not told about, gets no warning rather
/// than an error about a file they were not asked for.
fn clashes(options: &std::path::Path, key: &str) -> Option<Vec<String>> {
    let file = crate::bindings::active_file(options)?;
    let xml = std::fs::read_to_string(file).ok()?;

    let actions = crate::bindings::actions_bound_to(&xml, key);

    match actions.is_empty() {
        true => None,
        false => Some(actions),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// A gate a test can open and close by hand. Cloning shares the switch.
    #[derive(Clone, Default)]
    struct Switch(Rc<Cell<bool>>);

    impl Gate for Switch {
        fn open(&mut self, _now: Duration) -> bool {
            self.0.get()
        }
    }

    /// A microphone that hands over whatever a test queued for it.
    #[derive(Clone, Default)]
    struct Recording(Rc<RefCell<Vec<f32>>>);

    impl Ears for Recording {
        fn drain(&mut self) -> Vec<f32> {
            std::mem::take(&mut *self.0.borrow_mut())
        }
    }

    /// Drives a listener with a gate and a microphone the test controls.
    struct Rig {
        listener: Listener,
        gate: Switch,
        ears: Recording,
        hush: Hush,
        clock: Duration,
    }

    impl Rig {
        fn new() -> Self {
            let hush = Hush::default();
            let gate = Switch::default();
            let ears = Recording::default();

            Self {
                listener: Listener::new(
                    Box::new(gate.clone()),
                    Box::new(ears.clone()),
                    hush.clone(),
                ),
                gate,
                ears,
                hush,
                clock: Duration::ZERO,
            }
        }

        /// One poll: the gate is in this position and this much sound arrived.
        fn tick(&mut self, open: bool, sound: Vec<f32>) -> Option<Heard> {
            self.gate.0.set(open);
            *self.ears.0.borrow_mut() = sound;

            self.clock += POLL;
            self.listener.tick(self.clock)
        }
    }

    /// Audio loud enough and long enough to be speech.
    fn speech(span: Duration) -> Vec<f32> {
        vec![0.4; samples(span)]
    }

    #[test]
    fn holding_the_key_speaking_and_releasing_yields_what_was_said() {
        let mut rig = Rig::new();

        assert_eq!(rig.tick(false, vec![]), None);
        assert_eq!(rig.tick(true, vec![]), Some(Heard::Opened));
        assert_eq!(rig.tick(true, speech(Duration::from_secs(1))), None);

        let Some(Heard::Said(audio)) = rig.tick(false, vec![]) else {
            panic!("the utterance was not returned");
        };

        assert_eq!(audio.len(), samples(Duration::from_secs(1)));
    }

    #[test]
    fn the_word_begun_before_the_key_registered_is_still_there() {
        // A Commander starts talking as they reach for the key. Without the
        // preroll the first word is clipped, and a clipped first word does not
        // fail loudly - it transcribes as a different, plausible sentence.
        let mut rig = Rig::new();

        rig.tick(false, speech(Duration::from_millis(200)));
        rig.tick(true, vec![]);
        rig.tick(true, speech(Duration::from_secs(1)));

        let Some(Heard::Said(audio)) = rig.tick(false, vec![]) else {
            panic!("the utterance was not returned");
        };

        assert_eq!(
            audio.len(),
            samples(Duration::from_millis(1200)),
            "the audio from before the key went down was dropped"
        );
    }

    #[test]
    fn only_the_most_recent_moment_before_the_key_is_kept() {
        // The rolling buffer has to actually roll. Keeping everything would
        // hand the model a session's worth of room noise with a sentence at
        // the end of it.
        let mut rig = Rig::new();

        for _ in 0..20 {
            rig.tick(false, speech(Duration::from_millis(500)));
        }

        rig.tick(true, vec![]);

        let Some(Heard::Said(audio)) = rig.tick(false, speech(Duration::from_secs(1))) else {
            panic!("the utterance was not returned");
        };

        assert_eq!(
            audio.len(),
            samples(PREROLL) + samples(Duration::from_secs(1))
        );
    }

    #[test]
    fn a_brush_of_the_key_says_nothing() {
        let mut rig = Rig::new();

        rig.tick(true, vec![]);
        let outcome = rig.tick(false, speech(Duration::from_millis(100)));

        assert_eq!(outcome, Some(Heard::Nothing));
    }

    #[test]
    fn a_held_key_over_a_silent_room_says_nothing() {
        // The reason this matters is what the model does with silence: it does
        // not return nothing, it returns a caption of the silence, and that
        // would reach Ward as a question.
        let mut rig = Rig::new();

        rig.tick(true, vec![]);
        let outcome = rig.tick(false, vec![0.0001; samples(Duration::from_secs(3))]);

        assert_eq!(outcome, Some(Heard::Nothing));
    }

    #[test]
    fn ward_does_not_hear_itself_talking() {
        // Duck-and-gate, the gate half. Without it Ward's own voice comes back
        // through the room, is transcribed, and Ward answers itself.
        let mut rig = Rig::new();

        rig.hush.speaking();

        rig.tick(true, vec![]);
        rig.tick(true, speech(Duration::from_secs(2)));
        let outcome = rig.tick(false, speech(Duration::from_secs(1)));

        assert_eq!(outcome, Some(Heard::Nothing), "Ward transcribed itself");
    }

    #[test]
    fn the_room_is_ignored_for_a_moment_after_ward_stops() {
        // A speaker keeps moving after the audio ends and the room keeps
        // returning it. Releasing the instant playback finishes would catch
        // both.
        let mut rig = Rig::new();

        rig.hush.speaking();
        rig.hush.stopped(rig.clock);

        rig.tick(true, vec![]);
        let during = rig.tick(true, speech(Duration::from_millis(100)));
        assert_eq!(during, None);

        // Past the tail, the Commander is heard again.
        rig.clock += TAIL;
        rig.tick(true, speech(Duration::from_secs(1)));

        let Some(Heard::Said(audio)) = rig.tick(false, vec![]) else {
            panic!("the Commander was still being ignored after the tail");
        };

        assert_eq!(audio.len(), samples(Duration::from_secs(1)));
    }

    #[test]
    fn interrupting_ward_is_heard_once_the_tail_passes() {
        // Barge-in end to end: the key goes down while Ward is speaking, which
        // is what cancels playback, and what the Commander says after the tail
        // is what gets read.
        let mut rig = Rig::new();

        rig.hush.speaking();

        assert_eq!(
            rig.tick(true, speech(Duration::from_millis(100))),
            Some(Heard::Opened),
            "pressing the key while Ward talks must still open the gate"
        );

        // Playback is stopped and the tail begins.
        rig.hush.stopped(rig.clock);
        rig.clock += TAIL;

        rig.tick(true, speech(Duration::from_secs(1)));

        let Some(Heard::Said(audio)) = rig.tick(false, vec![]) else {
            panic!("the interruption was not heard");
        };

        assert_eq!(audio.len(), samples(Duration::from_secs(1)));
    }

    #[test]
    fn a_key_held_down_forever_does_not_grow_forever() {
        let mut rig = Rig::new();

        rig.tick(true, vec![]);
        for _ in 0..40 {
            rig.tick(true, speech(Duration::from_secs(1)));
        }

        let Some(Heard::Said(audio)) = rig.tick(false, vec![]) else {
            panic!("the utterance was not returned");
        };

        assert_eq!(audio.len(), samples(MOST_SPEECH));
    }

    #[test]
    fn nothing_is_reported_while_the_key_is_simply_down() {
        // The transition is the event. Reporting every poll would have the rest
        // of Ward filtering a stream of "still listening" to find the moment it
        // started.
        let mut rig = Rig::new();

        rig.tick(true, vec![]);
        assert_eq!(rig.tick(true, speech(Duration::from_millis(100))), None);
        assert_eq!(rig.tick(true, speech(Duration::from_millis(100))), None);
    }

    #[test]
    fn a_second_utterance_does_not_carry_the_first_one_with_it() {
        let mut rig = Rig::new();

        rig.tick(true, vec![]);
        rig.tick(false, speech(Duration::from_secs(1)));

        rig.tick(true, vec![]);
        let Some(Heard::Said(audio)) = rig.tick(false, speech(Duration::from_secs(1))) else {
            panic!("the second utterance was not returned");
        };

        assert_eq!(audio.len(), samples(Duration::from_secs(1)));
    }

    #[test]
    fn a_hush_that_was_never_set_holds_nothing_back() {
        assert!(!Hush::default().holds(Duration::from_secs(1)));
    }

    #[test]
    fn speaking_holds_until_told_rather_than_until_a_moment() {
        // Speech takes as long as it takes. A hush with an expiry guessed in
        // advance releases mid-sentence on a long reply.
        let hush = Hush::default();
        hush.speaking();
        assert!(hush.holds(Duration::from_secs(60 * 60)));
    }

    #[test]
    fn a_quiet_utterance_of_ample_length_is_still_rejected() {
        // Length alone is not evidence of speech, and this is the case a
        // duration check on its own would let through: a held key over a room
        // with a fan in it.
        assert!(!worth_reading(&vec![
            0.001;
            samples(Duration::from_secs(5))
        ]));
    }

    #[test]
    fn a_loud_utterance_of_ample_length_is_read() {
        assert!(worth_reading(&vec![0.4; samples(Duration::from_secs(1))]));
    }
}
