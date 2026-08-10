//! One stream for everything Ward emits, and one policy deciding what plays.
//!
//! Every audible thing goes through here: replies, callouts, ambient remarks,
//! and the cues that are sounds rather than words. One stream rather than
//! several, because priority, ducking, interruption and captioning all have to
//! agree, and two mechanisms that must agree are two mechanisms that eventually
//! do not. The symptom of getting this wrong is specific and known: a
//! broadcast line spoken in the companion's own voice, or a reply talking over
//! a callout because each had its own queue.
//!
//! An [`Act`] deliberately has **no voice field**. Which voice says a line is
//! resolved when it plays, not when it is written, because the voice can change
//! between the two — a persona switch, or a line that belongs to somebody other
//! than Ward. Baking it in at authoring time is how a line ends up in the wrong
//! mouth.
//!
//! Captions and the live log render off this stream rather than off the model's
//! output, which is what keeps a caption on screen for exactly as long as
//! something was being said.

//! Parts of the policy below have no caller in the application yet, and are
//! marked rather than left as silent warnings. Ambient and callout registers
//! arrive with the first thing Ward says without being asked; cues arrive with
//! the listening chirp; the time limit and the uninterruptible flag arrive with
//! the first line that has either. All of them are exercised by tests meanwhile,
//! because the policy is easier to get right once than to retrofit onto a queue
//! that already has voices attached.

use std::collections::VecDeque;
use std::time::Duration;

/// Who is talking. Not which voice — that is chosen at play time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speaker {
    /// Ward itself. Deliberately unlabeled in captions: a caption that says
    /// "Ward:" before every line wastes the space it has.
    Ward,
    /// Ward's own machinery, reporting on itself.
    System,
}

/// What kind of utterance this is, which is what priority is derived from.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Register {
    /// Background color. First to be dropped.
    Ambient,
    /// Ward noticed something without being asked.
    Callout,
    /// The answer to something the Commander asked.
    Reply,
    /// Something has gone wrong, or is about to.
    Alert,
}

impl Register {
    /// Higher wins. Derived rather than set per act, so two callouts cannot
    /// disagree about how important a callout is.
    pub fn priority(self) -> u8 {
        match self {
            Register::Ambient => 0,
            Register::Callout => 1,
            Register::Reply => 2,
            Register::Alert => 3,
        }
    }
}

/// A sound rather than a sentence — the cue that says Ward is listening.
///
/// Cues ride this stream rather than a parallel one, so that ducking,
/// interruption and priority apply to them without a second mechanism keeping
/// step with the first.
///
/// Still unused, and the listening cue is why it is worth being careful here.
/// The key that starts a turn is the same key that cuts one short, so a press
/// has to be understood before it can be answered with a sound: a cue fired on
/// the interrupting press would be Ward making a noise at the moment it was
/// told to be quiet. Ward runs without one for now, deliberately.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    Text(String),
    Cue(&'static str),
}

/// One thing to say.
#[derive(Clone, Debug, PartialEq)]
pub struct Act {
    pub speaker: Speaker,
    pub register: Register,
    pub body: Body,
    /// Whether something more important may cut this off mid-word.
    pub interruptible: bool,
    /// What this line is *about*.
    ///
    /// A newer act with the same subject replaces an older one still waiting,
    /// rather than queueing behind it. Two fuel warnings are not twice as
    /// useful; the second one is the only one worth hearing.
    pub subject: Option<String>,
    /// How long this stays worth saying.
    ///
    /// Ambient color about a system stops being interesting once you have left
    /// it. Without this, a queue that backs up says stale things with total
    /// confidence.
    pub ttl: Option<Duration>,
    /// Monotonic, so a clock adjustment cannot make an act look newer than one
    /// queued after it.
    pub queued_at: Duration,
}

impl Act {
    pub fn text(
        speaker: Speaker,
        register: Register,
        text: impl Into<String>,
        now: Duration,
    ) -> Self {
        Self {
            speaker,
            register,
            body: Body::Text(text.into()),
            interruptible: true,
            subject: None,
            ttl: None,
            queued_at: now,
        }
    }

    #[allow(dead_code)]
    pub fn cue(name: &'static str, now: Duration) -> Self {
        Self {
            speaker: Speaker::System,
            register: Register::Ambient,
            body: Body::Cue(name),
            interruptible: true,
            subject: None,
            ttl: None,
            queued_at: now,
        }
    }

