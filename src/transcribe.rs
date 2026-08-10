//! Turning captured audio into words.
//!
//! The model runs on this machine and nothing is uploaded. That is not a
//! privacy flourish — it is what makes the free path real, and it is why the
//! speech model is a file the Commander has rather than a service they pay for.
//!
//! Behind a trait for the same reason the microphone is: the listening loop has
//! to be testable without half a gigabyte of model weights, and right-sizing
//! the model is a Commander's hardware decision rather than a fixed choice.
//!
//! **What the model hears is not what it prints.** Speech models trained on
//! captioned video learn to describe sounds as well as transcribe them, so
//! silence and room noise come back as `[BLANK_AUDIO]`, `(wind blowing)` or
//! `*sighs*`. None of that is something the Commander said, and sending it to
//! the model as though it were would have Ward answer a question nobody asked.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Something that turns speech into text.
pub trait Transcriber {
    /// Reads an utterance. One channel, [`crate::mic::RATE`] samples a second.
    ///
    /// An empty result means the model heard nothing worth reporting, which is
    /// an ordinary outcome and not an error.
    fn read(&mut self, audio: &[f32]) -> Result<String>;
}

/// How many cores the model may use.
///
/// Capped deliberately. Elite is running on this machine and wants the
/// processor too, and a transcription that finishes fifty milliseconds sooner
/// at the cost of a frame hitch in the cockpit is a bad trade.
fn threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get().clamp(1, 4))
        .unwrap_or(2) as i32
}

/// Removes what the model wrote *about* the audio rather than *from* it.
///
/// Bracketed and asterisked annotations are sound descriptions, not speech.
/// Left in, a held key over a quiet room becomes Ward being asked to respond to
/// the word "blank audio".
pub fn only_speech(raw: &str) -> String {
    // Asterisks are only treated as a wrapper when they come in pairs. An odd
    // one is arithmetic or emphasis the Commander actually said, and a naive
    // toggle would swallow everything after it to the end of the line.
    let paired = raw.matches('*').count().is_multiple_of(2);

    let mut out = String::with_capacity(raw.len());
    let mut depth = 0usize;
    let mut starred = false;

    for c in raw.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            '*' if paired => starred = !starred,
            _ if depth == 0 && !starred => out.push(c),
            _ => {}
        }
    }

    // Whatever survived may now have a hole in the middle of it.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A speech model loaded from a file.
pub struct Whisper {
    state: whisper_rs::WhisperState,
}

impl Whisper {
    /// Loads the model. Slow, and done once.
    ///
    /// The message names the path, because the overwhelmingly likely reason to
    /// be here is that the file is somewhere other than where Ward looked.
    pub fn load(model: &Path) -> Result<Self> {
        if !model.is_file() {
            return Err(anyhow!(
                "no speech model at {}. Ward cannot hear you until one is there; \
                 point the \"speech model\" setting at the file.",
                model.display()
            ));
        }

        // whisper.cpp writes its own progress to standard output otherwise,
        // which in a windowed build goes nowhere and in a debug build buries
        // everything else.
        whisper_rs::install_logging_hooks();

        let path = model
            .to_str()
            .ok_or_else(|| anyhow!("the path to the speech model is not readable text"))?;

        let started = std::time::Instant::now();

        let context = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .with_context(|| format!("could not load the speech model at {}", model.display()))?;

        let state = context
            .create_state()
            .context("could not prepare the speech model")?;

        tracing::info!(
            target: "ward::listen",
            model = %model.display(),
            ms = started.elapsed().as_millis() as u64,
            "speech model loaded"
        );

        Ok(Self { state })
    }
}

impl Transcriber for Whisper {
    fn read(&mut self, audio: &[f32]) -> Result<String> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        params.set_n_threads(threads());
        params.set_language(Some("en"));
        params.set_translate(false);
        // Each utterance stands alone. Carrying context between them lets one
        // misheard sentence steer the next, and a push-to-talk turn is a
        // complete thought by construction.
        params.set_no_context(true);
        params.set_no_timestamps(true);
        params.set_suppress_blank(true);
        // Nothing of whisper.cpp's own reaches a stream Ward does not own.
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        let started = std::time::Instant::now();

        self.state
            .full(params, audio)
            .context("the speech model could not read that")?;

        let mut raw = String::new();
        for segment in self.state.as_iter() {
            if let Ok(text) = segment.to_str_lossy() {
                raw.push_str(&text);
            }
        }

        let text = only_speech(&raw);

