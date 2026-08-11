//! What is on screen in the headset, and for how long.
//!
//! Captions are not furniture. They appear because something was said, they
//! hold long enough to be read, and then they are gone — and that constraint is
//! the feature rather than a limitation of it. A caption panel that persists is
//! a thing in your view forever; a caption is a thing you read once.
//!
//! # The numbers come from broadcast captioning, not from taste
//!
//! Subtitling is a solved problem with published standards, and the first
//! version of this file guessed at all of it and got every number wrong. These
//! are Netflix's, from their Timed Text Style Guide:
//!
//! - **42 characters per line**, and **two lines at most**. One line is
//!   preferred, and a second is used only when the first will not hold the text.
//! - **20 characters per second** of reading speed for an adult audience.
//! - **Five sixths of a second minimum** and **seven seconds maximum** for any
//!   one caption.
//! - When two lines are needed, prefer a bottom-heavy shape, and never leave one
//!   or two words stranded on the top line.
//!
//! A reply longer than that becomes several captions shown in turn, which is
//! what subtitling has always done. It is not a wall of text that stands there
//! until it times out.
//!
//! **Timed from when speaking ends, not when it starts.** A caption whose clock
//! starts at the first word disappears while the voice is still talking, which
//! is precisely the failure this exists to prevent: the Commander looks up to
//! read what they just heard and it has already gone.

use std::collections::VecDeque;
use std::time::Duration;

use crate::speech::{Act, Body, Speaker};

/// The most characters on one line.
const PER_LINE: usize = 42;

/// The most lines in one caption.
const LINES: usize = 2;

/// How fast a caption is read, in characters per second.
///
/// Netflix's figure for an adult audience. Ward paces captions a little slower
/// than this rather than faster, because the Commander is flying rather than
/// watching, and a caption that has gone before it was noticed may as well not
/// have appeared.
const READING: f32 = 16.0;

/// Nothing is on screen for less than this, however short it is.
const AT_LEAST: Duration = Duration::from_millis(833);

/// Nothing is on screen for longer than this, however long it is.
const AT_MOST: Duration = Duration::from_secs(7);

/// How long the last caption of an utterance holds after the voice stops.
///
/// Short on purpose. The caption has already been up for the length of the
/// sentence; this is time to finish reading it, not time to read it again.
const AFTER_SPEAKING: Duration = Duration::from_millis(1200);

/// One thing on screen: at most two lines, at most forty-two characters each.
#[derive(Clone, Debug, PartialEq)]
pub struct Caption {
    pub lines: Vec<String>,
    /// Who said it, or nothing for Ward. Ward is unlabeled because a caption is
    /// small and the Commander already knows who the companion is; anybody else
    /// is named because the whole point of naming is that it is not Ward.
    pub speaker: Option<&'static str>,
}

impl Caption {
    fn characters(&self) -> usize {
        self.lines.iter().map(|line| line.chars().count()).sum()
    }

    /// How long this stays up, from its length.
    fn on_screen(&self) -> Duration {
        Duration::from_secs_f32(self.characters() as f32 / READING).clamp(AT_LEAST, AT_MOST)
    }
}

