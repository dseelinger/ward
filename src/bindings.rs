//! Reading the Commander's own control bindings.
//!
//! Ward presses the keys the Commander already uses. It does not have its own
//! keys, and it does not ask them to rebind anything — a companion that only
//! works if you rearrange your controls is a companion most people will not
//! use.
//!
//! The honest-reporting rule lives here. Elite lets every action be bound to a
//! joystick, a keyboard, or both, and this audience mostly flies on a stick.
//! An action bound only to a joystick **cannot** be pressed by software, and
//! saying so plainly is worth more than the feature: the alternative is a
//! Commander asking for something, hearing that it was done, and watching
//! nothing happen.

use std::path::{Path, PathBuf};

/// Where a binding came from, and whether Ward can use it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Binding {
    /// Bound to a key Ward can press.
    Key(String),
    /// Bound, but only to a device Ward cannot operate.
    ElsewhereOnly {
        /// What it is bound to, so the message can say so.
        device: String,
    },
    /// Not bound at all.
    Unbound,
}

impl Binding {
    /// What to tell the Commander when this cannot be used.
    ///
    /// Never silence. An action that does nothing without explanation is worse
    /// than one that says why.
    pub fn refusal(&self, action: &str) -> Option<String> {
        match self {
            Binding::Key(_) => None,
            Binding::ElsewhereOnly { .. } => Some(format!(
                "{action} is bound to your joystick, which Ward cannot press. \
                 Bind it to a key as well and Ward can use it."
            )),
            Binding::Unbound => Some(format!(
                "{action} is not bound to anything. Bind it in the game's controls \
                 and Ward can use it."
            )),
        }
    }
}

/// Finds the bindings file the game is currently using.
///
/// The preset is named in a marker file, and the file itself carries a version
/// in its name that changes between game updates. Globbing for the preset and
/// taking the newest is what survives an update; hardcoding a version is what
/// breaks silently the next time Frontier ships one.
pub fn active_file(options_dir: &Path) -> Option<PathBuf> {
    let marker = options_dir.join("StartPreset.4.start");
    let raw = std::fs::read_to_string(&marker).ok()?;

    // The marker repeats the preset name once per control scheme. They are
    // normally identical, and the first is the one that matters.
    let preset = raw.lines().next()?.trim().to_string();

    if preset.is_empty() {
        return None;
    }

    let mut candidates: Vec<PathBuf> = std::fs::read_dir(options_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                return false;
            };
            // A backup is a copy of a file, not a file the game is reading.
            name.starts_with(&format!("{preset}.")) && name.ends_with(".binds")
        })
        .collect();

    candidates.sort();
    candidates.pop()
}

