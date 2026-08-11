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
//! ## It is a markdown file, because a checklist is a thing people edit
//!
//! `data/checklist.md`, holding `- [x] buy tritium at Deciat`. Everything else
//! Ward writes is JSON, and this is not, on purpose: the settings file is
//! something you correct once, and this is a list you will want open in an
//! editor beside the game. Markdown task lists are the notation everyone
//! already has, and the file reads as itself in a diff, a text box, or a
//! terminal.
//!
//! Two consequences fall out of that choice and both are improvements.
//!
//! **Nothing the Commander writes is thrown away.** A line that is not a task
//! is kept exactly where it was and written back untouched, so a heading, a
//! blank line or a note between items survives Ward editing the list around it.
//!
//! **There is no such thing as a file that will not parse.** A malformed line
//! is simply a line that is not a task, which means the whole question of what
//! to do with an unreadable checklist — and the code that moved it aside —
//! stops existing.
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

use serde_json::Value;

use crate::capability::{Capability, Kind, Slot, Tool};
use crate::shown::{Row, Shown};

/// What the Commander sees this list called, everywhere it appears.
const TITLE: &str = "Checklist";

/// What a new file opens with, so it reads as a document rather than as
/// somebody's data format. Kept as ordinary lines, so the Commander can change
/// it and Ward will not put it back.
const HEADER: [&str; 2] = ["# Checklist", ""];

/// A cap on the list, so a stuck loop or a misheard instruction repeated forty
/// times is bounded trouble rather than a file that grows until something else
/// notices.
const MOST_ITEMS: usize = 200;

/// One line of the file.
///
/// Everything is kept, not only the parts Ward understands. The alternative is
/// that a note written between two items disappears the next time anything is
/// ticked, which is the sort of quiet loss that stops somebody trusting a file.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    Item {
        text: String,
        done: bool,
    },
    /// Anything else, held exactly as written.
    Kept(String),
}

/// Reads one line of the file.
///
/// Liberal about the notation, because the point of markdown is that the
/// Commander types it themselves: any of the three bullet characters, any
/// indentation, and a tick in either case.
fn read_line(line: &str) -> Line {
    let trimmed = line.trim_start();

    let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    else {
        return Line::Kept(line.to_string());
    };

    let done = match rest.get(..3) {
        Some("[ ]") => false,
        Some("[x]") | Some("[X]") => true,
        _ => return Line::Kept(line.to_string()),
    };

    let text = rest[3..].trim().to_string();

    // A bullet with a box and nothing after it is not an item anybody can name
    // out loud, so it stays a line rather than becoming an unreachable task.
    match text.is_empty() {
        true => Line::Kept(line.to_string()),
        false => Line::Item { text, done },
    }
}

fn write_line(line: &Line) -> String {
    match line {
        Line::Kept(raw) => raw.clone(),
        Line::Item { text, done } => match done {
            true => format!("- [x] {text}"),
            false => format!("- [ ] {text}"),
        },
    }
}

/// Which item a spoken phrase meant.
///
/// Three outcomes and not two. Transcription of a half-remembered phrase is
/// exactly where a single best guess goes wrong, and marking the wrong item
/// done is a silent failure — the Commander hears "done" and believes it.
#[derive(Debug, PartialEq)]
enum Match {
    /// The line it is on, not the item's position among items.
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

/// Every task in the file, with the line each one is on.
fn tasks(lines: &[Line]) -> Vec<(usize, &str, bool)> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(at, line)| match line {
            Line::Item { text, done } => Some((at, text.as_str(), *done)),
            Line::Kept(_) => None,
        })
        .collect()
}