    pub fn about(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    #[allow(dead_code)]
    pub fn expiring_after(mut self, ttl: Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }

    #[allow(dead_code)]
    pub fn uninterruptible(mut self) -> Self {
        self.interruptible = false;
        self
    }

    fn stale(&self, now: Duration) -> bool {
        match self.ttl {
            Some(ttl) => now.saturating_sub(self.queued_at) > ttl,
            None => false,
        }
    }

    /// How long this stays on screen, and later how long a caption holds after
    /// the words stop.
    ///
    /// Long enough to read: a floor for short lines, and time proportional to
    /// length beyond that. Capped, because a caption that outstays the moment
    /// is furniture.
    ///
    /// Timed from when speaking **ends**, not when it starts. Timing it from
    /// the start clears the caption while the voice is still talking, which is
    /// the specific way this goes wrong.
    pub fn dwell(&self) -> Duration {
        let characters = match &self.body {
            Body::Text(text) => text.chars().count(),
            Body::Cue(_) => 0,
        };

        let seconds = (0.06 * characters as f64).clamp(2.8, 10.0);
        Duration::from_secs_f64(seconds)
    }

    /// What a caption shows.
    ///
    /// Ward is unlabeled; anybody else is named. A caption is small and the
    /// Commander already knows who the companion is.
    pub fn caption(&self) -> Option<String> {
        match &self.body {
            Body::Cue(_) => None,
            Body::Text(text) => Some(match self.speaker {
                Speaker::Ward => text.clone(),
                Speaker::System => format!("Ward: {text}"),
            }),
        }
    }
}

/// What offering an act did.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Waiting its turn.
    Queued,
    /// Replaced an older act about the same subject.
    Replaced,
    /// Cut off whatever was being said.
    Preempted,
    /// Not worth saying. Carries why, because a line that silently never plays
    /// is indistinguishable from a bug.
    Dropped(&'static str),
}

/// Decides what gets said, and what gets cut off to say it.
pub struct Arbiter {
    queue: VecDeque<Act>,
    speaking: Option<Act>,
    /// Beyond this, the least important waiting act is dropped. A backlog that
    /// grows without limit becomes Ward talking about things that stopped being
    /// true minutes ago.
    depth: usize,
}

impl Default for Arbiter {
    fn default() -> Self {
        Self {
            queue: VecDeque::new(),
            speaking: None,
            depth: 8,
        }
    }
}

impl Arbiter {
    #[allow(dead_code)]
    pub fn new(depth: usize) -> Self {
        Self {
            depth,
            ..Default::default()
        }
    }

    pub fn speaking(&self) -> Option<&Act> {
        self.speaking.as_ref()
    }

    #[allow(dead_code)]
    pub fn waiting(&self) -> usize {
        self.queue.len()
    }

    /// Offers an act to the stream.
    pub fn offer(&mut self, act: Act, now: Duration) -> Outcome {
        if act.stale(now) {
            return Outcome::Dropped("already stale when offered");
        }

        // Supersede by subject: the newer line about a thing is the one worth
        // hearing, and the older one should not be waiting behind it.
        if let Some(slot) = act.subject.as_deref().and_then(|subject| {
            self.queue
                .iter()
                .position(|q| q.subject.as_deref() == Some(subject))
        }) {
            self.queue[slot] = act;
            return Outcome::Replaced;
        }

        // Cut off what is being said only if something more important arrived
        // and the current line permits it.
        if let Some(current) = &self.speaking {
            let outranks = act.register.priority() > current.register.priority();

            if outranks && current.interruptible {
                self.speaking = None;
                self.queue.push_front(act);
                return Outcome::Preempted;
            }
        }

        self.queue.push_back(act);

        if self.queue.len() > self.depth {
            // Drop the least important waiting act rather than the oldest: a
            // backlog should shed color, not the reply somebody asked for.
            if let Some(slot) = self
                .queue
                .iter()
                .enumerate()
                .min_by_key(|(i, a)| (a.register.priority(), std::cmp::Reverse(*i)))
                .map(|(i, _)| i)
            {
                let shed = self.queue.remove(slot);
                if shed.as_ref().map(|a| a.queued_at) == Some(now) {
                    return Outcome::Dropped("queue is full");
                }
            }
        }

        Outcome::Queued
    }

