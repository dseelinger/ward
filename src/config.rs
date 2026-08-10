//! Configuration, in two stores.
//!
//! One loader cannot both fail loudly and shrug, which is why there are two.
//!
//! - [`Settings`] is what the Commander chose. A key it does not recognize is a
//!   **fault**, and the whole file is rejected rather than partly applied. A
//!   typo in a settings file is a Commander who believes something is set and
//!   is about to be surprised in the cockpit; silently dropping the line makes
//!   that surprise arrive later and with no explanation.
//! - [`State`] is where the window was. Unknown keys are **ignored**. Nobody
//!   should be unable to start Ward because a remembered window position from a
//!   newer build means nothing to this one.
//!
//! **Defaults live in code, and only what was changed is written.** No
//! configuration file ships, so there is no seeded copy to go stale, nothing to
//! migrate, and no possibility of a shipped file quietly disagreeing with the
//! code that reads it. Setting a value back to its default removes it from the
//! file rather than writing the default in, so the file always says exactly
//! what the Commander changed and nothing else.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

/// Everything writable lives in one folder beside the executable: one thing to
/// delete for a genuine first run, one thing to copy, one thing an installer
/// has to preserve.
///
/// The override exists so a test can point somewhere disposable. It is not a
/// way to relocate a real install, and secrets deliberately have no equivalent:
/// an environment variable holding a key would be a plaintext bypass.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("WARD_DATA_DIR") {
        return PathBuf::from(dir);
    }

    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("data")))
        .unwrap_or_else(|| PathBuf::from("data"))
}

/// The single registry of what a setting is called and what it defaults to.
///
/// Deliberately one list rather than a list of names beside a table of
/// defaults. Two declarations that have to agree are two declarations that
/// eventually do not, and the failure here would be a correct settings file
/// rejected as unrecognized.
///
/// Keys are spelled the way a person would say them out loud, because the same
/// string is the name in the settings page, the name in help, and the name in
/// the message about a rejected file.
fn default_setting(key: &str) -> Option<Value> {
    match key {
        "model" => Some(json!(crate::anthropic::DEFAULT_MODEL)),
        // Chosen to be readable on a high-resolution display without anyone
        // adjusting it. Multiplies the display's own scale factor rather than
        // replacing it.
        "text size" => Some(json!(1.35)),
        "voice" => Some(json!(crate::voice::DEFAULT_VOICE)),
        // Percentage against the voice's natural pace, in the form the service
        // expects. Never a bare multiplier: providers disagree about what
        // range they accept, and a number that suits one clips against another.
        "speaking rate" => Some(json!("+0%")),
        _ => None,
    }
}

/// What the Commander chose. Holds overrides only.
#[derive(Debug, Default, Clone)]
pub struct Settings {
    overrides: BTreeMap<String, Value>,
}

impl Settings {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("settings.json")
    }

    /// Reads the file. A missing file is an ordinary first run.
    ///
    /// An unrecognized key rejects the whole file, naming the key.
    pub fn load(path: &Path) -> Result<Self> {
        let Some(map) = read_object(path)? else {
            return Ok(Self::default());
        };

        for key in map.keys() {
            if default_setting(key).is_none() {
                return Err(anyhow!(
                    "{} contains a setting Ward does not recognize: \"{key}\". \
                     Nothing was applied. Correct or remove the line.",
                    path.display()
                ));
            }
        }

        Ok(Self {
            overrides: map.into_iter().collect(),
        })
    }

    /// Nothing in the application changes a setting yet - the settings page is
    /// separate work - so the only callers today are the tests that prove a
    /// value survives a restart. Recorded here rather than left as a silent
    /// warning.
    #[allow(dead_code)]
    pub fn save(&self, path: &Path) -> Result<()> {
        write_object(path, &self.overrides)
    }

    /// The chosen value, or the built-in default.
    pub fn get(&self, key: &str) -> Value {
        self.overrides
            .get(key)
            .cloned()
            .or_else(|| default_setting(key))
            .unwrap_or(Value::Null)
    }

    pub fn f32(&self, key: &str, floor: f32, ceiling: f32) -> f32 {
        let raw = self.get(key).as_f64().unwrap_or_default() as f32;

        // A value outside the sane range came from somewhere other than the UI,
        // and honoring it would render Ward unusable and unfixable - the
        // controls needed to correct it would be the ones off screen.
        raw.clamp(floor, ceiling)
    }

    pub fn string(&self, key: &str) -> String {
        self.get(key).as_str().unwrap_or_default().to_string()
    }

    /// Sets a value, or clears it back to the default.
    ///
    /// Assigning the default *is* clearing: the override is removed rather than
    /// written, so the file only ever contains genuine changes.
    #[allow(dead_code)]
    pub fn set(&mut self, key: &str, value: Value) {
        if default_setting(key).as_ref() == Some(&value) {
            self.overrides.remove(key);
        } else {
            self.overrides.insert(key.to_string(), value);
        }
    }

    #[allow(dead_code)]
    pub fn clear(&mut self, key: &str) {
        self.overrides.remove(key);
    }

    /// How many settings differ from their defaults.
    pub fn changed(&self) -> usize {
        self.overrides.len()
    }
}

