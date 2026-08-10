//! Reading what the game writes down.
//!
//! Elite keeps an append-only journal, one file per session, and writes a
//! line for everything that happens. It is the only honest source of what the
//! Commander is actually doing: a model asked where you are will otherwise
//! answer from what it remembers about the game, confidently and wrongly.
//!
//! The watcher is **pull-based** — no thread, no timer, nothing running inside
//! it. That is what makes it testable: a test hands it a file and asks what
//! changed, with no game, no waiting and nothing to synchronize against.
//!
//! Several details here look arbitrary and are not. Each one is a way this
//! goes wrong that costs an afternoon to find.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

/// One line of the journal.
#[derive(Clone, Debug)]
pub struct Record {
    /// The event name, which is the only field every line is guaranteed to have.
    pub event: String,
    /// Always UTC.
    ///
    /// The game writes UTC and says so. Parsing it as local time is a mistake
    /// that hides for months and then surfaces as a count of what happened
    /// "today" being wrong by an hour's worth of events near midnight.
    pub timestamp: Option<time::OffsetDateTime>,
    pub raw: Value,
}

impl Record {
    fn parse(line: &str) -> Option<Self> {
        let raw: Value = serde_json::from_str(line).ok()?;
        let event = raw.get("event")?.as_str()?.to_string();

        let timestamp = raw.get("timestamp").and_then(|t| t.as_str()).and_then(|t| {
            time::OffsetDateTime::parse(t, &time::format_description::well_known::Rfc3339).ok()
        });

        Some(Self {
            event,
            timestamp,
            raw,
        })
    }

    pub fn text(&self, field: &str) -> Option<&str> {
        self.raw.get(field)?.as_str()
    }
}

/// Picks the journal the game is writing to now.
///
/// **By filename, not by modification time.** The name carries a fixed-width
/// session start and a part number, so sorting the names sorts the sessions.
/// Modification time does not survive a file being copied, restored, or checked
/// into a repository as a fixture — and the moment it stops surviving is the
/// moment tests start reading the wrong file for reasons nobody can see.
pub fn newest(dir: &Path) -> Option<PathBuf> {
    let mut journals: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| is_journal(path))
        .collect();

    journals.sort();
    journals.pop()
}

/// Journals only. The game writes several other files into the same folder,
/// and one of them belongs to somebody else's software entirely.
fn is_journal(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };

    name.starts_with("Journal.") && name.ends_with(".log")
}

/// Follows one journal file.
pub struct Watcher {
    path: PathBuf,
    offset: u64,
    /// Whether the first read has happened.
    ///
    /// The first read returns everything already in the file, which is the
    /// session so far rather than news. A caller wants to build state from it
    /// without announcing all of it aloud, so the two are told apart.
    started: bool,
}

/// What a read produced.
pub struct Read {
    /// Lines that were already there when Ward started looking.
    pub backlog: Vec<Record>,
    /// Lines written since the last read.
    pub fresh: Vec<Record>,
}

impl Watcher {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            started: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads whatever has been appended since last time.
    ///
    /// Fails soft, always. A line that will not parse is skipped rather than
    /// ending the session's monitoring: one malformed event, or one transient
    /// failure to open a file the game has open too, must not be the reason
    /// Ward stops knowing where you are for the next four hours.
    pub fn poll(&mut self) -> Read {
        let mut backlog = Vec::new();
        let mut fresh = Vec::new();

        // Opened afresh each time, and shared. The game holds this file open
        // for the whole session and would refuse an exclusive handle.
        let Ok(file) = File::open(&self.path) else {
            return Read { backlog, fresh };
        };

        let mut reader = BufReader::new(file);

        if reader.seek(SeekFrom::Start(self.offset)).is_err() {
            return Read { backlog, fresh };
        }

        let mut line = String::new();

        loop {
            line.clear();

            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => break,
            }

            // A line without its terminator is a line the game is still
            // writing. Leaving the offset behind it means it gets read whole
            // next time, rather than being parsed as a truncated fragment and
            // silently dropped.
            if !line.ends_with('\n') {
                break;
            }

            // Taken from where the reader actually is, not from bytes counted
            // by hand: the two disagree the moment anything about decoding or
            // line endings is not what was assumed.
            let position = reader.stream_position().unwrap_or(self.offset);

            if let Some(record) = Record::parse(line.trim()) {
                if self.started {
                    fresh.push(record);
                } else {
                    backlog.push(record);
                }
            }

            self.offset = position;
        }

        self.started = true;

        Read { backlog, fresh }
    }
}

