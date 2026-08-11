//! What every setting is, so the settings page can be generated rather than
//! hand-built.
//!
//! One declaration per setting, holding the name, the default, what kind of
//! thing it is and what to say about it. A page built by hand drifts from the
//! code it configures — a setting gets added and the row does not, or the row
//! says a default the code stopped shipping. Generating the page from this
//! makes both impossible.
//!
//! **A default lives here and nowhere else.** It is never written to disk, so
//! there is no seeded file to go stale, nothing to migrate, and no possibility
//! of a shipped configuration quietly disagreeing with the code that reads it.
//!
//! **Some rows belong to a capability and some belong to nobody.** The
//! microphone key, the voice and the journal folder are Ward's rather than any
//! one capability's, so there is a global list here as well as the rows each
//! capability contributes. A capability's rows are static data, deliberately:
//! settings are read before anything is constructed, so asking an instance
//! would be a loop.

use serde_json::{Value, json};

/// What kind of thing a setting is, which is what the page draws.
#[derive(Clone, Copy, PartialEq)]
pub enum Field {
    Text,
    /// A folder on this machine.
    Folder,
    /// A file on this machine.
    File,
    Flag,
    Number {
        floor: f32,
        ceiling: f32,
    },
    /// A key, named the way the game names keys.
    Key,
    /// Chosen from a list that has to be fetched from somewhere.
    Choice(Catalog),
}

/// A list of options that is not known until something is asked.
///
/// Never fails hard. When the list cannot be had, the page says why and the
/// field becomes an ordinary text box with the current value still in it —
/// because an empty dropdown is a setting the Commander can no longer change,
/// and that is worse than one they have to type.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Catalog {
    Models,
    Voices,
}

/// One setting.
pub struct Row {
    /// Spelled the way a person would say it. The same string is the name in
    /// the file, the name on the page, and the name in the message about a
    /// setting Ward does not recognize.
    pub key: &'static str,
    /// What the Commander is choosing, in a sentence.
    pub help: &'static str,
    pub kind: Field,
    /// Which card it appears under.
    pub card: &'static str,
    /// The page that explains it in full, if there is one.
    pub doc: Option<&'static str>,
    /// Whether the model may change it.
    ///
    /// The model reads text Ward did not write — a reply, a journal line, one
    /// day a message from another Commander. A setting it can change is a
    /// setting that text can change. Anything that gates what Ward is willing
    /// to do is set by a person or not at all.
    pub protected: bool,
    /// What ships, when the Commander has chosen nothing.
    pub default: fn() -> Value,
}

fn model() -> Value {
    json!(crate::anthropic::DEFAULT_MODEL)
}

fn voice() -> Value {
    json!(crate::voice::DEFAULT_VOICE)
}

fn rate() -> Value {
    json!("+0%")
}

fn text_size() -> Value {
    json!(1.35)
}

fn push_to_talk() -> Value {
    json!("Key_RightShift")
}

fn speech_model() -> Value {
    json!(r"models\ggml-small.en.bin")
}

fn journal_folder() -> Value {
    let profile = std::env::var("USERPROFILE").unwrap_or_default();
    json!(format!(
        r"{profile}\Saved Games\Frontier Developments\Elite Dangerous"
    ))
}

fn bindings_folder() -> Value {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    json!(format!(
        r"{local}\Frontier Developments\Elite Dangerous\Options\Bindings"
    ))
}