/// Where the window was. Advisory, and never a reason to fail.
#[derive(Debug, Default, Clone)]
pub struct State {
    values: BTreeMap<String, Value>,
}

impl State {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("state.json")
    }

    /// Unknown keys are kept but never questioned. A file that will not parse
    /// at all is discarded and reported, not propagated: losing a window
    /// position is not worth refusing to start over.
    pub fn load(path: &Path) -> Self {
        match read_object(path) {
            Ok(Some(map)) => Self {
                values: map.into_iter().collect(),
            },
            Ok(None) => Self::default(),
            Err(e) => {
                tracing::warn!(
                    target: "ward::config",
                    error = %e,
                    "running state unreadable, starting from defaults"
                );
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_object(path, &self.values)
    }

    pub fn f32(&self, key: &str) -> Option<f32> {
        self.values.get(key)?.as_f64().map(|v| v as f32)
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.values.insert(key.to_string(), value);
    }
}

fn read_object(path: &Path) -> Result<Option<serde_json::Map<String, Value>>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("could not read {}: {e}", path.display())),
    };

    if raw.trim().is_empty() {
        return Ok(None);
    }

    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;

    match value {
        Value::Object(map) => Ok(Some(map)),
        _ => Err(anyhow!(
            "{} should contain a single object of settings",
            path.display()
        )),
    }
}

