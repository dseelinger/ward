//! What is on screen in the headset, and for how long.
//!
//! Captions are not furniture. They appear because something was said, they
//! hold long enough to be read, and then they are gone — and that constraint is
//! the feature rather than a limitation of it. A caption panel that persists is
//! a thing in your view forever; a caption is a thing you read once.
//!
//! Three lines, rolling. Enough to catch up on a sentence you half heard,
//! little enough that it never becomes a wall of text over the cockpit.
//!
//! **Timed from when speaking ends, not when it starts.** A caption whose clock
//! starts at the first word disappears while the voice is still talking, which
//! is precisely the failure this exists to prevent: the Commander looks up to
//! read what they just heard and it has already gone.

use std::collections::VecDeque;
use std::time::Duration;

use crate::speech::{Act, Body, Speaker};

/// How many lines are visible at once.
const LINES: usize = 3;

/// One thing that was said.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// What to draw, already flattened to something a headset can read.
    pub text: String,
    /// Who said it, or nothing for Ward. Ward is unlabeled because a caption is
    /// small and the Commander already knows who the companion is; anybody else
    /// is named because the whole point of naming is that it is not Ward.
    pub speaker: Option<&'static str>,
    /// When this stops being shown, or nothing while it is still being said.
    ///
    /// Held open on purpose. A line has no expiry until the voice stops, which
    /// is what makes the clock start at the end of speech rather than the start.
    ends: Option<Duration>,
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
            // written that way, and `checklist_add` read aloud as "checklistadd"
            // is a caption that disagrees with the voice. One standing between
            // two spaces is arithmetic or a stray, and is left alone for the
            // same reason.
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

/// The rolling window.
#[derive(Default)]
pub struct Captions {
    lines: VecDeque<Line>,
}

impl Captions {
    /// Something started being said.
    ///
    /// Cues are silent here: a caption reading "listening chirp" helps nobody,
    /// and the sound is its own announcement.
    pub fn started(&mut self, act: &Act) {
        let Body::Text(text) = &act.body else {
            return;
        };

        let text = as_plain_text(text);

        if text.is_empty() {
            return;
        }

        self.lines.push_back(Line {
            text,
            speaker: match act.speaker {
                Speaker::Ward => None,
                Speaker::System => Some("Ward"),
            },
            ends: None,
        });

        while self.lines.len() > LINES {
            self.lines.pop_front();
        }
    }

    /// The last thing said has finished being said, so its clock starts now.
    ///
    /// Takes the dwell rather than computing one, so the length a caption holds
    /// and the length an act is given are the same number arrived at once.
    pub fn finished(&mut self, now: Duration, dwell: Duration) {
        if let Some(last) = self.lines.iter_mut().rev().find(|line| line.ends.is_none()) {
            last.ends = Some(now + dwell);
        }
    }

    /// Drops whatever has been on screen long enough.
    pub fn tick(&mut self, now: Duration) {
        self.lines
            .retain(|line| line.ends.is_none_or(|ends| now < ends));
    }

    /// What to draw, oldest first.
    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        self.lines.iter()
    }

    /// Whether there is anything to show at all.
    ///
    /// What the overlay hides on. Captions that are not saying anything are not
    /// on screen, which is the difference between a caption and a panel.
    pub fn quiet(&self) -> bool {
        self.lines.is_empty()
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

    #[test]
    fn a_caption_holds_until_the_voice_stops_and_then_some() {
        // The failure this prevents: a caption whose clock starts at the first
        // word disappears while Ward is still talking, so the Commander looks
        // up to read what they just heard and it has already gone.
        let mut captions = Captions::default();
        let act = said("Your jump range is forty two light years.");

        captions.started(&act);

        // A long time passes while it is still being spoken.
        captions.tick(at(60_000));
        assert!(!captions.quiet(), "it vanished mid-sentence");

        captions.finished(at(60_000), act.dwell());
        captions.tick(at(60_000));
        assert!(!captions.quiet(), "it vanished the moment speech ended");

        captions.tick(at(60_000) + act.dwell() + at(1));
        assert!(captions.quiet(), "it never went away");
    }

    #[test]
    fn only_three_lines_are_ever_on_screen() {
        let mut captions = Captions::default();

        for n in 0..6 {
            captions.started(&said(&format!("line {n}")));
        }

        let lines: Vec<&str> = captions.lines().map(|l| l.text.as_str()).collect();

        assert_eq!(lines, vec!["line 3", "line 4", "line 5"]);
    }

    #[test]
    fn ward_is_unlabeled_and_anybody_else_is_named() {
        let mut captions = Captions::default();
        captions.started(&said("forty two light years"));
        captions.started(&Act::text(
            Speaker::System,
            Register::Alert,
            "no key stored",
            at(0),
        ));

        let labels: Vec<Option<&'static str>> = captions.lines().map(|l| l.speaker).collect();
        assert_eq!(labels, vec![None, Some("Ward")]);
    }

    #[test]
    fn a_cue_puts_nothing_on_screen() {
        // A caption reading "listening chirp" helps nobody, and the sound is
        // already its own announcement.
        let mut captions = Captions::default();
        captions.started(&Act::cue("listening", at(0)));
        assert!(captions.quiet());
    }

    #[test]
    fn markdown_never_reaches_the_headset_as_punctuation() {
        // The model writes markdown whether or not it was asked to, and there
        // is nothing in a headset that renders it.
        assert_eq!(
            as_plain_text("Your **jump range** is _42_ light years."),
            "Your jump range is 42 light years."
        );
        assert_eq!(
            as_plain_text("Use `checklist_add` for that."),
            "Use checklist_add for that."
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
    fn a_heading_loses_its_hashes_and_keeps_its_words() {
        assert_eq!(
            as_plain_text("## Checklist\nOne of two done."),
            "Checklist One of two done."
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
    fn punctuation_inside_a_sentence_is_left_alone() {
        // The flattening must not eat characters that were meant. A hash in the
        // middle of a line is a hash, and a hyphenated word is one word.
        assert_eq!(
            as_plain_text("Docking at pad #4, mid-flight."),
            "Docking at pad #4, mid-flight."
        );
    }

    #[test]
    fn a_line_with_nothing_left_in_it_is_not_shown() {
        // Markup around nothing flattens to nothing, and an empty caption is a
        // blank rectangle over the cockpit.
        let mut captions = Captions::default();
        captions.started(&said("****"));
        assert!(captions.quiet());
    }

    #[test]
    fn finishing_twice_does_not_restart_an_older_line() {
        // Only the line still being spoken gets a clock. Without the check,
        // finishing again would reset a caption that had already begun fading.
        let mut captions = Captions::default();
        let first = said("the first thing");
        let second = said("the second thing");

        captions.started(&first);
        captions.finished(at(0), first.dwell());
        captions.started(&second);
        captions.finished(at(1000), second.dwell());

        // The first should expire on its own schedule, not the second's.
        captions.tick(at(0) + first.dwell() + at(1));
        let left: Vec<&str> = captions.lines().map(|l| l.text.as_str()).collect();
        assert_eq!(left, vec!["the second thing"]);
    }
}
