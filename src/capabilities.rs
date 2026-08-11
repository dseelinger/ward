//! The single composition point.
//!
//! Every capability this build has is in the one list below. Adding a
//! capability is adding a line here and nothing else — no discovery, no
//! attribute, nothing that scans a directory. One list means the whole
//! capability surface is visible at once and greppable, and it means a
//! capability that was written but never wired is a thing you can see rather
//! than a thing you find out about later.
//!
//! It was composed in three places in an earlier attempt at this, and the
//! consequence was exact: the test whose job was to notice an unwired
//! capability went on checking a registry that was not the application's.

use serde_json::Value;

use crate::capability::{Capability, Registry, Tool};

/// Ordered on purpose. The tool schemas go out ahead of the conversation on
/// every turn and are matched as a prefix of bytes, so this order is part of
/// what makes the cache hit.
pub fn registry(settings: &crate::config::Settings, data_dir: &std::path::Path) -> Registry {
    let honk = crate::honk::Honk::new(
        settings.flag("auto honk"),
        read_fire_binding(&settings.string("bindings folder")),
    );

    let checklist = crate::checklist::Checklist::load(&data_dir.join("checklist.md"));

    Registry::new(vec![Box::new(Version), Box::new(honk), Box::new(checklist)])
}

/// Reads the Commander's fire binding once, at startup.
fn read_fire_binding(folder: &str) -> crate::bindings::Binding {
    let Some(file) = crate::bindings::active_file(std::path::Path::new(folder)) else {
        return crate::bindings::Binding::Unbound;
    };

    match std::fs::read_to_string(&file) {
        Ok(xml) => crate::bindings::lookup(&xml, "PrimaryFire"),
        Err(_) => crate::bindings::Binding::Unbound,
    }
}

// --- version -----------------------------------------------------------------

static VERSION_TOOLS: &[Tool] = &[Tool {
    name: "ward_version",
    description: "Reports which version of Ward is running. \
                  Use this when the Commander asks what version they have, \
                  whether they are up to date, or which build this is.",
    slots: &[],
}];

/// Answers what build this is.
///
/// Chosen as the first capability because it depends on nothing. It needs no
/// game running, no network, no key beyond the one already needed to talk at
/// all, and it cannot be wrong: the answer is compiled in. That makes it a
/// clean test of the path itself rather than of anything it touches.
struct Version;