/// Flattens markup into something worth reading at a distance.
///
/// The model writes markdown whether or not it was asked to, and a headset has
/// no renderer for it. Left alone, the Commander reads asterisks — and asterisks
/// around a word are worse than no emphasis at all, because they are noise
/// exactly where the emphasis was meant to help.
///
/// Deliberately not a markdown parser. This flattens; it does not interpret.
pub fn as_plain_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    // Tracked rather than inferred from what has been written, because newlines
    // are turned into spaces on the way out - so by the time a marker is
    // reached, the output no longer remembers that a line just began.
    let mut fresh_line = true;

    while let Some(c) = chars.next() {
        match c {
            // Emphasis, in every spelling the model uses. Dropped rather than
            // rendered: there is no bold in a caption to promote it to.
            //
            // Except where the character is not markup at all. An underscore
            // inside a word is part of the word - Ward's own tool names are
            // written that way, and `checklist_add` read as "checklistadd" is a
            // caption that disagrees with the voice. One standing between two
            // spaces is arithmetic or a stray, and is left alone for the same
            // reason.
            '*' | '_' | '`' => {
                let before = out.chars().last();
                let after = chars.peek().copied();

                let inside_word = before.is_some_and(char::is_alphanumeric)
                    && after.is_some_and(char::is_alphanumeric);
                let standing_alone = before.is_some_and(char::is_whitespace)
                    && after.is_some_and(char::is_whitespace);

                if inside_word || standing_alone {
                    out.push(c);
                    fresh_line = false;
                }
            }

            // Heading and bullet markers, and only where they mean that. A hash
            // inside a sentence is a hash, and a hyphenated word is one word.
            '#' | '-' if fresh_line => {
                while chars.peek() == Some(&'#') {
                    chars.next();
                }
                while chars.peek() == Some(&' ') {
                    chars.next();
                }
                fresh_line = false;
            }

            '\n' => {
                out.push(' ');
                fresh_line = true;
            }

            _ => {
                out.push(c);
                fresh_line = false;
            }
        }
    }

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Breaks a sentence into lines of at most [`PER_LINE`] characters.
///
/// Greedy, then rebalanced. Filling the first line and letting the remainder
/// fall onto the second is what strands one word on its own, and the guidance
/// is explicit that a bottom-heavy shape reads better — so a two-line caption
/// is split as evenly as the words allow, favoring the lower line.
fn into_lines(text: &str) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();

    if words.is_empty() {
        return Vec::new();
    }

    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();

    for word in words {
        let would_be = match line.is_empty() {
            true => word.chars().count(),
            false => line.chars().count() + 1 + word.chars().count(),
        };

        if !line.is_empty() && would_be > PER_LINE {
            lines.push(std::mem::take(&mut line));
        }

        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }

    if !line.is_empty() {
        lines.push(line);
    }

    lines
}

/// Rebalances two lines so the lower one is the longer.
fn bottom_heavy(lines: &mut [String]) {
    if lines.len() != 2 {
        return;
    }

    loop {
        let (top, bottom) = (lines[0].clone(), lines[1].clone());

        let Some((head, tail)) = top.rsplit_once(' ') else {
            return;
        };

        // Only while it keeps the lower line legal and actually evens the two
        // out. Comparing the gap before against the gap after is the whole
        // rule: moving a word that overflows the lower line, or that simply
        // swaps which line is the long one, trades one problem for another.
        let moved = format!("{tail} {bottom}");

        let gap_now = top.chars().count().abs_diff(bottom.chars().count());
        let gap_after = head.chars().count().abs_diff(moved.chars().count());

        if moved.chars().count() > PER_LINE || gap_after >= gap_now {
            return;
        }

        lines[0] = head.to_string();
        lines[1] = moved;
    }
}

/// Turns one utterance into the captions that will show it, in order.
pub fn into_captions(text: &str, speaker: Option<&'static str>) -> Vec<Caption> {
    let flat = as_plain_text(text);
    let lines = into_lines(&flat);

    lines
        .chunks(LINES)
        .map(|chunk| {
            let mut lines = chunk.to_vec();
            bottom_heavy(&mut lines);
            Caption { lines, speaker }
        })
        .collect()
}

/// What is on screen, and what is queued to be.
#[derive(Default)]
pub struct Captions {
    waiting: VecDeque<Caption>,
    showing: Option<Caption>,
    /// When the caption on screen makes way for the next one, or nothing while
    /// the voice is still speaking and the last caption is up.
    until: Option<Duration>,
    /// Whether the voice has stopped. Until it has, the last caption holds.
    speaking: bool,
}

impl Captions {
    /// Something started being said.
    ///
    /// Cues put nothing on screen: a caption reading "listening chirp" helps
    /// nobody, and the sound is its own announcement.
    pub fn started(&mut self, act: &Act, now: Duration) {
        let Body::Text(text) = &act.body else {
            return;
        };

        let speaker = match act.speaker {
            Speaker::Ward => None,
            Speaker::System => Some("Ward"),
        };

        let captions = into_captions(text, speaker);

        if captions.is_empty() {
            return;
        }

        // A new utterance replaces the old one rather than queueing behind it.
        // Ward only speaks one thing at a time, so anything still on screen
        // belongs to something that has stopped being said.
        self.waiting = captions.into();
        self.showing = None;
        self.until = None;
        self.speaking = true;

        self.advance(now);
    }