/// Finds the one item a phrase means, or refuses to choose.
fn find(lines: &[Line], phrase: &str) -> Match {
    let needle = plain(phrase);

    if needle.is_empty() {
        return Match::None;
    }

    let tasks = tasks(lines);

    // An exact phrase wins outright, even if it is also contained in others.
    // Otherwise "refuel" could never be completed once "refuel at Jameson"
    // existed.
    if let Some((at, _, _)) = tasks.iter().find(|(_, text, _)| plain(text) == needle) {
        return Match::One(*at);
    }

    // Containment in either direction, because the Commander may say less than
    // they wrote down or more.
    let hits: Vec<(usize, &str)> = tasks
        .iter()
        .filter(|(_, text, _)| {
            let text = plain(text);
            text.contains(&needle) || needle.contains(&text)
        })
        .map(|(at, text, _)| (*at, *text))
        .collect();

    match hits.len() {
        0 => Match::None,
        1 => Match::One(hits[0].0),
        _ => Match::Several(hits.iter().map(|(_, text)| text.to_string()).collect()),
    }
}

/// The list, and the only thing allowed to change it.
pub struct Checklist {
    path: PathBuf,
    lines: Mutex<Vec<Line>>,
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
    /// Loads the list, or starts a new file.
    pub fn load(path: &Path) -> Self {
        let lines: Vec<Line> = match std::fs::read_to_string(path) {
            Ok(raw) => raw.lines().map(read_line).collect(),
            // A file that is not there is an ordinary first run. One that
            // cannot be read is worth saying out loud, but it is still not a
            // reason to refuse to start.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                HEADER.iter().map(|l| Line::Kept(l.to_string())).collect()
            }
            Err(e) => {
                tracing::warn!(
                    target: "ward::checklist",
                    error = %e,
                    path = %path.display(),
                    "the checklist could not be read, starting a new one"
                );
                HEADER.iter().map(|l| Line::Kept(l.to_string())).collect()
            }
        };

        tracing::info!(
            target: "ward::checklist",
            items = tasks(&lines).len(),
            "loaded"
        );

        Self {
            path: path.to_path_buf(),
            lines: Mutex::new(lines),
        }
    }

    /// Writes the list out. Called with the lock already held.
    ///
    /// Returns what to append to the Commander's answer, which is nothing when
    /// it worked. A checklist that quietly stops saving is one the Commander
    /// keeps using and then loses.
    fn save(&self, lines: &[Line]) -> String {
        let mut body: String = lines.iter().map(write_line).collect::<Vec<_>>().join("\n");
        body.push('\n');

        match crate::config::write_atomically(&self.path, &body) {
            Ok(()) => String::new(),
            Err(e) => {
                tracing::error!(target: "ward::checklist", error = %e, "could not save the checklist");
                " I could not save the list, so this will not survive a restart.".into()
            }
        }
    }

    fn held(&self) -> std::sync::MutexGuard<'_, Vec<Line>> {
        match self.lines.lock() {
            Ok(lines) => lines,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Everything on the list right now.
    pub fn shown(&self) -> Shown {
        let rows = tasks(&self.held())
            .into_iter()
            .map(|(_, text, done)| Row {
                text: text.to_string(),
                done,
            })
            .collect();

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

        let mut lines = self.held();

        if tasks(&lines).len() >= MOST_ITEMS {
            return format!("The checklist is full at {MOST_ITEMS} items.");
        }

        // Said twice is meant once. Adding it again would leave the Commander
        // completing one copy and reading back the other.
        if let Match::One(at) = find(&lines, text)
            && let Line::Item { text: existing, .. } = &lines[at]
            && plain(existing) == plain(text)
        {
            return format!("\"{existing}\" is already on the list.");
        }

        // Beneath the last task rather than at the end of the file, so a note
        // the Commander wrote under the list stays under the list.
        let at = match tasks(&lines).last() {
            Some((last, _, _)) => last + 1,
            None => lines.len(),
        };

        lines.insert(
            at,
            Line::Item {
                text: text.to_string(),
                done: false,
            },
        );

        let trouble = self.save(&lines);

        format!("Added \"{text}\" to the checklist.{trouble}")
    }

    fn complete(&self, phrase: &str) -> String {
        let mut lines = self.held();

        match find(&lines, phrase) {
            Match::None => nothing_like(phrase, &lines),
            Match::Several(names) => ambiguous(phrase, &names),
            Match::One(at) => {
                let Line::Item { text, done } = &mut lines[at] else {
                    return nothing_like(phrase, &lines);
                };

                if *done {
                    return format!("\"{text}\" was already done.");
                }

                *done = true;
                let text = text.clone();

                let left = tasks(&lines).iter().filter(|(_, _, done)| !done).count();
                let trouble = self.save(&lines);

                match left {
                    0 => format!("\"{text}\" done. That is the whole list.{trouble}"),
                    1 => format!("\"{text}\" done. One left.{trouble}"),
                    n => format!("\"{text}\" done. {n} left.{trouble}"),
                }
            }
        }
    }

    fn remove(&self, phrase: &str) -> String {
        let mut lines = self.held();

        match find(&lines, phrase) {
            Match::None => nothing_like(phrase, &lines),
            Match::Several(names) => ambiguous(phrase, &names),
            Match::One(at) => {
                let Line::Item { text, .. } = lines.remove(at) else {
                    return nothing_like(phrase, &lines);
                };

                let trouble = self.save(&lines);

                format!("Took \"{text}\" off the checklist.{trouble}")
            }
        }
    }
}