impl Capability for Version {
    fn id(&self) -> &'static str {
        "version"
    }

    fn group(&self) -> &'static str {
        "About Ward"
    }

    fn one_liner(&self) -> &'static str {
        "Tells you which version of Ward you are running."
    }

    fn examples(&self) -> &'static [&'static str] {
        &[
            "what version are you",
            "which build is this",
            "are you up to date",
        ]
    }

    fn tools(&self) -> &'static [Tool] {
        VERSION_TOOLS
    }

    fn run(&self, _tool: &str, _input: &Value) -> String {
        format!("Ward {}", env!("CARGO_PKG_VERSION"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_registry() -> Registry {
        // Somewhere disposable: composing the registry loads the Commander's
        // own files, and a test must not read or write the real ones.
        let dir = std::env::temp_dir().join("ward-registry-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        registry(&crate::config::Settings::default(), &dir)
    }

    #[test]
    fn the_registry_is_not_empty() {
        // A test that discovers things and finds none of them passes, which is
        // the one failure a registry test exists to catch. Assert the
        // enumeration itself, not only what is in it.
        assert!(
            !test_registry().capabilities().is_empty(),
            "no capability is wired up"
        );
    }

    #[test]
    fn every_capability_describes_itself() {
        for capability in test_registry().capabilities() {
            let id = capability.id();
            assert!(!capability.group().is_empty(), "{id} has no group");
            assert!(!capability.one_liner().is_empty(), "{id} has no one-liner");
        }
    }

    #[test]
    fn every_tool_is_described_and_named_once() {
        let r = test_registry();

        assert!(
            r.duplicate_tool_names().is_empty(),
            "two capabilities claim the same tool: {:?}",
            r.duplicate_tool_names()
        );

        for schema in r.tools() {
            let name = schema["name"].as_str().unwrap();
            let description = schema["description"].as_str().unwrap_or_default();
            assert!(
                description.len() > 20,
                "{name} needs a description the model can act on, got {description:?}"
            );
        }
    }

    #[test]
    fn a_capability_with_examples_offers_something_sayable() {
        // Examples feed the model and the interface. They are never matched
        // against transcribed speech, so their job is to be natural rather
        // than exhaustive.
        for capability in test_registry().capabilities() {
            if capability.tools().is_empty() {
                continue;
            }
            assert!(
                !capability.examples().is_empty(),
                "{} offers a tool but nothing a Commander might say",
                capability.id()
            );
        }
    }

    #[test]
    fn the_version_tool_reports_the_real_version() {
        let answer = test_registry().run("ward_version", &json!({}));
        assert_eq!(answer, format!("Ward {}", env!("CARGO_PKG_VERSION")));
        assert!(answer.contains('.'), "should look like a version: {answer}");
    }

    #[test]
    fn every_setting_a_capability_contributes_reaches_the_page() {
        // What this catches, precisely: a capability declaring rows that the
        // schema was never told to compose. It cannot catch the two disagreeing
        // about a key, because they are the same static - which is the point of
        // it being a static, and the reason this test asserts the weaker thing
        // rather than pretending to assert the stronger one.
        //
        // The failure is worth catching. An uncomposed row is a feature the
        // Commander cannot switch on, and a settings file rejected for naming
        // the setting the feature reads.
        let page: Vec<&str> = crate::schema::all().iter().map(|row| row.key).collect();

        for capability in test_registry().capabilities() {
            for row in capability.settings() {
                assert!(
                    page.contains(&row.key),
                    "{} contributes \"{}\", and it is not on the settings page",
                    capability.id(),
                    row.key
                );
            }
        }
    }

    // --- documentation -------------------------------------------------------
    //
    // A capability is not complete until its page exists, and these are what
    // make that true rather than merely encouraged. They run against the real
    // registry - the application's own, not a second list - so a capability
    // wired up without a page fails the build that wired it up.
    //
    // What they cannot check is whether the prose is any good. That is a
    // person's job, and it is deliberately not a gate: a review that blocks a
    // merge turns writing into paperwork. These check the facts underneath it.

    /// Where a capability's page lives, derived from its id.
    fn page_of(id: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs/capabilities")
            .join(format!("{id}.md"))
    }

    fn pages() -> Vec<(String, String)> {
        test_registry()
            .capabilities()
            .iter()
            .map(|c| {
                let path = page_of(c.id());
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                (c.id().to_string(), text)
            })
            .collect()
    }

    #[test]
    fn every_capability_has_a_page() {
        let registry = test_registry();

        // A discovery test that finds nothing passes, which is the one failure
        // this whole family exists to catch.
        assert!(!registry.capabilities().is_empty());

        for capability in registry.capabilities() {
            let path = page_of(capability.id());
            assert!(
                path.is_file(),
                "{} has no documentation page. Write {}.",
                capability.id(),
                path.display()
            );
        }
    }

    #[test]
    fn every_page_belongs_to_a_capability() {
        // The other direction. A page for something that was renamed or removed
        // is a page nobody will notice is lying.
        let registry = test_registry();
        let known: Vec<&str> = registry.capabilities().iter().map(|c| c.id()).collect();

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/capabilities");
        let entries = std::fs::read_dir(&dir).expect("no docs/capabilities directory");

        let mut found = 0;

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();

            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }

            found += 1;
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();

            assert!(
                known.contains(&stem.as_ref()),
                "{} documents no registered capability",
                path.display()
            );
        }

        assert!(found > 0, "no capability pages were found at all");
    }

    #[test]
    fn every_page_quotes_something_real() {
        // A mechanical stand-in for a rule that cannot be checked directly:
        // write from artifacts, never from the feature name. A page that cannot
        // quote real output, a real setting or a real event is a page written
        // from an idea of the feature rather than from the feature.
        for (id, text) in pages() {
            assert!(
                text.contains("```") || text.contains("\n> "),
                "{id}'s page quotes nothing real - no code block and no output"
            );
        }
    }

    /// Reads the declaration blocks at the foot of a page.
    ///
    /// `<!-- default: auto honk = false -->` is a claim the page makes, in a
    /// form that can be checked against the code that actually ships it.
    fn declared_defaults(text: &str) -> Vec<(String, String)> {
        text.lines()
            .filter_map(|line| {
                let inner = line
                    .trim()
                    .strip_prefix("<!-- default:")?
                    .strip_suffix("-->")?;
                let (key, value) = inner.split_once('=')?;
                Some((key.trim().to_string(), value.trim().to_string()))
            })
            .collect()
    }

    #[test]
    fn documented_defaults_match_the_code() {
        // Four settings were once documented with the wrong default, two of
        // them in prose, including one described as opt-in on every page while
        // shipping switched on.
        let mut checked = 0;

        for (id, text) in pages() {
            for (key, claimed) in declared_defaults(&text) {
                let Some(actual) = crate::config::default_setting(&key) else {
                    panic!("{id}'s page declares a default for \"{key}\", which is not a setting");
                };

                assert_eq!(
                    actual.to_string(),
                    claimed,
                    "{id}'s page says \"{key}\" defaults to {claimed}, and it ships as {actual}"
                );

                checked += 1;
            }
        }

        assert!(
            checked > 0,
            "no page declared a default, so nothing was checked"
        );
    }

    #[test]
    fn a_page_that_claims_a_default_declares_it() {
        // Otherwise the check above is trivially satisfied by never writing a
        // declaration block, and the prose goes back to being unverifiable.
        for (id, text) in pages() {
            let claims = [
                "by default",
                "defaults to",
                "default is",
                "unless you turn it on",
            ]
            .iter()
            .any(|phrase| text.to_lowercase().contains(phrase));

            if claims {
                assert!(
                    !declared_defaults(&text).is_empty(),
                    "{id}'s page talks about a default without declaring one, \
                     so nothing checks the claim. Add a line like \
                     <!-- default: auto honk = false --> at the foot of it."
                );
            }
        }
    }
}