    /// The voice has stopped, so the last caption is on a clock now.
    pub fn finished(&mut self, now: Duration) {
        self.speaking = false;

        // Only the caption still holding open. One that already has an expiry
        // is paced by its own length and is not waiting on the voice.
        if self.showing.is_some() && self.until.is_none() {
            self.until = Some(now + AFTER_SPEAKING);
        }
    }

    /// Moves to the next caption, or clears the screen.
    fn advance(&mut self, now: Duration) {
        self.showing = self.waiting.pop_front();

        // The last caption of an utterance has no expiry while the voice is
        // still on it, because the voice is what it is captioning. Everything
        // before it is paced by its own length.
        //
        // Held is spelled as *no* expiry rather than as one far in the future,
        // and the difference is not cosmetic: the voice stopping has to be able
        // to tell a caption that is waiting for it from one that is simply
        // long, and a distant expiry looks like the second.
        self.until = self.showing.as_ref().and_then(|caption| {
            match self.waiting.is_empty() && self.speaking {
                true => None,
                false => Some(now + caption.on_screen()),
            }
        });
    }

    /// Advances the screen to whatever should be on it now.
    pub fn tick(&mut self, now: Duration) {
        let Some(until) = self.until else {
            return;
        };

        if now >= until {
            self.advance(now);
        }
    }