/// Nothing matched, so say what is there rather than only that it is not.
fn nothing_like(phrase: &str, lines: &[Line]) -> String {
    let names: Vec<&str> = tasks(lines).into_iter().map(|(_, text, _)| text).collect();

    if names.is_empty() {
        return "The checklist is empty.".to_string();
    }

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
        dir.join("checklist.md")
    }

    fn fresh(name: &str) -> Checklist {
        Checklist::load(&temp(name))
    }

    fn lines(markdown: &str) -> Vec<Line> {
        markdown.lines().map(read_line).collect()
    }

    fn rows_of(list: &Checklist) -> Vec<Row> {
        let Shown::List { rows, .. } = list.shown() else {
            panic!("a checklist should show as a list");
        };
        rows
    }

    #[test]
    fn a_task_list_is_read_as_tasks() {
        assert_eq!(
            read_line("- [ ] buy tritium"),
            Line::Item {
                text: "buy tritium".into(),
                done: false
            }
        );
        assert_eq!(
            read_line("- [x] buy tritium"),
            Line::Item {
                text: "buy tritium".into(),
                done: true
            }
        );
    }

    #[test]
    fn the_notation_a_person_would_actually_type_is_accepted() {
        // The point of markdown is that the Commander writes it themselves, so
        // being strict about which bullet or which case of tick would make the
        // format a trap rather than a convenience.
        for written in [
            "* [x] buy tritium",
            "+ [x] buy tritium",
            "  - [X] buy tritium",
            "- [x]   buy tritium  ",
        ] {
            assert_eq!(
                read_line(written),
                Line::Item {
                    text: "buy tritium".into(),
                    done: true
                },
                "did not read: {written:?}"
            );
        }
    }

    #[test]
    fn anything_that_is_not_a_task_stays_a_line() {
        assert_eq!(read_line("# Checklist"), Line::Kept("# Checklist".into()));
        assert_eq!(read_line(""), Line::Kept("".into()));
        assert_eq!(
            read_line("just a note to myself"),
            Line::Kept("just a note to myself".into())
        );
        // A box with nothing after it cannot be named out loud, so it is not a
        // task - it would be an item no voice command could ever reach.
        assert_eq!(read_line("- [ ] "), Line::Kept("- [ ] ".into()));
    }

    #[test]
    fn what_is_written_is_what_is_read_back() {
        let markdown = "- [x] buy tritium at Deciat";
        assert_eq!(write_line(&read_line(markdown)), markdown);
    }

    #[test]
    fn the_file_on_disk_is_markdown_a_person_can_edit() {
        let path = temp("looks-right");
        let list = Checklist::load(&path);

        list.run("checklist_add", &json!({"item": "buy tritium at Deciat"}));
        list.run("checklist_add", &json!({"item": "refuel"}));
        list.run("checklist_complete", &json!({"item": "tritium"}));

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# Checklist\n\n- [x] buy tritium at Deciat\n- [ ] refuel\n"
        );
    }

    #[test]
    fn a_note_the_commander_wrote_survives_ward_editing_around_it() {
        // The whole reason for keeping every line. A note that disappears the
        // next time anything is ticked is how somebody stops trusting a file.
        let path = temp("preserved");
        std::fs::write(
            &path,
            "# My run\n\nDeciat trip, do these in order:\n\n- [ ] buy tritium\n\nremember the fuel scoop\n",
        )
        .unwrap();

        let list = Checklist::load(&path);
        list.run("checklist_complete", &json!({"item": "tritium"}));
        list.run("checklist_add", &json!({"item": "refuel"}));

        let written = std::fs::read_to_string(&path).unwrap();

        assert!(written.contains("# My run"), "got: {written}");
        assert!(
            written.contains("Deciat trip, do these in order:"),
            "got: {written}"
        );
        assert!(
            written.contains("remember the fuel scoop"),
            "got: {written}"
        );
        assert!(written.contains("- [x] buy tritium"), "got: {written}");
    }

    #[test]
    fn a_new_item_goes_under_the_list_rather_than_at_the_end_of_the_file() {
        let path = temp("placed");
        std::fs::write(&path, "- [ ] buy tritium\n\nremember the fuel scoop\n").unwrap();

        let list = Checklist::load(&path);
        list.run("checklist_add", &json!({"item": "refuel"}));

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "- [ ] buy tritium\n- [ ] refuel\n\nremember the fuel scoop\n"
        );
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

        let rows = rows_of(&list);
        assert_eq!(rows.len(), 1, "the item was dropped: {rows:?}");
        assert!(rows[0].done);
    }

    #[test]
    fn removing_is_not_completing() {
        let list = fresh("removed");

        list.run("checklist_add", &json!({"item": "buy tritium"}));
        list.run("checklist_remove", &json!({"item": "tritium"}));

        assert!(rows_of(&list).is_empty());
    }

    #[test]
    fn part_of_an_item_is_enough_to_name_it() {
        // Nobody repeats what they wrote down word for word, and the
        // transcription will not either.
        let list = lines("- [ ] buy tritium at Deciat\n- [ ] swap to the Krait");

        assert_eq!(find(&list, "tritium"), Match::One(0));
        assert_eq!(find(&list, "the Krait"), Match::One(1));
        assert_eq!(find(&list, "BUY TRITIUM AT DECIAT."), Match::One(0));
    }

    #[test]
    fn a_phrase_is_matched_against_tasks_and_not_against_notes() {
        // Lines that are not tasks are kept, which means they are also in the
        // list being searched. Matching one would offer to complete a heading.
        let list = lines("# Tritium run\n\n- [ ] refuel\n");
        assert_eq!(find(&list, "tritium"), Match::None);
    }

    #[test]
    fn saying_more_than_was_written_down_still_finds_it() {
        let list = lines("- [ ] refuel");
        assert_eq!(find(&list, "refuel at Jameson Memorial"), Match::One(0));
    }

    #[test]
    fn two_possible_items_are_never_guessed_between() {
        // The failure this prevents is silent: the Commander hears "done" and
        // believes the right thing was ticked.
        let list = lines("- [ ] buy tritium at Deciat\n- [ ] buy tritium at Sol");

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

        assert!(
            rows_of(&list).iter().all(|r| !r.done),
            "something was ticked anyway"
        );
    }

    #[test]
    fn an_exact_phrase_beats_being_contained_in_another() {
        // Without this, "refuel" could never be completed once "refuel at
        // Jameson" existed - the shorter item would be permanently ambiguous.
        let list = lines("- [ ] refuel\n- [ ] refuel at Jameson");
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
        assert_eq!(rows_of(&list).len(), 1, "added twice");
    }

    #[test]
    fn a_different_item_is_not_mistaken_for_a_duplicate() {
        let list = fresh("not-duplicate");

        list.run("checklist_add", &json!({"item": "buy tritium at Deciat"}));
        list.run("checklist_add", &json!({"item": "buy tritium at Sol"}));

        assert_eq!(rows_of(&list).len(), 2, "one was refused");
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
        assert!(rows_of(&list).is_empty());
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

        assert_eq!(rows_of(&list).len(), MOST_ITEMS);
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

        assert_eq!(rows_of(&list).len(), 80, "an add was lost");
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
