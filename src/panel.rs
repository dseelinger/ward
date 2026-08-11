//! What the panel is showing, and the three ways of asking it to change.
//!
//! This module holds the mode and nothing that draws it. That split is what lets the engine and
//! the capability both name a mode without either of them naming the toolkit — the engine must
//! not, and `check.sh` fails the build if it does.
//!
//! # Three ways in, because a headset is not a desk
//!
//! [D6](../docs/decisions.md) asks for the panel to be summoned and dismissed by voice, by hotkey
//! and by controller, and the reason is that in a headset there is always one of the three that is
//! inconvenient. Hands on the stick: say it. Mid-sentence with the model: press it. Controllers in
//! hand and no wish to talk: grab it.
//!
//! Each of the three arrives somewhere different and they must not each keep their own idea of
//! what is showing:
//!
//! - **Voice** lands here, as a request counted below. The engine publishes the count, and the
//!   overlay acts on one it has not seen. A count rather than a flag, because two "show the panel"
//!   in a row are two requests and a flag would swallow the second.
//! - **The hotkey** is polled by the overlay, which is the only thread that knows what is showing.
//! - **The gesture** never leaves the overlay at all.
//!
//! The mode itself lives in the overlay, because it is the only part of Ward that knows whether
//! there is a headset. Nothing here holds one — this is a request, not a state.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use serde_json::Value;

use crate::capability::{Capability, Tool};

/// How much of the panel is being shown.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Mode {
    /// Out of the way entirely. The headset's third state, and the one the window does not have.
    #[default]
    Gone,
    /// What is worth seeing without stopping flying.
    Mini,
    /// Everything the window has.
    Big,
}

impl Mode {
    /// The next mode round, which is what the key does.
    ///
    /// **Every mode is reachable from the key alone.** It was a toggle at first, on the reasoning
    /// that somebody glancing at the mini panel who asks for the panel wants the whole thing. That
    /// reasoning quietly assumed the other two doors into mini were open, and they were not: the
    /// spoken route needs the model to be listening and the grab needs controllers in hand. A
    /// Commander with neither had no way to reach mini at all, which is not a preference about
    /// cycling - it is a mode that shipped unreachable.
    #[must_use]
    pub fn cycled(self) -> Self {
        match self {
            Mode::Gone => Mode::Mini,
            Mode::Mini => Mode::Big,
            Mode::Big => Mode::Gone,
        }
    }

    #[must_use]
    pub fn showing(self) -> bool {
        self != Mode::Gone
    }

    fn code(self) -> u8 {
        match self {
            Mode::Gone => 0,
            Mode::Mini => 1,
            Mode::Big => 2,
        }
    }

    /// Anything unrecognized reads as gone, which is the state that cannot be in the way.
    fn from_code(code: u8) -> Self {
        match code {
            1 => Mode::Mini,
            2 => Mode::Big,
            _ => Mode::Gone,
        }
    }
}

/// How many times the panel has been asked for by voice, and what was last asked for.
///
/// Two numbers rather than a lock, because the engine reads them while publishing a picture and
/// publishing must never wait on anything. They are only ever written from a tool call and only
/// ever read from the overlay, so there is no sequence to get wrong beyond writing the mode before
/// the count that advertises it.
static ASKS: AtomicU64 = AtomicU64::new(0);
static WANTED: AtomicU8 = AtomicU8::new(0);

/// Asks for the panel, from a thread that has no idea whether a headset exists.
pub fn ask(mode: Mode) {
    // The mode first. A reader that sees the new count must see the mode that came with it, and
    // seeing a stale mode under a fresh count would show the wrong thing.
    WANTED.store(mode.code(), Ordering::SeqCst);
    ASKS.fetch_add(1, Ordering::SeqCst);
}

/// How many requests have been made, ever.
#[must_use]
pub fn asks() -> u64 {
    ASKS.load(Ordering::SeqCst)
}

/// What the last request was for.
#[must_use]
pub fn wanted() -> Mode {
    Mode::from_code(WANTED.load(Ordering::SeqCst))
}

/// The key that summons the panel.
///
/// Named the way the game names keys, like every other key setting, so a Commander sets it from
/// the same vocabulary Elite already taught them.
fn summon_key() -> Value {
    serde_json::json!("Key_F9")
}

pub static SETTINGS: &[crate::schema::Row] = &[crate::schema::Row {
    key: "panel key",
    help: "The key that shows and hides Ward's panel in the headset. \
           Named the way Elite names keys, so Key_F9 or Key_Grave.",
    kind: crate::schema::Field::Key,
    card: "The headset",
    doc: None,
    protected: false,
    default: summon_key,
}];