    /// Takes the next act worth saying, skipping any that went stale waiting.
    pub fn next(&mut self, now: Duration) -> Option<Act> {
        while let Some(act) = self.queue.pop_front() {
            if act.stale(now) {
                continue;
            }
            self.speaking = Some(act.clone());
            return Some(act);
        }

        self.speaking = None;
        None
    }

    /// The current act finished on its own.
    pub fn finished(&mut self) {
        self.speaking = None;
    }

    /// Stop talking, and do not carry on with what was queued.
    ///
    /// What the Commander means by interrupting. Dropping only the line being
    /// spoken would start the next one a beat later, which is a companion that
    /// cannot be told to be quiet - and everything waiting was queued while
    /// Ward held the floor, so none of it is what they now want to hear.
    ///
    /// An alert survives, because an alert is the case where being talked over
    /// is the lesser harm.
    pub fn quiet(&mut self) {
        self.speaking = None;
        self.queue.retain(|act| act.register == Register::Alert);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    fn reply(text: &str, now: Duration) -> Act {
        Act::text(Speaker::Ward, Register::Reply, text, now)
    }

    fn ambient(text: &str, now: Duration) -> Act {
        Act::text(Speaker::Ward, Register::Ambient, text, now)
    }

    #[test]
    fn an_act_carries_no_voice() {
        // Which voice says a line is resolved when it plays. A persona can
        // change between an act being written and being spoken, and baking the
        // voice in at authoring time is how a line ends up in the wrong mouth.
        // This is a structural assertion: the field must not exist.
        let act = reply("hello", at(0));
        let debug = format!("{act:?}");
        assert!(!debug.contains("voice"), "an act named a voice: {debug}");
    }

    #[test]
    fn a_cue_travels_on_the_same_stream_as_speech() {
        let mut a = Arbiter::default();
        a.offer(Act::cue("listening", at(0)), at(0));
        a.offer(reply("hello", at(0)), at(0));

        // Priority applies to the cue exactly as it does to words, without a
        // second mechanism to keep in step.
        assert_eq!(a.waiting(), 2);
        assert!(matches!(
            a.next(at(0)).unwrap().body,
            Body::Cue("listening")
        ));
    }

    #[test]
    fn a_reply_cuts_off_ambient_color() {
        let mut a = Arbiter::default();
        a.offer(ambient("that star is unusually bright", at(0)), at(0));
        a.next(at(0));

        let outcome = a.offer(reply("your jump range is 42 light years", at(1)), at(1));

        assert_eq!(outcome, Outcome::Preempted);
        assert!(a.speaking().is_none(), "the interrupted line stopped");

        let next = a.next(at(1)).unwrap();
        assert_eq!(
            next.body,
            Body::Text("your jump range is 42 light years".into())
        );
    }

    #[test]
    fn an_uninterruptible_act_is_not_cut_off() {
        let mut a = Arbiter::default();
        a.offer(ambient("a long story", at(0)).uninterruptible(), at(0));
        a.next(at(0));

        assert_eq!(a.offer(reply("hello", at(1)), at(1)), Outcome::Queued);
        assert!(a.speaking().is_some(), "it was cut off anyway");
    }

    #[test]
    fn ambient_never_cuts_off_a_reply() {
        let mut a = Arbiter::default();
        a.offer(reply("the answer", at(0)), at(0));
        a.next(at(0));

        assert_eq!(
            a.offer(ambient("by the way", at(1)), at(1)),
            Outcome::Queued
        );
        assert!(a.speaking().is_some());
    }

    #[test]
    fn a_newer_line_about_the_same_thing_replaces_the_older_one() {
        // Two fuel warnings are not twice as useful. The second is the only
        // one worth hearing, and the first should not be waiting behind it.
        let mut a = Arbiter::default();
        a.offer(ambient("fuel at 30 percent", at(0)).about("fuel"), at(0));

        let outcome = a.offer(ambient("fuel at 10 percent", at(1)).about("fuel"), at(1));

        assert_eq!(outcome, Outcome::Replaced);
        assert_eq!(a.waiting(), 1);
        assert_eq!(
            a.next(at(1)).unwrap().body,
            Body::Text("fuel at 10 percent".into())
        );
    }

    #[test]
    fn different_subjects_do_not_replace_each_other() {
        let mut a = Arbiter::default();
        a.offer(ambient("fuel low", at(0)).about("fuel"), at(0));
        a.offer(ambient("hull damaged", at(0)).about("hull"), at(0));
        assert_eq!(a.waiting(), 2);
    }

    #[test]
    fn a_line_that_waited_too_long_is_never_said() {
        // Color about a system stops being interesting once you have left it.
        let mut a = Arbiter::default();
        a.offer(
            ambient("this system has three stars", at(0)).expiring_after(at(100)),
            at(0),
        );

        assert!(a.next(at(500)).is_none(), "it said something stale");
    }

    #[test]
    fn a_line_still_within_its_time_is_said() {
        let mut a = Arbiter::default();
        a.offer(
            ambient("this system has three stars", at(0)).expiring_after(at(1000)),
            at(0),
        );
        assert!(a.next(at(500)).is_some());
    }

    #[test]
    fn an_act_already_stale_when_offered_says_why_it_was_dropped() {
        // A line that silently never plays cannot be told apart from a bug.
        let mut a = Arbiter::default();
        let outcome = a.offer(ambient("old news", at(0)).expiring_after(at(10)), at(5000));
        assert!(matches!(outcome, Outcome::Dropped(_)));
        assert_eq!(a.waiting(), 0);
    }

    #[test]
    fn a_backlog_sheds_color_before_it_sheds_answers() {
        let mut a = Arbiter::new(3);

        a.offer(reply("the answer", at(0)), at(0));
        for i in 0..6 {
            a.offer(ambient(&format!("chatter {i}"), at(0)), at(0));
        }

        assert_eq!(a.waiting(), 3);

        // The reply somebody asked for survived a flood of color.
        let survived: Vec<_> = std::iter::from_fn(|| a.next(at(0)))
            .map(|act| act.body)
            .collect();
        assert!(
            survived.contains(&Body::Text("the answer".into())),
            "the reply was dropped: {survived:?}"
        );
    }

    #[test]
    fn ward_is_unlabeled_in_captions_and_anybody_else_is_named() {
        let ward = reply("forty two light years", at(0));
        assert_eq!(ward.caption().unwrap(), "forty two light years");

        let system = Act::text(Speaker::System, Register::Alert, "no key stored", at(0));
        assert_eq!(system.caption().unwrap(), "Ward: no key stored");
    }

    #[test]
    fn a_cue_has_no_caption() {
        // A caption reading "listening chirp" helps nobody.
        assert!(Act::cue("listening", at(0)).caption().is_none());
    }

    #[test]
    fn a_short_line_still_stays_up_long_enough_to_read() {
        let brief = reply("Yes.", at(0));
        assert_eq!(brief.dwell(), Duration::from_secs_f64(2.8));
    }

    #[test]
    fn a_long_line_gets_more_time_but_not_without_limit() {
        let long = reply(&"a".repeat(100), at(0));
        assert_eq!(long.dwell(), Duration::from_secs_f64(6.0));

        let enormous = reply(&"a".repeat(10_000), at(0));
        assert_eq!(enormous.dwell(), Duration::from_secs_f64(10.0));
    }

    #[test]
    fn finishing_clears_what_was_being_said() {
        let mut a = Arbiter::default();
        a.offer(reply("hello", at(0)), at(0));
        a.next(at(0));
        assert!(a.speaking().is_some());

        a.finished();
        assert!(a.speaking().is_none());
    }

    #[test]
    fn an_empty_stream_yields_nothing() {
        assert!(Arbiter::default().next(at(0)).is_none());
    }

    #[test]
    fn being_interrupted_stops_the_line_and_abandons_the_queue() {
        // What the Commander means by holding the key while Ward is talking.
        // Dropping only the current line would start the next one a beat
        // later, which is a companion that cannot be told to be quiet.
        let mut a = Arbiter::default();
        a.offer(reply("the long answer", at(0)), at(0));
        a.next(at(0));
        a.offer(ambient("and another thing", at(0)), at(0));

        a.quiet();

        assert!(a.speaking().is_none());
        assert_eq!(a.waiting(), 0, "it carried on with what was queued");
    }

    #[test]
    fn being_interrupted_does_not_discard_an_alert() {
        // Being talked over is the lesser harm for the one register that
        // exists to say something has gone wrong.
        let mut a = Arbiter::default();
        a.offer(reply("the long answer", at(0)), at(0));
        a.next(at(0));
        a.offer(
            Act::text(Speaker::System, Register::Alert, "no key stored", at(0)),
            at(0),
        );

        a.quiet();

        assert_eq!(a.waiting(), 1, "the alert was dropped");
    }
}
