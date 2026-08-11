//! A list of things to do, kept between sessions.
//!
//! The Commander writes it and the Commander finishes it. Ward never decides
//! on its own that an item is done — the journal proposing completion, and
//! asking, is separate work and deliberately not here. A checklist that ticks
//! itself is a checklist you have to check.
//!
//! **Completing an item keeps it.** A list that only shows what is left cannot
//! answer how far along you are, and how far along you are is the thing a
//! spoken checklist is actually good at.
//!
//! ## One entrance, taken by both hands
//!
//! The panel does not edit the list directly. Ticking a box runs the same tool
//! the model runs, through the same registry, with the same words. That is not
//! neatness: it means the interface cannot offer an edit the tool would refuse,
//! and it means the panel and the voice cannot drift into two sets of rules
//! about what an edit is.
//!
//! Which leaves one hazard, and it is the one this file locks against: a panel
//! edit and a voice edit arriving at once. Read, change, write must happen
//! without another writer in the middle, or a create is lost or a delete
//! resurrects. Every change below holds the lock across all three.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capability::{Capability, Kind, Slot, Tool};
use crate::shown::{Row, Shown};

/// What the Commander sees this list called, everywhere it appears.
const TITLE: &str = "Checklist";

/// A cap on the list, so a stuck loop or a misheard instruction repeated forty
/// times is bounded trouble rather than a file that grows until something else
/// notices.
const MOST_ITEMS: usize = 200;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Item {
    text: String,
    #[serde(default)]
    done: bool,
}

/// Which item a spoken phrase meant.
///
/// Three outcomes and not two. Transcription of a half-remembered phrase is
/// exactly where a single best guess goes wrong, and marking the wrong item
/// done is a silent failure — the Commander hears "done" and believes it.
#[derive(Debug, PartialEq)]
enum Match {
    One(usize),
    None,
    /// More than one item could be meant, so none is chosen.
    Several(Vec<String>),
}

/// Reduces a phrase to what it has in common with how somebody would say it.
///
/// Case and punctuation are the model's choices, not the Commander's. "Buy
/// tritium." and "buy tritium" are the same instruction.
fn plain(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Finds the one item a phrase means, or refuses to choose.
fn find(items: &[Item], phrase: &str) -> Match {
    let needle = plain(phrase);

    if needle.is_empty() {
        return Match::None;
    }

    // An exact phrase wins outright, even if it is also contained in others.
    // Otherwise "refuel" could never be completed once "refuel at Jameson"
    // existed.
    if let Some(at) = items.iter().position(|item| plain(&item.text) == needle) {
        return Match::One(at);
    }

    // Containment in either direction, because the Commander may say less than
    // they wrote down or more.
    let hits: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            let text = plain(&item.text);
            text.contains(&needle) || needle.contains(&text)
        })
        .map(|(at, _)| at)
        .collect();

    match hits.len() {
        0 => Match::None,
        1 => Match::One(hits[0]),
        _ => Match::Several(hits.iter().map(|at| items[*at].text.clone()).collect()),
    }
}

/// The list, and the only thing allowed to change it.
pub struct Checklist {
    path: PathBuf,
    items: Mutex<Vec<Item>>,
}

static TOOLS: &[Tool] = &[
    Tool {
        name: "checklist_add",
        description: "Adds an item to the Commander's checklist. \
                      Use this when they say to remember, note down, or add \
                      something they intend to do.",
        slots: &[Slot {
            param: "item",
            kind: Kind::Text,
            required: true,
            help: "The thing to do, in the Commander's own words.",
            example: "buy tritium at Deciat",
        }],
    },
    Tool {
        name: "checklist_complete",
        description: "Marks an item on the checklist as done. The item stays on \
                      the list, ticked, so progress can still be read back. \
                      Use this when the Commander says they have finished one.",
        slots: &[Slot {
            param: "item",
            kind: Kind::Text,
            required: true,
            help: "Enough of the item to identify it. Part of it is enough.",
            example: "tritium",
        }],
    },
    Tool {
        name: "checklist_remove",
        description: "Takes an item off the checklist entirely, as though it \
                      had never been added. Use this when the Commander says \
                      to delete or forget one, not when they have done it.",
        slots: &[Slot {
            param: "item",
            kind: Kind::Text,
            required: true,
            help: "Enough of the item to identify it. Part of it is enough.",
            example: "tritium",
        }],
    },
    Tool {
        name: "checklist_read",
        description: "Reads back the Commander's checklist: how many are done \
                      and what is still left. Use this when they ask what is on \
                      the list or what they still have to do.",
        slots: &[],
    },
];