/// What Ward knows about the Commander's situation.
///
/// Only what the journal actually said. A field that has never been mentioned
/// stays empty rather than being guessed at, because an empty answer is
/// recoverable and a confident wrong one is not.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Situation {
    pub commander: Option<String>,
    pub system: Option<String>,
    pub body: Option<String>,
    pub station: Option<String>,
    pub ship: Option<String>,
    pub docked: bool,
    pub jumps_remaining: Option<u64>,
}

impl Situation {
    /// Folds one record into what is known.
    pub fn absorb(&mut self, record: &Record) {
        match record.event.as_str() {
            "Commander" => {
                self.commander = record.text("Name").map(str::to_string);
            }
            "LoadGame" => {
                self.commander = record.text("Commander").map(str::to_string);
                self.ship = record
                    .text("Ship_Localised")
                    .or(record.text("Ship"))
                    .map(str::to_string);
            }
            "Loadout" => {
                self.ship = record
                    .text("Ship_Localised")
                    .or(record.text("Ship"))
                    .map(str::to_string);
            }
            "Location" | "FSDJump" | "CarrierJump" => {
                self.system = record.text("StarSystem").map(str::to_string);
                self.body = record.text("Body").map(str::to_string);
                self.docked = record
                    .raw
                    .get("Docked")
                    .and_then(|d| d.as_bool())
                    .unwrap_or(false);
                if !self.docked {
                    self.station = None;
                }
            }
            "Docked" => {
                self.docked = true;
                self.station = record.text("StationName").map(str::to_string);
                self.system = record
                    .text("StarSystem")
                    .map(str::to_string)
                    .or(self.system.take());
            }
            "Undocked" => {
                self.docked = false;
                self.station = None;
            }
            "SupercruiseExit" => {
                self.body = record.text("Body").map(str::to_string);
            }
            "NavRoute" | "NavRouteClear" => {}
            _ => {}
        }
    }