/// Settings that belong to Ward rather than to any one capability.
pub static GLOBAL: &[Row] = &[
    Row {
        key: "voice",
        help: "Which voice Ward speaks in.",
        kind: Field::Choice(Catalog::Voices),
        card: "Voice",
        doc: None,
        protected: false,
        default: voice,
    },
    Row {
        key: "speaking rate",
        help: "How fast Ward talks, against the voice's natural pace. \
               Written the way the service expects it, such as +10% or -5%.",
        kind: Field::Text,
        card: "Voice",
        doc: None,
        protected: false,
        default: rate,
    },
    Row {
        key: "push to talk key",
        help: "The key you hold to talk. Named the way Elite names keys, \
               so Key_RightShift or Key_F10.",
        kind: Field::Key,
        card: "Hearing you",
        doc: None,
        protected: false,
        default: push_to_talk,
    },
    Row {
        key: "speech model",
        help: "The file Ward listens with. A path inside the data folder, \
               or anywhere on this machine.",
        kind: Field::File,
        card: "Hearing you",
        doc: None,
        protected: false,
        default: speech_model,
    },
    Row {
        key: "model",
        help: "Which model runs the turn.",
        kind: Field::Choice(Catalog::Models),
        card: "Thinking",
        doc: None,
        protected: false,
        default: model,
    },
    Row {
        key: "text size",
        help: "How large everything is drawn, on top of whatever scaling \
               Windows is already doing.",
        kind: Field::Number {
            floor: 0.5,
            ceiling: 4.0,
        },
        card: "Appearance",
        doc: None,
        protected: false,
        default: text_size,
    },
    Row {
        key: "journal folder",
        help: "Where Elite writes its journal. Ward reads it to know where \
               you are and what you are flying.",
        kind: Field::Folder,
        card: "The game",
        doc: None,
        protected: false,
        default: journal_folder,
    },
    Row {
        key: "bindings folder",
        help: "Where Elite keeps your control bindings, so Ward can find out \
               which key does what.",
        kind: Field::Folder,
        card: "The game",
        doc: None,
        protected: false,
        default: bindings_folder,
    },
];

/// Every setting this build has, global and contributed.
///
/// Composed here rather than discovered, for the same reason the capability
/// list is: a contributed row that was written and never reached the page is
/// something you can see in one file rather than something you find out about
/// when a Commander cannot switch a feature on.
pub fn all() -> Vec<&'static Row> {
    GLOBAL
        .iter()
        .chain(crate::honk::SETTINGS)
        .chain(crate::checklist::SETTINGS)
        .collect()
}

pub fn row(key: &str) -> Option<&'static Row> {
    all().into_iter().find(|row| row.key == key)
}

/// The cards, in the order the page shows them.
pub fn cards() -> Vec<&'static str> {
    let mut seen: Vec<&'static str> = Vec::new();

    for row in all() {
        if !seen.contains(&row.card) {
            seen.push(row.card);
        }
    }

    seen
}

/// Turns what was typed into what gets stored, or explains why it cannot.
///
/// Empty is not a value. It means the Commander cleared the box, which returns
/// the setting to its default rather than storing an empty string — a stored
/// empty voice name is a Ward that has stopped speaking with no way to see why.
pub fn read(row: &Row, typed: &str) -> Result<Option<Value>, String> {
    let typed = typed.trim();

    if typed.is_empty() {
        return Ok(None);
    }

    match row.kind {
        Field::Flag => match typed.to_lowercase().as_str() {
            "true" | "yes" | "on" => Ok(Some(json!(true))),
            "false" | "no" | "off" => Ok(Some(json!(false))),
            _ => Err("This is either on or off.".to_string()),
        },

        Field::Number { floor, ceiling } => {
            let Ok(value) = typed.parse::<f32>() else {
                return Err("This has to be a number.".to_string());
            };

            if value < floor || value > ceiling {
                return Err(format!("This has to be between {floor} and {ceiling}."));
            }

            Ok(Some(json!(value)))
        }

        Field::Key => match crate::press::scancode(typed) {
            Some(_) => Ok(Some(json!(typed))),
            None => Err(
                "Ward does not know that key. Use the name Elite uses, such as \
                 Key_RightShift."
                    .to_string(),
            ),
        },

        Field::Folder => match std::path::Path::new(typed).is_dir() {
            true => Ok(Some(json!(typed))),
            false => Err("There is no folder there.".to_string()),
        },

        // Not checked for existence. The speech model is large and lives
        // wherever the Commander put it, and refusing a path because the file
        // is not there yet would stop them setting it before fetching it.
        Field::File | Field::Text | Field::Choice(_) => Ok(Some(json!(typed))),
    }
}

