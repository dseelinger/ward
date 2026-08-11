//! Firing the discovery scanner on arrival.
//!
//! The one piece of automation with a reputation, and the reason is that it
//! removes a keystroke you make thousands of times and never once enjoy.
//!
//! It is also the first capability with **no tools at all**. Nothing asks Ward
//! to do this; it happens because the game said the Commander arrived
//! somewhere. That shape has to work, because most of what Ward will eventually
//! notice arrives the same way.
//!
//! ## Why pressing the fire key here is safe, and where the line is
//!
//! Elite has no separate binding for the discovery scanner. It fires on primary
//! fire with the scanner selected, so auto-honk means pressing the fire key.
//! That sounds alarming and is not, for one specific reason: hardpoints are
//! retracted coming out of a jump, so primary fire has nothing to fire.
//!
//! It stays safe only while that is true, which is why this fires on arrival
//! and nothing else. It is timing and tedium — pressing a key the Commander
//! was always going to press, at the only moment it does what they wanted.
//! Ward does not aim and does not decide.
//!
//! Off unless switched on. Automation that presses a fire key should be a
//! choice somebody made, not a surprise on first run.

use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

use crate::bindings::Binding;
use crate::capability::{Capability, Tool};
use crate::journal::Record;

/// Elite's name for the action that fires what is selected.
const ACTION: &str = "PrimaryFire";

/// Protected, and this is the clearest case for it in Ward today. The model
/// reads text nobody vetted, and this setting decides whether Ward presses a
/// fire key on its own. Whatever the model can switch on, something written
/// into a reply can switch on.
pub static SETTINGS: &[crate::schema::Row] = &[crate::schema::Row {
    key: "auto honk",
    help: "Fire the discovery scanner when you arrive in a system. \
           Needs primary fire bound to a key, not only to your stick.",
    kind: crate::schema::Field::Flag,
    card: "Flying",
    doc: Some("capabilities/honk.md"),
    protected: true,
    default: || serde_json::json!(false),
}];

#[derive(Default)]
struct State {
    /// Where the last honk happened, so arriving somewhere does not fire twice
    /// when the game mentions the system more than once.
    last: Option<String>,
}

pub struct Honk {
    enabled: bool,
    binding: Binding,
    state: Mutex<State>,
}

impl Honk {
    pub fn new(enabled: bool, binding: Binding) -> Self {
        Self {
            enabled,
            binding,
            state: Mutex::new(State::default()),
        }
    }

    /// Whether arriving here should fire the scanner, and why not if it should
    /// not.
    ///
    /// Separated from the pressing so the whole decision is testable without a
    /// keyboard, a game, or anything that can go wrong being connected to
    /// anything real.
    fn decide(&self, record: &Record) -> Decision {
        if !self.enabled {
            return Decision::Off;
        }

        if record.event != "FSDJump" && record.event != "CarrierJump" {
            return Decision::NotArriving;
        }

        let system = record.text("StarSystem").unwrap_or_default().to_string();

        if system.is_empty() {
            return Decision::NotArriving;
        }

        if let Ok(mut state) = self.state.lock() {
            if state.last.as_deref() == Some(system.as_str()) {
                return Decision::AlreadyHonked;
            }
            state.last = Some(system);
        }

        match &self.binding {
            Binding::Key(key) => match crate::press::scancode(key) {
                Some(code) => Decision::Press(code),
                // The Commander bound it to something real that Ward has no
                // scancode for. Saying so beats pressing something else.
                None => Decision::Unusable(format!(
                    "Firing is bound to {key}, which Ward does not know how to press."
                )),
            },
            other => Decision::Unusable(
                other
                    .refusal("Firing")
                    .unwrap_or_else(|| "Firing cannot be used.".to_string()),
            ),
        }
    }
}

#[derive(Debug, PartialEq)]
enum Decision {
    Press(u16),
    Off,
    NotArriving,
    AlreadyHonked,
    Unusable(String),
}