    /// What the model is told, as plain sentences.
    ///
    /// Only what is known. Saying nothing about the ship is better than a
    /// placeholder the model will treat as a fact.
    pub fn brief(&self) -> String {
        let mut lines = Vec::new();

        if let Some(commander) = &self.commander {
            lines.push(format!("The Commander is {commander}."));
        }

        match (&self.system, &self.body) {
            (Some(system), Some(body)) if body != system => {
                lines.push(format!("They are in {system}, near {body}."));
            }
            (Some(system), _) => lines.push(format!("They are in {system}.")),
            _ => {}
        }

        if self.docked {
            match &self.station {
                Some(station) => lines.push(format!("Docked at {station}.")),
                None => lines.push("Docked.".to_string()),
            }
        }

        if let Some(ship) = &self.ship {
            lines.push(format!("Flying a {ship}."));
        }

        if lines.is_empty() {
            "Nothing is known about the Commander's situation yet. The game may not be running."
                .to_string()
        } else {
            lines.join(" ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ward-journal-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn record(json: &str) -> Record {
        Record::parse(json).unwrap()
    }

    #[test]
    fn the_newest_journal_is_chosen_by_name_not_by_modification_time() {
        let dir = temp("newest");

        // Written in an order that disagrees with their names, and the older
        // name is touched last so its modification time is newest. Sorting by
        // time would pick the wrong one.
        for name in [
            "Journal.2026-08-10T073436.01.log",
            "Journal.2026-08-09T181024.01.log",
        ] {
            std::fs::write(dir.join(name), "{}\n").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let chosen = newest(&dir).unwrap();
        assert_eq!(
            chosen.file_name().unwrap(),
            "Journal.2026-08-10T073436.01.log"
        );
    }

    #[test]
    fn other_files_in_the_folder_are_not_journals() {
        let dir = temp("filter");
        std::fs::write(dir.join("Status.json"), "{}").unwrap();
        std::fs::write(dir.join("NavRoute.json"), "{}").unwrap();
        std::fs::write(dir.join("Journal.2026-08-10T073436.01.log"), "{}\n").unwrap();

        // The game writes several other files into this folder, and other
        // software writes its own. Only journals are journals.
        assert_eq!(
            newest(&dir).unwrap().file_name().unwrap(),
            "Journal.2026-08-10T073436.01.log"
        );
    }

    #[test]
    fn an_empty_folder_yields_nothing() {
        assert!(newest(&temp("empty")).is_none());
    }

    #[test]
    fn the_first_read_is_backlog_and_later_reads_are_news() {
        let dir = temp("backlog");
        let path = dir.join("Journal.2026-08-10T073436.01.log");

        std::fs::write(
            &path,
            "{\"timestamp\":\"2026-08-10T07:34:36Z\",\"event\":\"LoadGame\"}\n\
             {\"timestamp\":\"2026-08-10T07:35:00Z\",\"event\":\"Location\"}\n",
        )
        .unwrap();

        let mut watcher = Watcher::new(path.clone());
        let first = watcher.poll();

        assert_eq!(first.backlog.len(), 2, "the session so far is not news");
        assert!(first.fresh.is_empty());

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(
            file,
            "{{\"timestamp\":\"2026-08-10T07:40:00Z\",\"event\":\"FSDJump\"}}"
        )
        .unwrap();

        let second = watcher.poll();
        assert!(second.backlog.is_empty());
        assert_eq!(second.fresh.len(), 1);
        assert_eq!(second.fresh[0].event, "FSDJump");
    }

    #[test]
    fn a_line_still_being_written_is_left_for_next_time() {
        // The game writes a line in pieces. Reading half of one and treating it
        // as whole loses the event entirely.
        let dir = temp("partial");
        let path = dir.join("Journal.2026-08-10T073436.01.log");

        std::fs::write(&path, "{\"event\":\"Docked\"}\n{\"event\":\"Und").unwrap();

        let mut watcher = Watcher::new(path.clone());
        assert_eq!(watcher.poll().backlog.len(), 1, "read a partial line");

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "ocked\"}}").unwrap();

        let after = watcher.poll();
        assert_eq!(after.fresh.len(), 1);
        assert_eq!(after.fresh[0].event, "Undocked");
    }

    #[test]
    fn one_bad_line_does_not_end_the_session() {
        // A single malformed event must not be why Ward stops knowing where you
        // are for the rest of the evening.
        let dir = temp("bad-line");
        let path = dir.join("Journal.2026-08-10T073436.01.log");

        std::fs::write(
            &path,
            "{\"event\":\"Docked\"}\n\
             this line is not json at all\n\
             {\"event\":\"Undocked\"}\n",
        )
        .unwrap();

        let read = Watcher::new(path).poll();

        assert_eq!(read.backlog.len(), 2);
        assert_eq!(read.backlog[1].event, "Undocked");
    }

    #[test]
    fn a_missing_file_is_not_a_crash() {
        let mut watcher = Watcher::new(temp("gone").join("Journal.2026-01-01T000000.01.log"));
        assert!(watcher.poll().backlog.is_empty());
    }

    #[test]
    fn both_line_endings_parse() {
        // The game writes one of these and a fixture may preserve either. The
        // parser is expected to handle both rather than the repository being
        // expected to normalize them.
        let dir = temp("endings");
        let path = dir.join("Journal.2026-08-10T073436.01.log");

        std::fs::write(
            &path,
            "{\"event\":\"Docked\"}\r\n{\"event\":\"Undocked\"}\n",
        )
        .unwrap();

        assert_eq!(Watcher::new(path).poll().backlog.len(), 2);
    }

    #[test]
    fn a_timestamp_is_read_as_utc() {
        // The game writes UTC and says so. Reading it as local time skews any
        // count of what happened today by the size of the offset.
        let r = record("{\"timestamp\":\"2026-08-10T07:34:36Z\",\"event\":\"FSDJump\"}");
        let t = r.timestamp.unwrap();

        assert_eq!(t.offset(), time::UtcOffset::UTC);
        assert_eq!(t.hour(), 7);
    }

    #[test]
    fn arriving_somewhere_sets_where_you_are() {
        let mut s = Situation::default();
        s.absorb(&record(
            "{\"event\":\"FSDJump\",\"StarSystem\":\"Shinrarta Dezhra\",\"Body\":\"Shinrarta Dezhra A\"}",
        ));

        assert_eq!(s.system.as_deref(), Some("Shinrarta Dezhra"));
        assert!(s.brief().contains("Shinrarta Dezhra"));
    }

    #[test]
    fn docking_and_leaving_are_remembered() {
        let mut s = Situation::default();

        s.absorb(&record(
            "{\"event\":\"Docked\",\"StationName\":\"Jameson Memorial\",\"StarSystem\":\"Shinrarta Dezhra\"}",
        ));
        assert!(s.docked);
        assert!(s.brief().contains("Jameson Memorial"));

        s.absorb(&record("{\"event\":\"Undocked\"}"));
        assert!(!s.docked);
        assert!(!s.brief().contains("Jameson Memorial"));
    }

    #[test]
    fn nothing_known_says_so_rather_than_inventing() {
        // The whole point. An empty answer is recoverable; a confident wrong
        // one is what the guardrails exist to prevent.
        let brief = Situation::default().brief();
        assert!(brief.contains("Nothing is known"), "got: {brief}");
        assert!(brief.contains("may not be running"));
    }

    #[test]
    fn an_unknown_event_changes_nothing() {
        let mut s = Situation::default();
        s.absorb(&record(
            "{\"event\":\"SomethingAddedIn2027\",\"Whatever\":1}",
        ));
        assert_eq!(s, Situation::default());
    }
}