static TOOLS: &[Tool] = &[
    Tool {
        name: "panel_show",
        description: "Shows Ward's panel in the headset, at full size. \
                      Use this when the Commander asks to see the panel, the conversation, \
                      the checklist, the settings, or asks Ward to show itself.",
        slots: &[],
    },
    Tool {
        name: "panel_glance",
        description: "Shows a small panel in the headset carrying only what is worth seeing \
                      at a glance. Use this when the Commander asks for something small, \
                      out of the way, or just wants to keep an eye on Ward.",
        slots: &[],
    },
    Tool {
        name: "panel_hide",
        description: "Takes Ward's panel out of the headset. \
                      Use this when the Commander asks to hide, dismiss, close or get rid of \
                      the panel.",
        slots: &[],
    },
];

/// Lets the panel be asked for out loud.
///
/// It reports what it asked for rather than what happened, and that is honest rather than lazy:
/// this runs on the turn, the panel runs on the overlay thread, and there may be no headset at
/// all. Claiming the panel is showing would be a claim nothing here can check.
pub struct Panel;

impl Capability for Panel {
    fn id(&self) -> &'static str {
        "panel"
    }

    fn group(&self) -> &'static str {
        "The headset"
    }

    fn one_liner(&self) -> &'static str {
        "Shows and hides Ward's panel in the headset."
    }

    fn examples(&self) -> &'static [&'static str] {
        &[
            "show me the panel",
            "put the panel away",
            "give me the small one",
        ]
    }

    fn settings(&self) -> &'static [crate::schema::Row] {
        SETTINGS
    }

    fn tools(&self) -> &'static [Tool] {
        TOOLS
    }

    fn run(&self, tool: &str, _args: &Value) -> String {
        let mode = match tool {
            "panel_show" => Mode::Big,
            "panel_glance" => Mode::Mini,
            "panel_hide" => Mode::Gone,
            other => return format!("Ward has no tool called {other}."),
        };

        ask(mode);

        match mode {
            Mode::Gone => "The panel is dismissed.".to_string(),
            Mode::Mini => "The small panel is up.".to_string(),
            Mode::Big => "The panel is up.".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mode_survives_being_a_number_and_back() {
        // It crosses two threads as a byte. A code that did not round-trip would be a panel that
        // opens in the wrong mode, and the mini and big cases would be indistinguishable in a log.
        for mode in [Mode::Gone, Mode::Mini, Mode::Big] {
            assert_eq!(Mode::from_code(mode.code()), mode);
        }
    }

    #[test]
    fn anything_unrecognized_reads_as_out_of_the_way() {
        // The failure to avoid is a byte nobody wrote turning into a panel stuck over the cockpit.
        assert_eq!(Mode::from_code(200), Mode::Gone);
    }

    #[test]
    fn asking_twice_for_the_same_thing_is_two_requests() {
        // The specific reason this is a count rather than a flag: a Commander who says "show the
        // panel", dismisses it by hand, and says it again must get it back. A flag would already
        // read "big" and the second request would do nothing at all.
        let before = asks();

        ask(Mode::Big);
        ask(Mode::Big);

        assert_eq!(asks(), before + 2);
        assert_eq!(wanted(), Mode::Big);
    }

    #[test]
    fn the_key_alone_reaches_every_mode_and_comes_back() {
        // The failure this replaced: mini shipped reachable only by voice and by grabbing, so a
        // Commander with no controllers in hand and nothing being transcribed could not get to it
        // at all. One key, three presses, back where you started.
        let mut mode = Mode::Gone;
        let mut seen = vec![mode];

        for _ in 0..3 {
            mode = mode.cycled();
            seen.push(mode);
        }

        assert_eq!(
            seen,
            vec![Mode::Gone, Mode::Mini, Mode::Big, Mode::Gone],
            "the key has to reach mini and get back out"
        );
    }

    #[test]
    fn the_panel_is_showing_in_both_of_the_modes_that_draw() {
        assert!(Mode::Mini.showing());
        assert!(Mode::Big.showing());
        assert!(!Mode::Gone.showing());
    }

    #[test]
    fn every_tool_asks_for_a_mode_and_says_so() {
        let panel = Panel;

        for tool in TOOLS {
            let answer = panel.run(tool.name, &serde_json::json!({}));
            assert!(
                !answer.is_empty() && !answer.contains("no tool called"),
                "{} answered {answer}",
                tool.name
            );
        }

        // And the last one wins, so hiding is what a Commander who said "hide it" gets.
        assert_eq!(wanted(), Mode::Gone);
    }

    #[test]
    fn a_tool_nobody_declared_is_refused_rather_than_guessed_at() {
        let answer = Panel.run("panel_wobble", &serde_json::json!({}));
        assert!(answer.contains("no tool called"), "got: {answer}");
    }

    #[test]
    fn the_summoning_key_is_one_ward_can_actually_read() {
        // The setting is a key name from Elite's vocabulary, and a default Ward cannot resolve
        // would be a hotkey that silently never fires.
        let row = &SETTINGS[0];
        let named = (row.default)();
        let named = named.as_str().expect("a key setting is a name");

        assert!(
            crate::press::scancode(named).is_some(),
            "{named} is not a key Ward knows"
        );
    }
}