impl Capability for Honk {
    fn id(&self) -> &'static str {
        "honk"
    }

    fn group(&self) -> &'static str {
        "Flying"
    }

    fn one_liner(&self) -> &'static str {
        "Fires the discovery scanner when you arrive in a system."
    }

    fn examples(&self) -> &'static [&'static str] {
        // Nothing to say. This capability is never asked for, which is the
        // point of it.
        &[]
    }

    fn settings(&self) -> &'static [crate::schema::Row] {
        SETTINGS
    }

    /// None, and deliberately. A capability that reacts to the game rather than
    /// to a question offers the model nothing to call.
    fn tools(&self) -> &'static [Tool] {
        &[]
    }

    fn run(&self, tool: &str, _input: &Value) -> String {
        format!("honk has no tool called {tool}")
    }

    fn on_event(&self, record: &Record) {
        match self.decide(record) {
            Decision::Press(code) => {
                tracing::info!(target: "ward::act", action = ACTION, "firing the scanner");

                // On its own thread: the press holds the key briefly, and
                // nothing that reads the journal should be waiting on that.
                std::thread::spawn(move || {
                    crate::press::tap(code, Duration::from_millis(400));
                });
            }
            Decision::Unusable(why) => {
                tracing::warn!(target: "ward::act", reason = %why, "cannot fire the scanner");
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arrival(system: &str) -> Record {
        Record::parse(&format!(
            "{{\"event\":\"FSDJump\",\"StarSystem\":\"{system}\"}}"
        ))
        .unwrap()
    }

    fn on() -> Honk {
        Honk::new(true, Binding::Key("Key_F".to_string()))
    }

    #[test]
    fn arriving_somewhere_fires_the_scanner() {
        assert_eq!(on().decide(&arrival("Lave")), Decision::Press(0x21));
    }

    #[test]
    fn arriving_at_the_same_place_twice_only_fires_once() {
        // The game mentions a system more than once around a jump. Two honks
        // is one honk and one noise for nothing.
        let honk = on();
        assert!(matches!(honk.decide(&arrival("Lave")), Decision::Press(_)));
        assert_eq!(honk.decide(&arrival("Lave")), Decision::AlreadyHonked);
    }

    #[test]
    fn moving_on_fires_again() {
        let honk = on();
        honk.decide(&arrival("Lave"));
        assert!(matches!(honk.decide(&arrival("Diso")), Decision::Press(_)));
    }

    #[test]
    fn it_does_nothing_unless_switched_on() {
        // Automation that presses a fire key should be a choice somebody made,
        // not a surprise on first run.
        let off = Honk::new(false, Binding::Key("Key_F".to_string()));
        assert_eq!(off.decide(&arrival("Lave")), Decision::Off);
    }

    #[test]
    fn nothing_else_in_the_journal_fires_it() {
        // This is safe only because hardpoints are retracted coming out of a
        // jump. Firing on anything else would eventually press the fire key
        // with weapons deployed.
        let honk = on();
        for event in [
            "Docked",
            "Undocked",
            "SupercruiseExit",
            "FuelScoop",
            "Music",
        ] {
            let record = Record::parse(&format!("{{\"event\":\"{event}\"}}")).unwrap();
            assert_eq!(
                honk.decide(&record),
                Decision::NotArriving,
                "{event} should not fire the scanner"
            );
        }
    }

    #[test]
    fn a_stick_only_binding_explains_itself_rather_than_doing_nothing() {
        let honk = Honk::new(
            true,
            Binding::ElsewhereOnly {
                device: "4098BEA1".to_string(),
            },
        );

        let Decision::Unusable(why) = honk.decide(&arrival("Lave")) else {
            panic!("a joystick binding should be reported, not ignored");
        };

        assert!(why.contains("joystick"), "got: {why}");
    }

    #[test]
    fn an_unbound_fire_key_explains_itself() {
        let honk = Honk::new(true, Binding::Unbound);
        assert!(matches!(
            honk.decide(&arrival("Lave")),
            Decision::Unusable(_)
        ));
    }

    #[test]
    fn a_key_ward_cannot_press_is_reported_rather_than_substituted() {
        let honk = Honk::new(true, Binding::Key("Key_SomethingExotic".to_string()));

        let Decision::Unusable(why) = honk.decide(&arrival("Lave")) else {
            panic!("an unknown key should be reported");
        };

        assert!(why.contains("does not know how to press"), "got: {why}");
    }

    #[test]
    fn it_offers_the_model_nothing_to_call() {
        // The shape that has to work: a capability with no tools at all.
        assert!(on().tools().is_empty());
    }
}