    /// What to draw, or nothing.
    pub fn showing(&self) -> Option<&Caption> {
        self.showing.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::Register;

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    fn said(text: &str) -> Act {
        Act::text(Speaker::Ward, Register::Reply, text, at(0))
    }

    fn shown(captions: &Captions) -> Vec<String> {
        captions
            .showing()
            .map(|c| c.lines.clone())
            .unwrap_or_default()
    }

    #[test]
    fn no_line_is_longer_than_the_standard_allows() {
        let long = "Deciat is thirty four light years away and you will need to \
                    refuel at least once on the way, probably at Jameson Memorial.";

        for caption in into_captions(long, None) {
            assert!(caption.lines.len() <= LINES, "{:?}", caption.lines);
            for line in &caption.lines {
                assert!(
                    line.chars().count() <= PER_LINE,
                    "{} characters: {line}",
                    line.chars().count()
                );
            }
        }
    }

    #[test]
    fn a_short_answer_is_one_line_and_not_two() {
        // The guidance is explicit that one line is preferred and a second is
        // used only when the first will not hold the text.
        let captions = into_captions("Forty two light years.", None);
        assert_eq!(captions.len(), 1);
        assert_eq!(captions[0].lines, vec!["Forty two light years."]);
    }

    #[test]
    fn two_lines_are_bottom_heavy_rather_than_top_heavy() {
        // Filling the first line and letting the rest fall onto the second is
        // what strands one word on its own.
        let text = "You will need to refuel at least once on the way there, Commander.";
        let captions = into_captions(text, None);

        assert_eq!(captions.len(), 1, "{:?}", captions);
        let lines = &captions[0].lines;
        assert_eq!(lines.len(), 2);
        assert!(
            lines[1].chars().count() >= lines[0].chars().count(),
            "top heavy: {lines:?}"
        );
    }

    #[test]
    fn a_long_reply_becomes_several_captions_rather_than_a_wall() {
        let long = "The Thargoids are an ancient alien species humanity first \
                    clashed with centuries ago. They went dormant for a long time, \
                    then began resurfacing a few years back, attacking outposts and \
                    infesting systems across the bubble.";

        let captions = into_captions(long, None);

        assert!(captions.len() > 2, "got {} captions", captions.len());
        assert!(captions.iter().all(|c| c.lines.len() <= LINES));
    }

    #[test]
    fn no_caption_outstays_the_maximum() {
        // Seven seconds is the ceiling for one caption, whatever it holds.
        let caption = Caption {
            lines: vec!["a".repeat(PER_LINE), "b".repeat(PER_LINE)],
            speaker: None,
        };
        assert!(caption.on_screen() <= AT_MOST);
    }

    #[test]
    fn no_caption_flashes_past_faster_than_it_can_be_read() {
        let caption = Caption {
            lines: vec!["Yes.".to_string()],
            speaker: None,
        };
        assert_eq!(caption.on_screen(), AT_LEAST);
    }

    #[test]
    fn the_last_caption_waits_for_the_voice_rather_than_the_clock() {
        // The failure this prevents: a caption whose clock runs out disappears
        // while Ward is still saying that sentence.
        let mut captions = Captions::default();
        captions.started(&said("Forty two light years."), at(0));

        captions.tick(at(600_000));
        assert!(
            captions.showing().is_some(),
            "it vanished while Ward was still talking"
        );

        captions.finished(at(600_000));
        captions.tick(at(600_000));
        assert!(
            captions.showing().is_some(),
            "it vanished the moment speech ended"
        );

        captions.tick(at(600_000) + AFTER_SPEAKING + at(1));
        assert!(captions.showing().is_none(), "it never went away");
    }

    #[test]
    fn earlier_captions_are_paced_by_their_own_length() {
        // Only the last one waits on the voice. The ones before it have a
        // sentence still to come, so they move on by themselves.
        let long = "The Thargoids are an ancient alien species humanity first \
                    clashed with centuries ago, and they went dormant for a long \
                    time before resurfacing.";

        let mut captions = Captions::default();
        captions.started(&said(long), at(0));

        let first = shown(&captions);
        assert!(!first.is_empty());

        captions.tick(at(0) + AT_MOST + at(1));
        let second = shown(&captions);

        assert_ne!(first, second, "it never moved on");
        assert!(!second.is_empty(), "it went blank mid-utterance");
    }

    #[test]
    fn a_new_utterance_replaces_whatever_was_up() {
        // Ward says one thing at a time, so anything still on screen belongs to
        // something that has stopped being said.
        let mut captions = Captions::default();
        captions.started(&said("The first thing entirely."), at(0));
        captions.started(&said("The second thing."), at(1000));

        assert_eq!(shown(&captions), vec!["The second thing."]);
    }

    #[test]
    fn ward_is_unlabeled_and_anybody_else_is_named() {
        let mut captions = Captions::default();
        captions.started(&said("forty two light years"), at(0));
        assert_eq!(captions.showing().unwrap().speaker, None);

        captions.started(
            &Act::text(Speaker::System, Register::Alert, "no key stored", at(0)),
            at(0),
        );
        assert_eq!(captions.showing().unwrap().speaker, Some("Ward"));
    }

    #[test]
    fn a_cue_puts_nothing_on_screen() {
        let mut captions = Captions::default();
        captions.started(&Act::cue("listening", at(0)), at(0));
        assert!(captions.showing().is_none());
    }

    #[test]
    fn markdown_never_reaches_the_headset_as_punctuation() {
        assert_eq!(
            as_plain_text("Your **jump range** is _42_ light years."),
            "Your jump range is 42 light years."
        );
    }

    #[test]
    fn a_marker_that_is_not_markup_survives() {
        // Ward's own tool names carry underscores, and a caption reading
        // "checklistadd" disagrees with the voice that just said the name.
        assert_eq!(
            as_plain_text("Use `checklist_add` for that."),
            "Use checklist_add for that."
        );
        assert_eq!(
            as_plain_text("that is a 5 * 3 grid"),
            "that is a 5 * 3 grid"
        );
    }

    #[test]
    fn a_list_reads_as_a_sentence_rather_than_as_dashes() {
        assert_eq!(
            as_plain_text("Still to do:\n- buy tritium\n- refuel"),
            "Still to do: buy tritium refuel"
        );
    }

    #[test]
    fn punctuation_inside_a_sentence_is_left_alone() {
        assert_eq!(
            as_plain_text("Docking at pad #4, mid-flight."),
            "Docking at pad #4, mid-flight."
        );
    }

    #[test]
    fn a_line_with_nothing_left_in_it_is_not_shown() {
        // Markup around nothing flattens to nothing, and an empty caption is a
        // black box over the cockpit with no words in it.
        let mut captions = Captions::default();
        captions.started(&said("****"), at(0));
        assert!(captions.showing().is_none());
    }

    #[test]
    fn a_word_longer_than_a_line_does_not_lose_the_rest_of_the_sentence() {
        // Station and system names get long, and a name that cannot be broken
        // must not swallow what follows it.
        let captions = into_captions(
            "Docking at Hutton-Orbital-Is-A-Very-Long-Name-Indeed-Really now.",
            None,
        );
        let all: String = captions
            .iter()
            .flat_map(|c| c.lines.clone())
            .collect::<Vec<_>>()
            .join(" ");

        assert!(all.contains("now."), "the tail was lost: {all}");
    }
}
