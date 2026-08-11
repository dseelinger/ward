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
//! - **Five sixths of a second minimum** and **seven seconds maximum** for any
//!   one caption.
//! - When two lines are needed, prefer a bottom-heavy shape, and never leave one
//!   or two words stranded on the top line.
//!
//! A reply longer than that becomes several captions shown in turn, which is
//! what subtitling has always done. It is not a wall of text that stands there
//! until it times out.
//!
//! **Paced by the audio, never by a clock of its own.** Each caption is shown
//! when the speech it belongs to starts playing, and the captions within one
//! piece of speech divide that piece's real duration between them by length.
//! Reading speed was the first attempt and it ran away from the voice inside a
//! sentence: a guess cannot see how fast the speaker is going, and knows nothing
//! of the pauses while the next piece is being fetched.

use std::collections::VecDeque;
use std::time::Duration;

/// The most characters on one line.
const PER_LINE: usize = 42;

/// The most lines in one caption.
const LINES: usize = 2;

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
    pub fn characters(&self) -> usize {
        self.lines.iter().map(|line| line.chars().count()).sum()
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
///
/// **Paced by the audio, not by a clock.** The first version guessed at how
/// long each caption should hold from a reading speed, and it ran away from the
/// voice within a sentence — the guess was never going to match a speaker whose
/// pace it could not see, and the gaps while the next piece of speech was being
/// fetched were not in the guess at all.
///
/// Now each caption is shown when the audio it belongs to starts playing, and
/// the captions within one piece of speech divide that piece's real duration
/// between them by length. The voice cannot drift from the captions because the
/// captions are told how long the voice will take.
#[derive(Default)]
pub struct Captions {
    waiting: VecDeque<(Caption, Duration)>,
    showing: Option<Caption>,
    /// When the caption on screen makes way for the next, or for nothing.
    until: Option<Duration>,
}

impl Captions {
    /// A piece of speech has started playing, and takes `audio` to say.
    ///
    /// Cues put nothing on screen: a caption reading "listening chirp" helps
    /// nobody, and the sound is its own announcement.
    pub fn speaking(
        &mut self,
        text: &str,
        speaker: Option<&'static str>,
        audio: Duration,
        now: Duration,
    ) {
        let captions = into_captions(text, speaker);

        if captions.is_empty() {
            return;
        }

        // Split the piece's real duration between its captions by how much of
        // the text each one holds. A caption with twice the words is on screen
        // twice as long, which is what the voice is doing with them.
        let total: usize = captions
            .iter()
            .map(Caption::characters)
            .sum::<usize>()
            .max(1);

        self.waiting = captions
            .into_iter()
            .map(|caption| {
                let share = caption.characters() as f64 / total as f64;
                (caption, audio.mul_f64(share))
            })
            .collect();

        self.advance(now);
    }

    /// The voice has stopped, whether it finished or was cut off.
    ///
    /// Called from the thread that plays the audio rather than from the window,
    /// deliberately. The window is not painted while it is minimized, and a
    /// caption whose clock lives there is one that stays on screen forever the
    /// moment the Commander puts Ward out of the way — which is most of the time
    /// in a headset.
    pub fn finished(&mut self, now: Duration) {
        self.waiting.clear();

        if self.showing.is_some() {
            self.until = Some(now + AFTER_SPEAKING);
        }
    }

    fn advance(&mut self, now: Duration) {
        match self.waiting.pop_front() {
            Some((caption, holds)) => {
                self.showing = Some(caption);
                // Never off screen faster than it can be read, and never left
                // up beyond what the standard allows however long the audio is.
                self.until = Some(now + holds.clamp(AT_LEAST, AT_MOST));
            }
            None => {
                self.showing = None;
                self.until = None;
            }
        }
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

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
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
        let captions = into_captions("Forty two light years.", None);
        assert_eq!(captions.len(), 1);
        assert_eq!(captions[0].lines, vec!["Forty two light years."]);
    }

    #[test]
    fn two_lines_are_bottom_heavy_rather_than_top_heavy() {
        let text = "You will need to refuel at least once on the way there, Commander.";
        let captions = into_captions(text, None);

        assert_eq!(captions.len(), 1, "{captions:?}");
        let lines = &captions[0].lines;
        assert_eq!(lines.len(), 2);
        assert!(
            lines[1].chars().count() >= lines[0].chars().count(),
            "top heavy: {lines:?}"
        );
    }

    #[test]
    fn one_long_sentence_is_broken_into_several_captions() {
        let one = "The Thargoids are an ancient alien species that humanity first \
                   clashed with centuries ago, went dormant for a very long time \
                   afterwards, and then began resurfacing to attack outposts.";

        let captions = into_captions(one, None);
        assert!(captions.len() > 1, "{captions:?}");
    }

    #[test]
    fn captions_are_paced_by_the_audio_rather_than_by_a_guess() {
        // The fault this replaced: captions timed from an assumed reading speed
        // ran away from the voice within a sentence, because the guess could
        // not see how fast the speaker was going, and knew nothing of the gaps
        // while the next piece of speech was being fetched.
        let mut captions = Captions::default();

        // Long enough to need more than one caption, or there is nothing for
        // the pacing to get wrong.
        let text = "The first quarter of what is being said here. \
                    The second quarter of what is being said here. \
                    The third quarter of what is being said here. \
                    The fourth quarter of what is being said here.";

        assert!(
            into_captions(text, None).len() >= 2,
            "a single caption cannot drift"
        );

        // The same words said quickly and said slowly. If the captions are
        // paced by the audio then the second one is still on its first caption
        // when the first one has moved on; if they are paced by anything else -
        // a reading speed, a fixed interval - both behave identically and this
        // is the assertion that says so.
        captions.speaking(text, None, at(4000), at(0));
        let first = shown(&captions);
        assert!(!first.is_empty());

        captions.tick(at(2500));
        assert_ne!(
            shown(&captions),
            first,
            "a quick line held as long as a slow one"
        );

        let mut slowly = Captions::default();
        slowly.speaking(text, None, at(20_000), at(0));

        assert_eq!(
            shown(&slowly),
            first,
            "the two did not start in the same place"
        );

        slowly.tick(at(2500));
        assert_eq!(
            shown(&slowly),
            first,
            "it ran ahead of a voice that was taking its time"
        );
    }

    #[test]
    fn a_longer_caption_takes_a_longer_share_of_the_audio() {
        let short = Caption {
            lines: vec!["Yes.".to_string()],
            speaker: None,
        };
        let long = Caption {
            lines: vec!["a".repeat(PER_LINE), "b".repeat(PER_LINE)],
            speaker: None,
        };

        assert!(long.characters() > short.characters() * 10);
    }

    #[test]
    fn nothing_is_left_on_screen_when_the_voice_stops() {
        // The caption layer used to be cleared by the window, which is not
        // painted while Ward is minimized - so a caption stayed up forever the
        // moment the Commander put Ward out of the way, which in a headset is
        // most of the time.
        let mut captions = Captions::default();
        captions.speaking("Something Ward is saying.", None, at(60_000), at(0));

        assert!(captions.showing().is_some());

        captions.finished(at(1000));
        captions.tick(at(1000));
        assert!(
            captions.showing().is_some(),
            "it vanished the instant speech ended"
        );

        captions.tick(at(1000) + AFTER_SPEAKING + at(1));
        assert!(captions.showing().is_none(), "it never went away");
    }

    #[test]
    fn being_cut_off_drops_what_had_not_been_said_yet() {
        // Barge-in. Whatever was queued belongs to a sentence that is no longer
        // going to be spoken.
        let long = "The Thargoids are an ancient alien species humanity first \
                    clashed with centuries ago, and they went dormant for a very \
                    long time before resurfacing across the bubble.";

        let mut captions = Captions::default();
        captions.speaking(long, None, at(20_000), at(0));

        captions.finished(at(500));
        captions.tick(at(500) + AFTER_SPEAKING + at(1));

        assert!(
            captions.showing().is_none(),
            "it carried on after being cut off"
        );
    }

    #[test]
    fn no_caption_flashes_past_faster_than_it_can_be_read() {
        let mut captions = Captions::default();
        captions.speaking("Yes. No. Maybe.", None, at(30), at(0));

        let first = shown(&captions);
        captions.tick(at(100));
        assert_eq!(shown(&captions), first, "it flashed past");
    }

    #[test]
    fn no_caption_outstays_the_maximum() {
        let mut captions = Captions::default();
        captions.speaking("Four words here now.", None, at(600_000), at(0));

        captions.tick(AT_MOST + at(1));
        assert!(captions.showing().is_none(), "it outstayed the ceiling");
    }

    #[test]
    fn a_new_piece_of_speech_replaces_what_was_up() {
        let mut captions = Captions::default();
        captions.speaking("The first thing entirely.", None, at(5000), at(0));
        captions.speaking("The second thing.", None, at(5000), at(1000));

        assert_eq!(shown(&captions), vec!["The second thing."]);
    }

    #[test]
    fn ward_is_unlabeled_and_anybody_else_is_named() {
        let mut captions = Captions::default();

        captions.speaking("forty two light years", None, at(2000), at(0));
        assert_eq!(captions.showing().unwrap().speaker, None);

        captions.speaking("no key stored", Some("Ward"), at(2000), at(0));
        assert_eq!(captions.showing().unwrap().speaker, Some("Ward"));
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
    fn nothing_but_markup_puts_nothing_on_screen() {
        let mut captions = Captions::default();
        captions.speaking("****", None, at(1000), at(0));
        assert!(captions.showing().is_none());
    }

    #[test]
    fn a_word_longer_than_a_line_does_not_lose_the_rest_of_the_sentence() {
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
