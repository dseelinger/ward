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
pub fn registry(settings: &crate::config::Settings) -> Registry {
    let honk = crate::honk::Honk::new(
        settings.flag("auto honk"),
        read_fire_binding(&settings.string("bindings folder")),
    );

    Registry::new(vec![Box::new(Version), Box::new(honk)])
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
        registry(&crate::config::Settings::default())
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
}
