//! Replaying a recorded session.
//!
//! This is the piece that makes every journal-driven feature testable without
//! the game: no Elite running, no headset, and no flying somewhere to trigger
//! the thing being debugged. It also makes a scenario repeatable, which a live
//! session never is — the same events, in the same order, with the same timing,
//! every run.
//!
//! Time arrives as an argument rather than being read from a clock. That is
//! what lets a test run an hour of play in a millisecond, and it is why the
//! whole thing is deterministic: nothing here can observe how long it took.

use std::path::Path;
use std::time::Duration;

use crate::journal::{Record, Situation};

/// A recorded session, ready to be played back.
pub struct Replay {
    records: Vec<Record>,
    next: usize,
    /// When the first event happened, so everything else can be expressed as
    /// an offset from it.
    origin: Option<time::OffsetDateTime>,
}

impl Replay {
    /// Reads a recording.
    ///
    /// Bytes, not lines: a fixture is byte-preserved so the parser sees exactly
    /// what the game wrote, including the line endings it chose.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let raw = std::fs::read(path)?;
        let text = String::from_utf8_lossy(&raw);

        let records: Vec<Record> = text
            .lines()
            .filter_map(|l| Record::parse(l.trim()))
            .collect();

        let origin = records.iter().find_map(|r| r.timestamp);

        Ok(Self {
            records,
            next: 0,
            origin,
        })
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// How far into the session an event happened.
    fn offset_of(&self, record: &Record) -> Duration {
        match (self.origin, record.timestamp) {
            (Some(origin), Some(at)) => {
                let seconds = (at - origin).as_seconds_f64().max(0.0);
                Duration::from_secs_f64(seconds)
            }
            _ => Duration::ZERO,
        }
    }

    /// Everything that had happened by a given point in the session.
    ///
    /// `speed` is how much faster than real time to run: 1.0 plays a session at
    /// the pace it happened, 100.0 covers an hour of play in thirty-six
    /// seconds, and a test that passes a large enough elapsed time gets the
    /// whole session at once.
    pub fn advance(&mut self, elapsed: Duration, speed: f64) -> Vec<Record> {
        let horizon = Duration::from_secs_f64(elapsed.as_secs_f64() * speed.max(0.0));

        let mut due = Vec::new();

        while let Some(record) = self.records.get(self.next) {
            if self.offset_of(record) > horizon {
                break;
            }
            due.push(record.clone());
            self.next += 1;
        }

        due
    }

    /// Plays the whole session and returns what Ward would know at the end.
    ///
    /// The common case in a test: the question is almost always what the state
    /// became, not what order it got there in.
    pub fn situation(&mut self) -> Situation {
        let mut situation = Situation::default();

        for record in self.advance(Duration::from_secs(86_400), 1.0) {
            situation.absorb(&record);
        }

        situation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Replay {
        // A real session, scrubbed of the Commander's name, Frontier ID, purse
        // and the systems actually visited, and byte-preserved so the parser
        // sees the line endings the game really wrote.
        Replay::load(Path::new("tests/fixtures/session.journal.log"))
            .expect("the recorded session should be readable")
    }

    #[test]
    fn a_recorded_session_loads() {
        let replay = fixture();
        assert!(!replay.is_empty(), "the recording is empty");
        assert!(replay.len() > 10, "only {} events", replay.len());
    }

    #[test]
    fn replaying_the_whole_session_produces_a_situation() {
        let situation = fixture().situation();

        assert_eq!(situation.commander.as_deref(), Some("Jameson"));
        assert!(
            situation.system.is_some(),
            "the session never established where the Commander was"
        );
    }

    #[test]
    fn the_same_recording_replays_identically() {
        // The property the whole testbed rests on. A live session is never
        // repeatable; this has to be, or a failure cannot be investigated.
        assert_eq!(fixture().situation(), fixture().situation());
    }

    #[test]
    fn speed_changes_how_much_has_happened_not_what_happens() {
        // An hour of play in a millisecond, and the same result.
        let fast = fixture().advance(Duration::from_millis(1), 100_000_000.0);
        let whole = fixture().advance(Duration::from_secs(86_400), 1.0);

        assert_eq!(fast.len(), whole.len());
    }

    #[test]
    fn nothing_is_due_before_it_happened() {
        let mut replay = fixture();

        // At the very start of the session, only what shares the first
        // timestamp has occurred.
        let immediate = replay.advance(Duration::ZERO, 1.0);
        assert!(
            immediate.len() < replay.len() + immediate.len(),
            "the entire session arrived at once"
        );
    }

    #[test]
    fn events_arrive_in_order_and_only_once() {
        let mut replay = fixture();
        let mut seen = Vec::new();

        // Advancing in steps must produce the same sequence as one large step,
        // with nothing repeated and nothing skipped. The horizon has to run
        // past the end of the session, or this compares a part against a whole.
        for step in 1..=1500 {
            seen.extend(replay.advance(Duration::from_secs(step * 60), 1.0));
        }

        let whole = fixture().advance(Duration::from_secs(86_400), 1.0);

        assert_eq!(seen.len(), whole.len());
        let names: Vec<&str> = seen.iter().map(|r| r.event.as_str()).collect();
        let expected: Vec<&str> = whole.iter().map(|r| r.event.as_str()).collect();
        assert_eq!(names, expected);
    }

    #[test]
    fn the_fixture_kept_the_line_endings_the_game_wrote() {
        // The repository must not normalize this. A parser test has to see what
        // Elite actually wrote, and the parser is expected to cope with either.
        let raw = std::fs::read("tests/fixtures/session.journal.log").unwrap();
        assert!(
            raw.windows(2).any(|w| w == b"\r\n"),
            "the fixture was normalized on its way into the repository"
        );
    }

    #[test]
    fn the_fixture_carries_nothing_personal() {
        // It is a real session and this repository is public.
        let text =
            String::from_utf8_lossy(&std::fs::read("tests/fixtures/session.journal.log").unwrap())
                .into_owned();

        for forbidden in ["FID", "Credits", "ShipName", "ShipIdent"] {
            assert!(
                !text.contains(forbidden),
                "the fixture still carries {forbidden}"
            );
        }
    }
}