/// Writes through a `.writing` sibling, then moves it into place.
///
/// A sibling in the same directory rather than a temporary folder, because a
/// move across volumes is a copy and a delete, and a copy can be interrupted
/// halfway. Within one directory the move is atomic, so a reader sees either
/// the old file or the new one - never the half of the new one that had been
/// flushed when the power went.
fn write_object(path: &Path, values: &BTreeMap<String, Value>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }

    let map: serde_json::Map<String, Value> =
        values.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut body = serde_json::to_string_pretty(&Value::Object(map))?;
    body.push('\n');

    let mut writing = path.as_os_str().to_owned();
    writing.push(".writing");
    let writing = PathBuf::from(writing);

    std::fs::write(&writing, body)
        .with_context(|| format!("could not write {}", writing.display()))?;

    std::fs::rename(&writing, path).with_context(|| {
        format!(
            "could not move {} into place at {}",
            writing.display(),
            path.display()
        )
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("ward-config-test-{name}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_setting_is_recognized_only_if_it_has_a_default() {
        assert!(default_setting("model").is_some());
        assert!(default_setting("text size").is_some());
        assert!(default_setting("a key nobody declared").is_none());
    }

    #[test]
    fn a_missing_file_is_an_ordinary_first_run() {
        let dir = temp("absent");
        let s = Settings::load(&Settings::path(&dir)).unwrap();
        assert_eq!(s.changed(), 0);
        assert_eq!(s.string("model"), crate::anthropic::DEFAULT_MODEL);
    }

    #[test]
    fn an_unrecognized_setting_rejects_the_whole_file() {
        let dir = temp("strict");
        let path = Settings::path(&dir);
        std::fs::write(&path, r#"{"model": "claude-opus-5", "txt size": 2.0}"#).unwrap();

        let err = Settings::load(&path).unwrap_err().to_string();

        assert!(err.contains("txt size"), "should name the key: {err}");
        assert!(err.contains("Nothing was applied"), "got: {err}");
    }

    #[test]
    fn an_unrecognized_running_state_key_is_ignored() {
        let dir = temp("lenient");
        let path = State::path(&dir);
        std::fs::write(&path, r#"{"window width": 900.0, "from a newer build": 7}"#).unwrap();

        // The opposite policy to settings, deliberately: a remembered window
        // position from a newer build must never stop Ward starting.
        let state = State::load(&path);
        assert_eq!(state.f32("window width"), Some(900.0));
    }

    #[test]
    fn unreadable_running_state_still_starts() {
        let dir = temp("corrupt-state");
        let path = State::path(&dir);
        std::fs::write(&path, "{ this is not json").unwrap();

        assert_eq!(State::load(&path).f32("window width"), None);
    }

    #[test]
    fn only_changed_settings_reach_the_file() {
        let dir = temp("overrides");
        let path = Settings::path(&dir);

        let mut s = Settings::default();
        s.set("text size", json!(2.0));
        // Assigning the default is clearing, so this must not be written.
        s.set("model", json!(crate::anthropic::DEFAULT_MODEL));
        s.save(&path).unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("text size"), "got: {written}");
        assert!(!written.contains("model"), "default was written: {written}");
    }

    #[test]
    fn clearing_returns_a_setting_to_its_default() {
        let mut s = Settings::default();
        s.set("text size", json!(2.5));
        assert_eq!(s.f32("text size", 0.5, 4.0), 2.5);

        s.clear("text size");
        assert_eq!(s.f32("text size", 0.5, 4.0), 1.35);
        assert_eq!(s.changed(), 0);
    }

    #[test]
    fn a_setting_survives_a_restart() {
        let dir = temp("roundtrip");
        let path = Settings::path(&dir);

        let mut s = Settings::default();
        s.set("text size", json!(1.8));
        s.save(&path).unwrap();

        let reloaded = Settings::load(&path).unwrap();
        assert_eq!(reloaded.f32("text size", 0.5, 4.0), 1.8);
    }

    #[test]
    fn an_absurd_value_is_brought_back_into_range() {
        // A size of 40 would put one word on screen, and the control needed to
        // fix it would be off the edge of it.
        let mut s = Settings::default();
        s.set("text size", json!(40.0));
        assert_eq!(s.f32("text size", 0.5, 4.0), 4.0);
    }

    #[test]
    fn a_remembered_window_survives_a_restart() {
        let dir = temp("state-roundtrip");
        let path = State::path(&dir);

        let mut state = State::default();
        state.set("window width", json!(1234.0));
        state.set("window height", json!(567.0));
        state.save(&path).unwrap();

        let reloaded = State::load(&path);
        assert_eq!(reloaded.f32("window width"), Some(1234.0));
        assert_eq!(reloaded.f32("window height"), Some(567.0));
    }

    #[test]
    fn writing_leaves_no_partial_file_behind() {
        let dir = temp("durable");
        let path = Settings::path(&dir);

        let mut s = Settings::default();
        s.set("text size", json!(1.5));
        s.save(&path).unwrap();
        s.set("text size", json!(1.6));
        s.save(&path).unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("writing"))
            .collect();

        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
        assert_eq!(
            Settings::load(&path).unwrap().f32("text size", 0.5, 4.0),
            1.6
        );
    }

    #[test]
    fn the_temporary_file_is_a_sibling_of_the_real_one() {
        // Not a temp folder: a move across volumes is a copy and a delete, and
        // a copy can be interrupted halfway. This is what makes the move atomic.
        let dir = temp("sibling");
        let path = Settings::path(&dir);

        let mut writing = path.as_os_str().to_owned();
        writing.push(".writing");

        assert_eq!(PathBuf::from(writing).parent(), path.parent());
    }
}