impl Checklist {
    /// Loads the list, or starts an empty one.
    ///
    /// A file that will not parse is **moved aside rather than replaced**. It
    /// is the Commander's own writing, and the alternative is that the first
    /// item added afterwards overwrites whatever was in there.
    pub fn load(path: &Path) -> Self {
        let items = match std::fs::read_to_string(path) {
            Err(_) => Vec::new(),

            Ok(raw) if raw.trim().is_empty() => Vec::new(),

            Ok(raw) => match serde_json::from_str::<Vec<Item>>(&raw) {
                Ok(items) => items,
                Err(e) => {
                    let aside = path.with_extension("json.unreadable");

                    match std::fs::rename(path, &aside) {
                        Ok(()) => tracing::warn!(
                            target: "ward::checklist",
                            error = %e,
                            kept = %aside.display(),
                            "the checklist could not be read and was kept aside"
                        ),
                        Err(moving) => tracing::error!(
                            target: "ward::checklist",
                            error = %e,
                            moving = %moving,
                            "the checklist could not be read or moved aside"
                        ),
                    }

                    Vec::new()
                }
            },
        };

        tracing::info!(target: "ward::checklist", items = items.len(), "loaded");

        Self {
            path: path.to_path_buf(),
            items: Mutex::new(items),
        }
    }

    /// Writes the list out. Called with the lock already held.
    ///
    /// Returns what to append to the Commander's answer, which is nothing when
    /// it worked. A checklist that quietly stops saving is one the Commander
    /// keeps using and then loses.
    fn save(&self, items: &[Item]) -> String {
        let body = match serde_json::to_string_pretty(items) {
            Ok(body) => body,
            Err(e) => {
                tracing::error!(target: "ward::checklist", error = %e, "could not write the checklist");
                return " I could not save the list, so this will not survive a restart.".into();
            }
        };

        match crate::config::write_atomically(&self.path, &format!("{body}\n")) {
            Ok(()) => String::new(),
            Err(e) => {
                tracing::error!(target: "ward::checklist", error = %e, "could not save the checklist");
                " I could not save the list, so this will not survive a restart.".into()
            }
        }
    }

    /// Everything on the list right now.
    pub fn shown(&self) -> Shown {
        let rows = match self.items.lock() {
            Ok(items) => items
                .iter()
                .map(|item| Row {
                    text: item.text.clone(),
                    done: item.done,
                })
                .collect(),
            Err(poisoned) => poisoned
                .into_inner()
                .iter()
                .map(|item| Row {
                    text: item.text.clone(),
                    done: item.done,
                })
                .collect(),
        };

        Shown::List {
            title: TITLE.to_string(),
            rows,
        }
    }

    fn add(&self, text: &str) -> String {
        let text = text.trim();

        if text.is_empty() {
            return "There was nothing to add.".to_string();
        }

        let mut items = match self.items.lock() {
            Ok(items) => items,
            Err(poisoned) => poisoned.into_inner(),
        };

        if items.len() >= MOST_ITEMS {
            return format!("The checklist is full at {MOST_ITEMS} items.");
        }

        // Said twice is meant once. Adding it again would leave the Commander
        // completing one copy and reading back the other.
        if let Match::One(at) = find(&items, text)
            && plain(&items[at].text) == plain(text)
        {
            return format!("\"{}\" is already on the list.", items[at].text);
        }

        items.push(Item {
            text: text.to_string(),
            done: false,
        });

        let trouble = self.save(&items);

        format!("Added \"{text}\" to the checklist.{trouble}")
    }