        tracing::info!(
            target: "ward::listen",
            ms = started.elapsed().as_millis() as u64,
            seconds = audio.len() as f32 / crate::mic::RATE as f32,
            chars = text.len(),
            "transcribed"
        );

        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_description_of_the_room_is_not_something_the_commander_said() {
        // Every one of these is a real thing whisper emits over quiet audio. A
        // held key in a silent room must not become a question.
        assert_eq!(only_speech("[BLANK_AUDIO]"), "");
        assert_eq!(only_speech(" (wind blowing) "), "");
        assert_eq!(only_speech("*sighs*"), "");
        assert_eq!(only_speech("[ Silence ]"), "");
    }

    #[test]
    fn speech_around_an_annotation_survives_it() {
        assert_eq!(
            only_speech(" What is my jump range? [BLANK_AUDIO]"),
            "What is my jump range?"
        );
        assert_eq!(
            only_speech("Plot a course (coughs) to Sol."),
            "Plot a course to Sol."
        );
    }

    #[test]
    fn ordinary_speech_is_left_alone() {
        assert_eq!(
            only_speech(" How far is Deciat from here? "),
            "How far is Deciat from here?"
        );
    }

    #[test]
    fn segments_run_together_are_spaced_apart() {
        // The model returns one string per segment, each with its own leading
        // space, and concatenating them naively produces double spaces mid
        // sentence.
        assert_eq!(
            only_speech(" Take me to Shinrarta. And honk on arrival."),
            "Take me to Shinrarta. And honk on arrival."
        );
    }

    #[test]
    fn a_lone_asterisk_does_not_swallow_the_sentence() {
        // An unpaired marker means the model was not annotating, so it is a
        // character the Commander said and everything after it has to survive.
        // A naive toggle would drop "3 grid" and nobody would know why.
        assert_eq!(only_speech("that is a 5 * 3 grid"), "that is a 5 * 3 grid");
    }

    #[test]
    fn the_model_uses_some_of_the_machine_and_not_all_of_it() {
        // Elite wants the processor too. A transcription that finishes slightly
        // sooner at the cost of a hitch in the cockpit is a bad trade.
        let n = threads();
        assert!((1..=4).contains(&n), "asked for {n} threads");
    }

    /// Where the model is on the machine running this.
    ///
    /// Not a default, and deliberately not read from settings: this test is run
    /// by hand, and half a gigabyte of weights lives wherever the person
    /// running it put them. Absent, it fails rather than skipping - a hardware
    /// test that quietly passes because it could not run is worse than no test.
    fn model_on_this_machine() -> std::path::PathBuf {
        std::env::var("WARD_SPEECH_MODEL")
            .map(std::path::PathBuf::from)
            .expect("set WARD_SPEECH_MODEL to the ggml model file to run this")
    }

    /// The whole hearing path, with nobody speaking.
    ///
    /// Ward says a sentence through its own voice, and reads it back through
    /// its own ears - synthesis, decoding, the downmix and resampling the
    /// capture callback does, the model, and the annotation stripping. Nothing
    /// here is a stand-in except the room.
    ///
    /// Permanently ignored: it needs the model file and reaches the live voice
    /// service. Run it deliberately. It is the only thing that answers "does
    /// Ward actually hear words" without a person and a microphone.
    #[tokio::test]
    #[ignore = "needs the speech model and reaches the live voice service"]
    async fn ward_reads_back_a_sentence_it_spoke() {
        use rodio::Source;

        let said = "What is my jump range, Ward?";

        let speech = crate::voice::synthesize(said, crate::voice::DEFAULT_VOICE, "+0%")
            .await
            .expect("synthesis failed");

        let decoded = rodio::Decoder::new(std::io::Cursor::new(speech.audio))
            .expect("the spoken audio could not be decoded");

        let rate = decoded.sample_rate().get();
        let channels = decoded.channels().get() as usize;
        let interleaved: Vec<f32> = decoded.collect();

        let audio = crate::mic::to_model_rate(&interleaved, rate, channels);

        assert!(
            audio.len() > crate::mic::RATE as usize / 2,
            "less than half a second of audio to read"
        );

        let heard = Whisper::load(&model_on_this_machine())
            .expect("the model would not load")
            .read(&audio)
            .expect("transcription failed");

        // Punctuation and capitalization are the model's own choices and are
        // not what is being checked. The words are.
        let plain = |s: &str| {
            s.to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric() || c.is_whitespace())
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };

        assert_eq!(plain(&heard), plain(said), "heard: {heard:?}");
    }

    #[test]
    fn a_missing_model_says_where_it_looked() {
        let Err(e) = Whisper::load(Path::new("nowhere/ggml-small.en.bin")) else {
            panic!("a model that is not there loaded anyway");
        };

        let err = e.to_string();
        assert!(err.contains("ggml-small.en.bin"), "got: {err}");
        assert!(err.contains("speech model"), "got: {err}");
    }
}
