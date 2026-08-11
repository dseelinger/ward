//! What a capability is, and how one describes itself.
//!
//! A capability declares itself **as data**, once, and is never mutated
//! afterwards. That is not tidiness: the tool schema is part of the prompt
//! prefix that gets cached, so a schema which differs by a byte between turns
//! silently stops the cache matching and every turn pays full price.
//!
//! The description methods have no default implementations. A capability that
//! cannot say what it is for does not compile, which is a better place to find
//! out than a test, and a much better place than a Commander asking what Ward
//! can do and being told about something nameless.

use std::collections::BTreeSet;

use serde_json::{Value, json};

/// One argument of one tool.
///
/// `param` is deliberately one string doing three jobs: it is the argument the
/// model fills in, the field name help advertises, and the field any downstream
/// call receives. Three separate strings would be three strings that drift, and
/// the drift shows up as a tool that is called correctly and does nothing.
pub struct Slot {
    pub param: &'static str,
    pub kind: Kind,
    pub required: bool,
    /// What this is, in a sentence, for the model and for help.
    pub help: &'static str,
    /// A real value, not a placeholder.
    pub example: &'static str,
}

/// Only `Text` is built outside tests today, because the one capability that
/// exists takes no arguments. The first capability that does will use the
/// others; recorded here rather than left as a silent warning.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Number,
    Flag,
}

impl Kind {
    fn wire(self) -> &'static str {
        match self {
            Kind::Text => "string",
            Kind::Number => "number",
            Kind::Flag => "boolean",
        }
    }
}

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub slots: &'static [Slot],
}

impl Tool {
    /// The schema as the model receives it.
    ///
    /// Ordering is taken from the declaration and never sorted, so the same
    /// declaration always produces the same bytes.
    pub fn schema(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for slot in self.slots {
            properties.insert(
                slot.param.to_string(),
                json!({
                    "type": slot.kind.wire(),
                    "description": format!("{} For example: {}", slot.help, slot.example),
                }),
            );

            if slot.required {
                required.push(Value::String(slot.param.to_string()));
            }
        }

        json!({
            "name": self.name,
            "description": self.description,
            "input_schema": {
                "type": "object",
                "properties": Value::Object(properties),
                "required": Value::Array(required),
            }
        })
    }
}

/// Everything a capability must be able to say about itself.
pub trait Capability: Send + Sync {
    /// Stable identity. Never shown to a Commander.
    fn id(&self) -> &'static str;

    /// Which part of Ward this belongs to. Groups are how spoken help stays
    /// short: a Commander hears the groups, then one group, then the detail.
    fn group(&self) -> &'static str;

    /// One sentence, in the second person, describing what it does.
    fn one_liner(&self) -> &'static str;

    /// Things a Commander might actually say.
    ///
    /// Read by spoken help, which is separate work; until then this is
    /// exercised by tests rather than by the application.
    ///
    /// These feed the model as examples and the interface as "try saying…".
    /// They are **never matched against transcribed speech**. Transcription of
    /// system, station and commodity names is exactly where recognition fails,
    /// and a phrase matcher fails silently when it misses - the Commander said
    /// the right thing, and nothing happened, with nothing to explain why.
    #[allow(dead_code)]
    fn examples(&self) -> &'static [&'static str];

    /// Tools this capability offers the model.
    ///
    /// Genuinely optional. A capability that reacts to what the game is doing,
    /// rather than to something being asked, has none at all - and that is a
    /// supported shape rather than an accident of an empty list.
    fn tools(&self) -> &'static [Tool] {
        &[]
    }

    /// Runs one of this capability's tools.
    ///
    /// Returns text for the model to read. Errors are returned as text too:
    /// the turn has to survive a tool that fails, and the model is better
    /// placed to explain a failure to a Commander than a dropped turn is.
    fn run(&self, tool: &str, input: &Value) -> String {
        let _ = input;
        format!("{} has no tool called {tool}", self.id())
    }

    /// The settings this capability contributes to the page.
    ///
    /// Declared here as well as composed in the schema, and a test holds the
    /// two together. Without it a capability can read a setting that has no
    /// row, which presents as a feature the Commander cannot switch on and a
    /// settings file that gets rejected for naming it.
    ///
    /// Read only by that test today: the page is drawn from the schema, which
    /// is the composed list. This is the other end of the pair, and marked
    /// rather than left as a silent warning.
    #[allow(dead_code)]
    fn settings(&self) -> &'static [crate::schema::Row] {
        &[]
    }

    /// What this capability currently has to show, if anything.
    ///
    /// The other half of a capability being reachable without being asked. A
    /// surface renders this rather than remembering the last answer, which is
    /// what keeps a panel from going stale until something reloads it — and it
    /// is how an edit made by voice appears on the panel, and an edit made on
    /// the panel is read back correctly by voice.
    ///
    /// Returning nothing is ordinary. Most capabilities answer a question and
    /// have nothing standing to show afterwards.
    fn display(&self) -> Option<crate::shown::Shown> {
        None
    }

    /// Something happened in the game.
    ///
    /// The other half of what a capability can be. Most of what Ward will
    /// eventually notice arrives this way rather than by being asked, and a
    /// capability that only reacts is a supported shape rather than a gap.
    ///
    /// Takes `&self`: a registry is shared and never mutable, so a capability
    /// keeping state keeps it behind its own lock. That also stops one
    /// capability's bookkeeping from becoming everybody's problem.
    fn on_event(&self, record: &crate::journal::Record) {
        let _ = record;
    }
}

/// The capabilities this build has, in declaration order.
pub struct Registry {
    capabilities: Vec<Box<dyn Capability>>,
}