    fn complete(&self, phrase: &str) -> String {
        let mut items = match self.items.lock() {
            Ok(items) => items,
            Err(poisoned) => poisoned.into_inner(),
        };

        match find(&items, phrase) {
            Match::None => nothing_like(phrase, &items),
            Match::Several(names) => ambiguous(phrase, &names),
            Match::One(at) if items[at].done => {
                format!("\"{}\" was already done.", items[at].text)
            }
            Match::One(at) => {
                items[at].done = true;
                let text = items[at].text.clone();

                let left = items.iter().filter(|item| !item.done).count();
                let trouble = self.save(&items);

                match left {
                    0 => format!("\"{text}\" done. That is the whole list.{trouble}"),
                    1 => format!("\"{text}\" done. One left.{trouble}"),
                    n => format!("\"{text}\" done. {n} left.{trouble}"),
                }
            }
        }
    }

    fn remove(&self, phrase: &str) -> String {
        let mut items = match self.items.lock() {
            Ok(items) => items,
            Err(poisoned) => poisoned.into_inner(),
        };

        match find(&items, phrase) {
            Match::None => nothing_like(phrase, &items),
            Match::Several(names) => ambiguous(phrase, &names),
            Match::One(at) => {
                let removed = items.remove(at);
                let trouble = self.save(&items);

                format!("Took \"{}\" off the checklist.{trouble}", removed.text)
            }
        }
    }
}

/// Nothing matched, so say what is there rather than only that it is not.
fn nothing_like(phrase: &str, items: &[Item]) -> String {
    if items.is_empty() {
        return "The checklist is empty.".to_string();
    }

    let names: Vec<&str> = items.iter().map(|item| item.text.as_str()).collect();

    format!(
        "Nothing on the checklist matches \"{phrase}\". It has: {}.",
        names.join(", ")
    )
}

/// Two things could have been meant, so neither is chosen.
fn ambiguous(phrase: &str, names: &[String]) -> String {
    format!("\"{phrase}\" could mean {}. Which one?", names.join(" or "))
}

