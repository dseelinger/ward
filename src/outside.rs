//! The line between Ward and everything that is not Ward.
//!
//! Two things in this program reach outside the process and cost something when
//! they do: a connection, which spends money, and the speakers, which make a
//! noise in a room. Neither belongs in a test that did not ask for it.
//!
//! The reason this is worth a module rather than care is that Ward is built by
//! an agent running unattended. A test that reaches a live provider spends money
//! while nobody is watching, on top of whatever the run itself costs — two
//! spending surfaces, one of them unsupervised. A test that opens the speakers
//! at three in the morning is a smaller problem and the same mistake.
//!
//! So every call site that goes outside says so first. In a normal build that
//! is nothing at all: the check compiles only into tests, so there is no
//! scaffolding in the program the Commander runs. In a test build it fails
//! unless the test said it meant it.

/// Says that something is about to leave the process.
///
/// `to` is named in the failure, because the useful thing to know is which of
/// them a test reached rather than that it reached one.
pub fn going_out(to: &str) {
    let _ = to;

    #[cfg(test)]
    PERMITTED.with(|permitted| {
        assert!(
            permitted.get(),
            "a test reached {to}, which costs money or makes a noise. If it \
             meant to, hold a permit from outside::deliberately() and mark the \
             test #[ignore] with a reason. If it did not, that is the bug."
        );
    });
}

#[cfg(test)]
thread_local! {
    /// Per thread rather than per process, because tests run in parallel and a
    /// permit shared between them would let one test's intent excuse another
    /// test's accident.
    ///
    /// This holds for the tests that need it because they are all
    /// single-threaded: an async test runs its future on the thread that
    /// started it unless it asks for otherwise. A multi-threaded test that
    /// genuinely wants the wire will fail loudly here rather than quietly pass,
    /// which is the right way round.
    static PERMITTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Permission for this thread to reach outside, until it is dropped.
#[cfg(test)]
pub struct Permit(());

#[cfg(test)]
impl Drop for Permit {
    fn drop(&mut self) {
        PERMITTED.with(|permitted| permitted.set(false));
    }
}

/// Says that this test means to reach outside the process.
///
/// Hold the returned value for as long as that is true. Dropping it closes the
/// door again, so one test's intent cannot outlive it on a reused thread.
#[cfg(test)]
pub fn deliberately() -> Permit {
    PERMITTED.with(|permitted| permitted.set(true));
    Permit(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "a test reached")]
    fn going_outside_without_saying_so_fails_the_test() {
        going_out("somewhere");
    }

    #[test]
    fn a_test_that_means_it_is_let_through() {
        let _permit = deliberately();
        going_out("somewhere");
    }

    // The three below test the wiring rather than the mechanism. The mechanism
    // working is worth nothing if a call site forgets to say it is leaving, and
    // a call site is exactly the kind of thing a later change adds without
    // remembering this file exists.
    //
    // Each of these reaches the guard before it reaches a socket or a device,
    // so none of them costs anything to run.

    #[tokio::test]
    #[should_panic(expected = "the voice service")]
    async fn synthesis_is_behind_the_guard() {
        let _ = crate::voice::synthesize("hello", crate::voice::DEFAULT_VOICE, "+0%").await;
    }

    #[tokio::test]
    #[should_panic(expected = "the model service")]
    async fn the_model_service_is_behind_the_guard() {
        let _ = crate::anthropic::models("not-a-key").await;
    }

    #[test]
    #[should_panic(expected = "the speakers")]
    fn the_speakers_are_behind_the_guard() {
        let _ = crate::voice::play(Vec::new(), &crate::voice::Playing::default());
    }

    #[test]
    fn permission_does_not_outlive_the_test_that_asked_for_it() {
        // Threads are reused between tests. A permit that leaked would excuse
        // the next accident on the same thread, and the symptom would be this
        // guard silently doing nothing.
        {
            let _permit = deliberately();
            going_out("somewhere");
        }

        assert!(
            std::panic::catch_unwind(|| going_out("somewhere")).is_err(),
            "the permit outlived its scope"
        );
    }
}