/// Reads which key an action is bound to.
///
/// Primary is preferred, and secondary is consulted when primary is on a device
/// Ward cannot use — which is the common case for anybody flying on a stick,
/// and exactly the case a naive reading gets wrong by looking only at primary,
/// finding a joystick, and giving up on a key that was there all along.
pub fn lookup(xml: &str, action: &str) -> Binding {
    let Ok(document) = roxmltree::Document::parse(xml) else {
        return Binding::Unbound;
    };

    let Some(node) = document
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == action)
    else {
        return Binding::Unbound;
    };

    let mut elsewhere: Option<String> = None;

    for slot in ["Primary", "Secondary"] {
        let Some(binding) = node.children().find(|c| c.tag_name().name() == slot) else {
            continue;
        };

        let device = binding.attribute("Device").unwrap_or_default();
        let key = binding.attribute("Key").unwrap_or_default();

        if key.is_empty() || device == "{NoDevice}" {
            continue;
        }

        if device == "Keyboard" {
            return Binding::Key(key.to_string());
        }

        elsewhere.get_or_insert_with(|| device.to_string());
    }

    match elsewhere {
        Some(device) => Binding::ElsewhereOnly { device },
        None => Binding::Unbound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BINDS: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<Root PresetName="Custom" MajorVersion="4" MinorVersion="2">
    <PrimaryFire>
        <Primary Device="4098BEA1" Key="Joy_9" />
        <Secondary Device="Keyboard" Key="Key_F" />
    </PrimaryFire>
    <ExplorationFSSEnter>
        <Primary Device="Keyboard" Key="Key_Apostrophe" />
        <Secondary Device="4098BD65" Key="Joy_17" />
    </ExplorationFSSEnter>
    <StickOnly>
        <Primary Device="4098BEA1" Key="Joy_3" />
        <Secondary Device="{NoDevice}" Key="" />
    </StickOnly>
    <NeverBound>
        <Primary Device="{NoDevice}" Key="" />
        <Secondary Device="{NoDevice}" Key="" />
    </NeverBound>
</Root>"#;

    #[test]
    fn a_keyboard_secondary_is_found_behind_a_joystick_primary() {
        // The case that matters for this audience, taken from a real bindings
        // file: firing is on the stick, with a key as the backup. Reading only
        // the primary would find a joystick and conclude nothing can be done,
        // while the key sat there unused.
        assert_eq!(
            lookup(BINDS, "PrimaryFire"),
            Binding::Key("Key_F".to_string())
        );
    }

    #[test]
    fn a_keyboard_primary_is_used_directly() {
        assert_eq!(
            lookup(BINDS, "ExplorationFSSEnter"),
            Binding::Key("Key_Apostrophe".to_string())
        );
    }

    #[test]
    fn an_action_only_on_the_stick_says_so_rather_than_doing_nothing() {
        let binding = lookup(BINDS, "StickOnly");

        assert!(matches!(binding, Binding::ElsewhereOnly { .. }));

        let refusal = binding.refusal("Landing gear").unwrap();
        assert!(refusal.contains("joystick"), "got: {refusal}");
        assert!(refusal.contains("Bind it to a key"), "got: {refusal}");
    }

    #[test]
    fn an_unbound_action_says_that_instead() {
        let binding = lookup(BINDS, "NeverBound");
        assert_eq!(binding, Binding::Unbound);

        let refusal = binding.refusal("Cargo scoop").unwrap();
        assert!(refusal.contains("not bound"), "got: {refusal}");
    }

    #[test]
    fn an_action_the_game_does_not_have_is_unbound() {
        assert_eq!(lookup(BINDS, "InventedAction"), Binding::Unbound);
    }

    #[test]
    fn a_usable_binding_has_nothing_to_apologize_for() {
        assert!(lookup(BINDS, "PrimaryFire").refusal("Fire").is_none());
    }

    #[test]
    fn nonsense_in_place_of_a_bindings_file_is_not_a_crash() {
        assert_eq!(lookup("<not xml", "PrimaryFire"), Binding::Unbound);
        assert_eq!(lookup("", "PrimaryFire"), Binding::Unbound);
    }

    #[test]
    fn the_active_preset_is_read_from_the_marker() {
        let dir = std::env::temp_dir().join("ward-bindings-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // The marker repeats the name once per control scheme.
        std::fs::write(
            dir.join("StartPreset.4.start"),
            "Custom\nCustom\nCustom\nCustom",
        )
        .unwrap();

        // A version in the filename that changes between game updates, plus
        // backups the game is not reading.
        std::fs::write(dir.join("Custom.4.2.binds"), BINDS).unwrap();
        std::fs::write(dir.join("Custom.4.2.binds.1927777385.backup"), "old").unwrap();
        std::fs::write(dir.join("Other.4.2.binds"), "not the active preset").unwrap();

        let found = active_file(&dir).unwrap();
        assert_eq!(found.file_name().unwrap(), "Custom.4.2.binds");
    }

    #[test]
    fn a_missing_marker_is_not_a_crash() {
        let dir = std::env::temp_dir().join("ward-bindings-absent");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(active_file(&dir).is_none());
    }
}