impl Capability for Checklist {
    fn id(&self) -> &'static str {
        "checklist"
    }

    fn group(&self) -> &'static str {
        "Keeping track"
    }

    fn one_liner(&self) -> &'static str {
        "Keeps a list of things you mean to do, and reads back how far along you are."
    }

    fn examples(&self) -> &'static [&'static str] {
        &[
            "add buy tritium to my checklist",
            "what is left on my list",
            "mark the tritium done",
            "take refuelling off the list",
        ]
    }

    fn tools(&self) -> &'static [Tool] {
        TOOLS
    }

    fn display(&self) -> Option<Shown> {
        Some(self.shown())
    }

    fn run(&self, tool: &str, input: &Value) -> String {
        let item = input["item"].as_str().unwrap_or_default();

        match tool {
            "checklist_add" => self.add(item),
            "checklist_complete" => self.complete(item),
            "checklist_remove" => self.remove(item),
            "checklist_read" => self.shown().spoken(),
            other => format!("The checklist has no tool called {other}."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ward-checklist-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("checklist.json")
    }

    fn fresh(name: &str) -> Checklist {
        Checklist::load(&temp(name))
    }

    fn items(text: &[&str]) -> Vec<Item> {
        text.iter()
            .map(|t| Item {
                text: t.to_string(),
                done: false,
            })
            .collect()
    }

    #[test]
    fn adding_completing_and_reading_is_the_whole_loop() {
        let list = fresh("loop");

        list.run("checklist_add", &json!({"item": "buy tritium"}));
        list.run("checklist_add", &json!({"item": "refuel at Jameson"}));
        list.run("checklist_complete", &json!({"item": "tritium"}));

        assert_eq!(
            list.run("checklist_read", &json!({})),
            "Checklist: 1 of 2 done. Still to do: refuel at Jameson."
        );
    }

    #[test]
    fn a_completed_item_stays_on_the_list() {
        // The decision this list is built around. Removing it on completion
        // would leave "how far along am I" with no answer.
        let list = fresh("kept");

        list.run("checklist_add", &json!({"item": "buy tritium"}));
        list.run("checklist_complete", &json!({"item": "buy tritium"}));

        let Shown::List { rows, .. } = list.shown() else {
            panic!("a checklist should show as a list");
        };

        assert_eq!(rows.len(), 1, "the item was dropped: {rows:?}");
        assert!(rows[0].done);
    }

    #[test]
    fn removing_is_not_completing() {
        let list = fresh("removed");

        list.run("checklist_add", &json!({"item": "buy tritium"}));
        list.run("checklist_remove", &json!({"item": "tritium"}));

        let Shown::List { rows, .. } = list.shown() else {
            panic!("a checklist should show as a list");
        };

        assert!(rows.is_empty(), "still there: {rows:?}");
    }

    #[test]
    fn part_of_an_item_is_enough_to_name_it() {
        // Nobody repeats what they wrote down word for word, and the
        // transcription will not either.
        let list = items(&["buy tritium at Deciat", "swap to the Krait"]);

        assert_eq!(find(&list, "tritium"), Match::One(0));
        assert_eq!(find(&list, "the Krait"), Match::One(1));
        assert_eq!(find(&list, "BUY TRITIUM AT DECIAT."), Match::One(0));
    }

    #[test]
    fn saying_more_than_was_written_down_still_finds_it() {
        let list = items(&["refuel"]);
        assert_eq!(find(&list, "refuel at Jameson Memorial"), Match::One(0));
    }

    #[test]
    fn two_possible_items_are_never_guessed_between() {
        // The failure this prevents is silent: the Commander hears "done" and
        // believes the right thing was ticked.
        let list = items(&["buy tritium at Deciat", "buy tritium at Sol"]);

        let Match::Several(names) = find(&list, "buy tritium") else {
            panic!("it picked one of two");
        };

        assert_eq!(names.len(), 2);
    }

    #[test]
    fn being_asked_which_one_names_both() {
        let list = fresh("ambiguous");

        list.run("checklist_add", &json!({"item": "buy tritium at Deciat"}));
        list.run("checklist_add", &json!({"item": "buy tritium at Sol"}));

        let answer = list.run("checklist_complete", &json!({"item": "buy tritium"}));

        assert!(answer.contains("Deciat"), "got: {answer}");
        assert!(answer.contains("Sol"), "got: {answer}");
        assert!(answer.contains("Which one"), "got: {answer}");

        let Shown::List { rows, .. } = list.shown() else {
            panic!("a checklist should show as a list");
        };
        assert!(
            rows.iter().all(|r| !r.done),
            "something was ticked anyway: {rows:?}"
        );
    }

    #[test]
    fn an_exact_phrase_beats_being_contained_in_another() {
        // Without this, "refuel" could never be completed once "refuel at
        // Jameson" existed - the shorter item would be permanently ambiguous.
        let list = items(&["refuel", "refuel at Jameson"]);
        assert_eq!(find(&list, "refuel"), Match::One(0));
    }

    #[test]
    fn asking_for_something_that_is_not_there_says_what_is() {
        let list = fresh("absent");
        list.run("checklist_add", &json!({"item": "buy tritium"}));

        let answer = list.run("checklist_complete", &json!({"item": "sell the Cutter"}));

        assert!(answer.contains("Nothing on the checklist"), "got: {answer}");
        assert!(answer.contains("buy tritium"), "got: {answer}");
    }

    #[test]
    fn the_same_item_twice_is_added_once() {
        let list = fresh("duplicate");

        list.run("checklist_add", &json!({"item": "buy tritium"}));
        let answer = list.run("checklist_add", &json!({"item": "Buy tritium."}));

        assert!(answer.contains("already on the list"), "got: {answer}");

        let Shown::List { rows, .. } = list.shown() else {
            panic!("a checklist should show as a list");
        };
        assert_eq!(rows.len(), 1, "added twice: {rows:?}");
    }

    #[test]
    fn a_different_item_is_not_mistaken_for_a_duplicate() {
        let list = fresh("not-duplicate");

        list.run("checklist_add", &json!({"item": "buy tritium at Deciat"}));
        list.run("checklist_add", &json!({"item": "buy tritium at Sol"}));

        let Shown::List { rows, .. } = list.shown() else {
            panic!("a checklist should show as a list");
        };
        assert_eq!(rows.len(), 2, "one was refused: {rows:?}");
    }

    #[test]
    fn the_list_survives_a_restart() {
        let path = temp("restart");

        let list = Checklist::load(&path);
        list.run("checklist_add", &json!({"item": "buy tritium"}));
        list.run("checklist_add", &json!({"item": "refuel"}));
        list.run("checklist_complete", &json!({"item": "tritium"}));

        let reopened = Checklist::load(&path);

        assert_eq!(
            reopened.run("checklist_read", &json!({})),
            "Checklist: 1 of 2 done. Still to do: refuel."
        );
    }

    #[test]
    fn a_checklist_that_will_not_parse_is_kept_rather_than_replaced() {
        // The Commander's own writing. Starting empty and overwriting it with
        // the next item added is how it would be lost for good.
        let path = temp("unreadable");
        std::fs::write(&path, "{ this was never an array").unwrap();

        let list = Checklist::load(&path);
        list.run("checklist_add", &json!({"item": "buy tritium"}));

        let aside = path.with_extension("json.unreadable");

        assert!(aside.is_file(), "the old file was not kept");
        assert!(
            std::fs::read_to_string(&aside)
                .unwrap()
                .contains("never an array")
        );
    }

    #[test]
    fn an_empty_list_reads_as_empty_rather_than_as_a_failure() {
        let list = fresh("empty");
        assert_eq!(
            list.run("checklist_read", &json!({})),
            "Checklist is empty."
        );
    }

    #[test]
    fn adding_nothing_adds_nothing() {
        // The model can call a tool with a blank argument, and an empty row on
        // the panel is impossible to remove by voice.
        let list = fresh("blank");
        list.run("checklist_add", &json!({"item": "   "}));

        let Shown::List { rows, .. } = list.shown() else {
            panic!("a checklist should show as a list");
        };
        assert!(rows.is_empty(), "a blank item was added: {rows:?}");
    }

    #[test]
    fn completing_something_twice_says_so_rather_than_counting_it_again() {
        let list = fresh("twice");
        list.run("checklist_add", &json!({"item": "buy tritium"}));
        list.run("checklist_complete", &json!({"item": "tritium"}));

        let answer = list.run("checklist_complete", &json!({"item": "tritium"}));
        assert!(answer.contains("already done"), "got: {answer}");
    }

    #[test]
    fn the_list_cannot_grow_without_limit() {
        let list = fresh("full");

        for n in 0..MOST_ITEMS + 5 {
            list.run("checklist_add", &json!({ "item": format!("item {n}") }));
        }

        let Shown::List { rows, .. } = list.shown() else {
            panic!("a checklist should show as a list");
        };
        assert_eq!(rows.len(), MOST_ITEMS);
    }

    #[test]
    fn two_writers_at_once_lose_nothing() {
        // The race the lock exists for: a panel edit and a voice edit arriving
        // together. Read, change and write have to happen with no other writer
        // in between, or a create is lost.
        use std::sync::Arc;

        let list = Arc::new(fresh("racing"));
        let mut hands = Vec::new();

        for worker in 0..8 {
            let list = list.clone();
            hands.push(std::thread::spawn(move || {
                for n in 0..10 {
                    list.run("checklist_add", &json!({ "item": format!("{worker}-{n}") }));
                }
            }));
        }

        for hand in hands {
            hand.join().unwrap();
        }

        let Shown::List { rows, .. } = list.shown() else {
            panic!("a checklist should show as a list");
        };
        assert_eq!(rows.len(), 80, "an add was lost");
    }

    #[test]
    fn an_unknown_tool_returns_text_rather_than_failing() {
        let list = fresh("unknown");
        assert!(
            list.run("checklist_burn", &json!({}))
                .contains("no tool called")
        );
    }
}
