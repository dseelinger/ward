//! Diagnostics: levels, per-subsystem targets, structured fields, two sinks.
//!
//! This is a **third stream**, and keeping it separate is deliberate. Captions
//! are what a Commander hears rendered as text; the live log is the
//! conversation, readable as a conversation. Diagnostics are for working out
//! why something misbehaved. Merging the last two turns the log into a debug
//! dump and it stops being readable as a conversation at exactly the moment
//! somebody wants to check what was said.
//!
//! Two sinks, because there are two readers:
//!
//! - `ward.log` — lines a person reads while something is going wrong.
//! - `ward.jsonl` — one JSON object per line, for a tool. Filtering a session
//!   by subsystem, or measuring how long turns took, should be a query rather
//!   than an exercise in reading prose.
//!
//! **Nothing leaves the machine.** No analytics, no metrics endpoint, no
//! upload, no service to run. These are files in the data folder and that is
//! all they are.
//!
//! Runtime verbosity control, redaction and crash capture are deliberately not
//! here — they are 1.0.0 work. What is here is the shape they attach to.

use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use time::format_description::BorrowedFormatItem;
use time::macros::format_description;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Fixed at startup so every event can report how far into the session it
/// happened.
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Time since the process started, from a clock that cannot go backwards.
///
/// Wall-clock time can jump - a clock correction or a daylight-saving change
/// will happily hand you two events in the wrong order, or a negative duration
/// between them. Anything that reasons about *when* rather than merely *what*
/// has to use this instead, which is why it is public and not a logging
/// detail: endpointing, barge-in and overlap policy are all timing behavior,
/// and none of that can be retrofitted onto a list that only knows its order.
pub fn since_start() -> Duration {
    START.get_or_init(Instant::now).elapsed()
}

/// Keeps the background writers alive. Dropping it flushes them, so it has to
/// live as long as the application does.
pub struct Guard {
    _human: tracing_appender::non_blocking::WorkerGuard,
    _machine: tracing_appender::non_blocking::WorkerGuard,
}

const WALL: &[BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]");

/// Prints wall-clock time and monotonic offset together.
///
/// Both, because they answer different questions. Wall clock is what lines an
/// event up against something outside the process - a journal entry the game
/// wrote, or the moment a Commander says they saw the problem. The offset is
/// what survives the clock being adjusted underneath a running session.
struct Stamp;

impl FormatTime for Stamp {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> fmt::Result {
        let millis = since_start().as_millis();

        match time::OffsetDateTime::now_utc().format(WALL) {
            Ok(wall) => write!(w, "{wall}Z +{millis}ms"),
            // A clock that will not format is not a reason to lose the event.
            Err(_) => write!(w, "+{millis}ms"),
        }
    }
}

/// Starts logging into `<data_dir>/logs`.
///
/// Returns a guard that must be held for the lifetime of the process.
pub fn init(data_dir: &Path) -> Result<Guard> {
    // Anchor the monotonic clock before anything can record an event, so no
    // event carries a larger offset than one logged after it.
    START.get_or_init(Instant::now);

    let dir = data_dir.join("logs");
    std::fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;

    let (human, human_guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::daily(&dir, "ward.log"));
    let (machine, machine_guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::daily(&dir, "ward.jsonl"));

    // Ward's own subsystems at the given level; everything underneath only
    // when it has something wrong to report.
    //
    // Without the `warn` floor the file is not about Ward. The graphics stack
    // alone logs a full adapter enumeration at info on every start - one event,
    // two kilobytes, eight devices - and it buried the four events worth
    // reading. A diagnostic nobody can find their own events in is a file, not
    // a diagnostic.
    //
    // WARD_LOG overrides it for a debugging session. That is a development
    // affordance, not the in-application control: changing verbosity without a
    // restart is 1.0.0 work.
    let filter = |level: &str| {
        EnvFilter::try_from_env("WARD_LOG")
            .unwrap_or_else(|_| EnvFilter::new(format!("warn,ward={level}")))
    };

    let human_layer = tracing_subscriber::fmt::layer()
        .with_timer(Stamp)
        .with_target(true)
        .with_ansi(false)
        .with_writer(human)
        .with_filter(filter("info"));

    let machine_layer = tracing_subscriber::fmt::layer()
        .json()
        .flatten_event(true)
        .with_timer(Stamp)
        .with_target(true)
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(machine)
        .with_filter(filter("info"));

    let registry = tracing_subscriber::registry()
        .with(human_layer)
        .with(machine_layer);

    // A console exists in debug builds, and reading events there beats opening
    // a file while iterating.
    #[cfg(debug_assertions)]
    let registry = registry.with(
        tracing_subscriber::fmt::layer()
            .with_timer(Stamp)
            .with_target(true)
            .with_writer(std::io::stderr)
            .with_filter(filter("debug")),
    );

    registry.init();

    Ok(Guard {
        _human: human_guard,
        _machine: machine_guard,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_moves_forward_and_never_back() {
        let a = since_start();
        std::thread::sleep(Duration::from_millis(2));
        let b = since_start();
        assert!(b > a, "{b:?} should be later than {a:?}");
    }

    #[test]
    fn the_clock_is_anchored_once() {
        // Two readings taken across an init must share an origin, or events
        // logged before and after would not be comparable.
        let before = since_start();
        START.get_or_init(Instant::now);
        let after = since_start();
        assert!(after >= before);
    }

    #[test]
    fn the_wall_clock_format_is_sortable() {
        // Year first, fixed width, zero padded: sorting the text sorts the
        // events. A locale-shaped date would not.
        let formatted = time::OffsetDateTime::now_utc().format(WALL).unwrap();
        assert_eq!(formatted.len(), "2026-08-10 21:15:03.123".len());
        assert_eq!(&formatted[4..5], "-");
        assert_eq!(&formatted[10..11], " ");
    }
}