/// How a value is shown in a box.
pub fn write(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(key: &str) -> &'static Row {
        row(key).unwrap_or_else(|| panic!("no row for {key}"))
    }

    #[test]
    fn every_setting_has_a_row_and_every_row_has_a_key() {
        let rows = all();

        // A discovery test that finds nothing passes.
        assert!(!rows.is_empty(), "there are no settings at all");

        for row in &rows {
            assert!(!row.key.is_empty());
            assert!(
                row.help.len() > 10,
                "{} needs help text somebody can act on",
                row.key
            );
            assert!(!row.card.is_empty(), "{} belongs to no card", row.key);
        }
    }

    #[test]
    fn no_two_rows_claim_the_same_setting() {
        // Two rows for one key means one of them is unreachable, and which one
        // depends on the order they happen to be composed in.
        let rows = all();
        let mut keys: Vec<&str> = rows.iter().map(|r| r.key).collect();
        let before = keys.len();

        keys.sort_unstable();
        keys.dedup();

        assert_eq!(keys.len(), before, "a setting is declared twice");
    }

    #[test]
    fn clearing_a_box_returns_a_setting_to_its_default() {
        // Not an empty string. A stored empty voice name is a Ward that has
        // stopped speaking, with nothing on the page to show why.
        assert_eq!(read(find("voice"), "   "), Ok(None));
        assert_eq!(read(find("speaking rate"), ""), Ok(None));
    }

    #[test]
    fn a_key_ward_cannot_press_is_refused_with_the_name_of_one_it_can() {
        let refusal = read(find("push to talk key"), "Key_Joystick").unwrap_err();
        assert!(refusal.contains("Key_RightShift"), "got: {refusal}");

        assert_eq!(
            read(find("push to talk key"), "Key_F10"),
            Ok(Some(json!("Key_F10")))
        );
    }

    #[test]
    fn a_folder_that_is_not_there_is_refused() {
        // The journal folder being wrong is the difference between Ward knowing
        // where you are and Ward knowing nothing, and it fails silently.
        let refusal = read(find("journal folder"), r"C:\nowhere\at\all").unwrap_err();
        assert!(refusal.contains("no folder"), "got: {refusal}");

        let real = std::env::temp_dir();
        assert!(read(find("journal folder"), &real.to_string_lossy()).is_ok());
    }

    #[test]
    fn a_speech_model_that_is_not_there_yet_is_still_accepted() {
        // Half a gigabyte of weights. Refusing the path because the file has
        // not been fetched yet would stop the Commander pointing at where they
        // are about to put it.
        assert!(read(find("speech model"), r"D:\models\ggml-medium.en.bin").is_ok());
    }

    #[test]
    fn a_size_that_would_hide_the_controls_is_refused() {
        // The specific trap: at a size of 40 the box needed to fix it is off
        // the edge of the window.
        assert!(read(find("text size"), "40").is_err());
        assert!(read(find("text size"), "0.1").is_err());
        assert_eq!(read(find("text size"), "1.5"), Ok(Some(json!(1.5))));
    }

    #[test]
    fn something_that_is_not_a_number_says_so_rather_than_becoming_zero() {
        let refusal = read(find("text size"), "large").unwrap_err();
        assert!(refusal.contains("number"), "got: {refusal}");
    }

    #[test]
    fn a_setting_that_gates_what_ward_will_do_is_protected() {
        // The model reads text Ward did not write. A setting it can change is
        // a setting that text can change, and this one presses a fire key.
        assert!(
            find("auto honk").protected,
            "auto honk is not protected from the model"
        );
    }

    #[test]
    fn every_card_holds_something() {
        for card in cards() {
            assert!(
                all().iter().any(|row| row.card == card),
                "{card} is an empty card"
            );
        }
    }

    #[test]
    fn a_flag_reads_the_words_a_person_would_use() {
        assert_eq!(read(find("auto honk"), "true"), Ok(Some(json!(true))));
        assert_eq!(read(find("auto honk"), "Off"), Ok(Some(json!(false))));
        assert!(read(find("auto honk"), "maybe").is_err());
    }
}