impl Registry {
    pub fn new(capabilities: Vec<Box<dyn Capability>>) -> Self {
        Self { capabilities }
    }

    pub fn capabilities(&self) -> &[Box<dyn Capability>] {
        &self.capabilities
    }

    /// Every tool, in registration order.
    ///
    /// Order is part of the contract, not a detail. This array is sent on every
    /// turn ahead of the conversation, and the cache matches on a prefix of
    /// bytes; reordering it costs a cache miss on every turn thereafter.
    pub fn tools(&self) -> Vec<Value> {
        self.capabilities
            .iter()
            .flat_map(|c| c.tools().iter().map(Tool::schema))
            .collect()
    }

    /// Finds the capability owning a tool and runs it.
    ///
    /// A name nobody claims returns text rather than failing. The loop has to
    /// survive any tool call, including one for a tool that was removed between
    /// the schema being cached and the model asking for it.
    pub fn run(&self, tool: &str, input: &Value) -> String {
        for capability in &self.capabilities {
            if capability.tools().iter().any(|t| t.name == tool) {
                return capability.run(tool, input);
            }
        }

        format!("Ward has no tool called {tool}.")
    }

    /// Hands an event to every capability.
    ///
    /// All of them, always. Deciding here which capabilities care would mean
    /// this list knowing what each one is for, which is the knowledge the
    /// descriptor exists to keep in one place.
    pub fn on_event(&self, record: &crate::journal::Record) {
        for capability in &self.capabilities {
            capability.on_event(record);
        }
    }

    /// Tool names that appear more than once.
    ///
    /// Two capabilities claiming one name means the second is unreachable. The
    /// test that asserts this is empty is the consumer; nothing at runtime
    /// needs to ask, because the answer cannot change after the build.
    #[allow(dead_code)]
    pub fn duplicate_tool_names(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        let mut duplicates = BTreeSet::new();

        for capability in &self.capabilities {
            for tool in capability.tools() {
                if !seen.insert(tool.name) {
                    duplicates.insert(tool.name.to_string());
                }
            }
        }

        duplicates.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A capability with no tools at all. Exists to prove that shape is
    /// supported rather than merely untested - it is what a capability that
    /// reacts to the game will look like.
    struct Silent;

    impl Capability for Silent {
        fn id(&self) -> &'static str {
            "silent"
        }
        fn group(&self) -> &'static str {
            "test"
        }
        fn one_liner(&self) -> &'static str {
            "Does something without being asked."
        }
        fn examples(&self) -> &'static [&'static str] {
            &[]
        }
    }

    static NOISY_TOOLS: &[Tool] = &[Tool {
        name: "make_noise",
        description: "Makes a noise.",
        slots: &[
            Slot {
                param: "loudness",
                kind: Kind::Number,
                required: true,
                help: "How loud, from 1 to 10.",
                example: "7",
            },
            Slot {
                param: "reason",
                kind: Kind::Text,
                required: false,
                help: "Why.",
                example: "arrival",
            },
        ],
    }];

    struct Noisy;

    impl Capability for Noisy {
        fn id(&self) -> &'static str {
            "noisy"
        }
        fn group(&self) -> &'static str {
            "test"
        }
        fn one_liner(&self) -> &'static str {
            "Makes a noise on request."
        }
        fn examples(&self) -> &'static [&'static str] {
            &["make a noise"]
        }
        fn tools(&self) -> &'static [Tool] {
            NOISY_TOOLS
        }
        fn run(&self, tool: &str, input: &Value) -> String {
            format!("{tool} at {}", input["loudness"])
        }
    }

    fn registry() -> Registry {
        Registry::new(vec![Box::new(Silent), Box::new(Noisy)])
    }

    #[test]
    fn a_capability_with_no_tools_is_a_supported_shape() {
        let r = Registry::new(vec![Box::new(Silent)]);
        assert!(r.tools().is_empty());
        assert_eq!(r.capabilities().len(), 1);
    }

    #[test]
    fn only_capabilities_with_tools_contribute_schemas() {
        assert_eq!(registry().tools().len(), 1);
    }

    #[test]
    fn a_schema_is_byte_identical_between_builds() {
        // The schema rides in the cached prefix of every turn. A difference of
        // one byte between two turns is a cache miss on every turn after it,
        // paid for in money and in the delay before Ward starts speaking.
        let once = serde_json::to_string(&registry().tools()).unwrap();
        let twice = serde_json::to_string(&registry().tools()).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn a_schema_carries_the_slot_help_and_example() {
        let schema = &registry().tools()[0];
        let loudness = &schema["input_schema"]["properties"]["loudness"];

        assert_eq!(loudness["type"], "number");
        assert!(
            loudness["description"].as_str().unwrap().contains("7"),
            "the example should reach the model: {loudness}"
        );
        assert_eq!(schema["input_schema"]["required"], json!(["loudness"]));
    }

    #[test]
    fn a_tool_reaches_the_capability_that_declared_it() {
        let out = registry().run("make_noise", &json!({"loudness": 9}));
        assert_eq!(out, "make_noise at 9");
    }

    #[test]
    fn an_unclaimed_tool_returns_text_rather_than_failing() {
        // A turn has to survive this. The model can be asking for a tool that
        // existed when the prefix was cached and does not exist now.
        let out = registry().run("no_such_tool", &json!({}));
        assert!(out.contains("no tool called"), "got: {out}");
    }

    #[test]
    fn duplicate_tool_names_are_detectable() {
        let r = Registry::new(vec![Box::new(Noisy), Box::new(Noisy)]);
        assert_eq!(r.duplicate_tool_names(), vec!["make_noise".to_string()]);
    }
}
