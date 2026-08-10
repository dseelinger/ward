//! The microphone, behind an interface.
//!
//! Two things sit behind [`Ears`] rather than in front of it. The obvious one is
//! testing: the listening loop has to be drivable with no sound card and no
//! Commander. The one that matters more is echo cancellation, which is the
//! unlock for continuous listening and for interrupting Ward naturally. That
//! belongs between the device and the loop, and putting it there later is a
//! change to one implementation rather than a change to the loop.
//!
//! **The device is opened once and never stopped.** Capture runs for as long as
//! Ward does, and the gate decides what is *kept*. Opening a stream takes tens
//! of milliseconds, and a Commander who holds the key and starts talking has
//! already said a syllable by then. A first word lost to setup is lost for good,
//! so there is no setup on that path — only a flag.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// What the speech model expects, so it is what everything above the device
/// speaks in. Converting once, here, means nothing further up has to know what
/// the sound card happened to offer.
pub const RATE: u32 = 16_000;

/// The most audio held for a consumer that has stopped collecting it.
///
/// A bound on a leak rather than a tuning knob: the loop drains this many times
/// a second, so reaching the cap at all means something upstream has stopped,
/// and the right failure is to forget old audio rather than to grow forever.
const MOST_HELD: usize = RATE as usize * 60;

/// Somewhere Commander speech comes from.
///
/// Not [`Send`] on purpose. A capture stream owns operating-system handles that
/// cannot cross threads on Windows, so the device is opened on the thread that
/// listens and stays there.
pub trait Ears {
    /// Everything heard since the last call: one channel, [`RATE`] samples a
    /// second, nominally -1.0 to 1.0.
    ///
    /// Never blocks and never waits for a quantity. An empty return means
    /// nothing arrived in that instant, not that the microphone is finished.
    fn drain(&mut self) -> Vec<f32>;
}

/// Averages a run of input samples into one output sample.
///
/// A box filter and a decimator in one pass. Sound cards commonly offer 44.1 or
/// 48 kHz and the model wants 16, and simply keeping every third sample folds
/// everything above 8 kHz back down into the speech band as a whistle the model
/// then tries to read as a word. Averaging the samples that are dropped is the
/// cheapest thing that does not do that.
///
/// State is carried across calls because audio arrives in whatever sized pieces
/// the device feels like handing over, and a window that straddles two of them
/// still has to come out as one sample.
struct Resampler {
    /// Input samples per output sample. Fractional for rates that do not
    /// divide, which is why the budget below is carried rather than reset.
    step: f64,
    budget: f64,
    sum: f32,
    count: u32,
}

impl Resampler {
    fn new(from: u32) -> Self {
        Self {
            step: from as f64 / RATE as f64,
            budget: from as f64 / RATE as f64,
            sum: 0.0,
            count: 0,
        }
    }

    fn feed(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for &sample in input {
            self.sum += sample;
            self.count += 1;
            self.budget -= 1.0;

            if self.budget <= 0.0 {
                out.push(self.sum / self.count as f32);
                self.sum = 0.0;
                self.count = 0;
                // Added rather than assigned, so the leftover fraction counts
                // toward the next window and the long-run rate stays exact.
                self.budget += self.step;
            }
        }
    }
}

/// Collapses interleaved channels to one.
///
/// Averaged rather than taking the first channel. A headset boom on a stereo
/// input can land entirely on the channel that is not first, and a Commander
/// whose microphone works everywhere else would hear Ward say it heard nothing.
fn to_mono(frame: &[f32], channels: usize) -> f32 {
    if channels <= 1 {
        return frame.first().copied().unwrap_or_default();
    }

    frame.iter().sum::<f32>() / channels as f32
}

/// A real microphone.
pub struct Microphone {
    /// Dropping this stops capture, so it is held even though nothing reads it.
    _stream: cpal::Stream,
    heard: Arc<Mutex<Vec<f32>>>,
}

impl Microphone {
    /// Opens the system's default input device and starts capturing.
    ///
    /// The device is chosen rather than asked about. Letting the Commander pick
    /// one is separate work; what matters here is that the failure is loud —
    /// a silent fallback to a default that captures nothing is the shape of
    /// this bug that costs an afternoon.
    pub fn open() -> Result<Self> {
        let host = cpal::default_host();

        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("no microphone. Windows has no default recording device."))?;

        let name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "an unnamed device".to_string());

        let supported = device
            .default_input_config()
            .with_context(|| format!("could not read the settings of {name}"))?;

        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let channels = config.channels as usize;
        let rate = config.sample_rate;

        let heard: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));

        let collect = {
            let heard = heard.clone();
            let mut resampler = Resampler::new(rate);
            let mut mono: Vec<f32> = Vec::new();

            move |samples: &[f32]| {
                mono.clear();
                mono.extend(samples.chunks(channels).map(|f| to_mono(f, channels)));

                let Ok(mut heard) = heard.lock() else {
                    return;
                };

                resampler.feed(&mono, &mut heard);

                if heard.len() > MOST_HELD {
                    let excess = heard.len() - MOST_HELD;
                    heard.drain(..excess);
                }
            }
        };

        // A device that stops mid-session says so once. It cannot be repaired
        // from inside the callback, and a message every buffer would be a
        // hundred lines a second saying the same thing.
        let complain = |e: cpal::StreamError| {
            tracing::warn!(target: "ward::listen", error = %e, "the microphone stopped");
        };

        // Sound cards disagree about what they hand over. Whatever it is, it
        // leaves this block as f32.
        let stream = match format {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config,
                {
                    let mut collect = collect;
                    move |data: &[f32], _: &_| collect(data)
                },
                complain,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config,
                {
                    let mut collect = collect;
                    let mut scratch: Vec<f32> = Vec::new();
                    move |data: &[i16], _: &_| {
                        scratch.clear();
                        scratch.extend(data.iter().map(|s| *s as f32 / i16::MAX as f32));
                        collect(&scratch);
                    }
                },
                complain,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                &config,
                {
                    let mut collect = collect;
                    let mut scratch: Vec<f32> = Vec::new();
                    move |data: &[u16], _: &_| {
                        scratch.clear();
                        scratch.extend(
                            data.iter()
                                .map(|s| (*s as f32 - 32_768.0) / i16::MAX as f32),
                        );
                        collect(&scratch);
                    }
                },
                complain,
                None,
            ),
            other => {
                return Err(anyhow!(
                    "{name} records in a format Ward cannot read ({other}). \
                     Choose a different recording device in Windows sound settings."
                ));
            }
        }
        .with_context(|| format!("could not start capture on {name}"))?;

        stream
            .play()
            .with_context(|| format!("could not start {name}"))?;

        tracing::info!(
            target: "ward::listen",
            device = %name,
            rate,
            channels,
            format = %format,
            "listening"
        );

        Ok(Self {
            _stream: stream,
            heard,
        })
    }
}

impl Ears for Microphone {
    fn drain(&mut self) -> Vec<f32> {
        match self.heard.lock() {
            Ok(mut heard) => std::mem::take(&mut *heard),
            // The callback panicked while holding it. Capture is over; saying
            // so is the whole of what can be done from here.
            Err(_) => Vec::new(),
        }
    }
}

/// Puts a whole recording through the same conversion capture uses.
///
/// Test-only. The running application does this a buffer at a time inside the
/// callback; this is the identical two steps over a recording that already
/// exists, so a test can put a known utterance through the real conversion
/// rather than an approximation of it.
#[cfg(test)]
pub fn to_model_rate(interleaved: &[f32], from: u32, channels: usize) -> Vec<f32> {
    let mono: Vec<f32> = interleaved
        .chunks(channels.max(1))
        .map(|frame| to_mono(frame, channels))
        .collect();

    let mut out = Vec::new();
    Resampler::new(from).feed(&mono, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resample(from: u32, input: &[f32]) -> Vec<f32> {
        let mut r = Resampler::new(from);
        let mut out = Vec::new();
        r.feed(input, &mut out);
        out
    }

    #[test]
    fn a_second_of_audio_comes_out_a_second_long() {
        // The model is handed a duration as a sample count. Getting the rate
        // wrong does not fail - it transcribes a chipmunk, or a drawl.
        assert_eq!(resample(48_000, &vec![0.0; 48_000]).len(), 16_000);
        assert_eq!(resample(16_000, &vec![0.0; 16_000]).len(), 16_000);
    }

    #[test]
    fn a_rate_that_does_not_divide_still_comes_out_right() {
        // 44100 is 2.75625 input samples per output one. Truncating the
        // fraction would drift, and a long utterance would end up measurably
        // shorter than it was spoken.
        let out = resample(44_100, &vec![0.0; 44_100]);
        assert!(
            (out.len() as i64 - 16_000).abs() <= 1,
            "a second became {} samples",
            out.len()
        );
    }

    #[test]
    fn a_steady_signal_survives_being_resampled() {
        // Averaging must not change the level. If it did, everything would
        // arrive quieter than it was spoken and the too-quiet guard would
        // reject real speech.
        let out = resample(48_000, &vec![0.5; 48_000]);
        assert!(out.iter().all(|s| (*s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn audio_arriving_in_pieces_resamples_the_same_as_in_one_go() {
        // A device hands over whatever sized buffer it likes, and a window that
        // straddles two of them still has to come out as one sample. Without
        // the carried state this drops a sample per buffer, which at fifty
        // buffers a second is audible damage.
        let input: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.001).sin()).collect();

        let whole = resample(48_000, &input);

        let mut r = Resampler::new(48_000);
        let mut piecemeal = Vec::new();
        for piece in input.chunks(731) {
            r.feed(piece, &mut piecemeal);
        }

        assert_eq!(whole.len(), piecemeal.len());
        for (a, b) in whole.iter().zip(piecemeal.iter()) {
            assert!((a - b).abs() < 1e-6, "{a} != {b}");
        }
    }

    #[test]
    fn averaging_the_dropped_samples_removes_what_decimation_would_fold_back() {
        // The reason this is not "keep every third sample." A tone above the
        // half-rate ceiling has nowhere to land but back inside the speech
        // band, where the model reads it as a word nobody said.
        //
        // 12 kHz at 48 kHz is one full cycle every four samples, and would
        // reappear at 4 kHz at its original strength. Three consecutive samples
        // of it sum to a single sample's worth, so averaging leaves a third.
        let tone: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 * std::f32::consts::TAU * 12_000.0 / 48_000.0).sin())
            .collect();

        let folded = tone.iter().step_by(3).copied().collect::<Vec<f32>>();
        let averaged = resample(48_000, &tone);

        let loudest = |s: &[f32]| s.iter().fold(0.0f32, |m, v| m.max(v.abs()));

        assert!(
            loudest(&averaged) < loudest(&folded) / 2.0,
            "averaged {} against decimated {}",
            loudest(&averaged),
            loudest(&folded)
        );
    }

    #[test]
    fn a_boom_on_the_second_channel_is_not_lost() {
        // Taking the first channel is the tempting shortcut. A headset that
        // lands entirely on the right would record silence, and the Commander
        // would be told nothing was said.
        assert!(to_mono(&[0.0, 0.8], 2) > 0.0);
        assert!((to_mono(&[0.4, 0.8], 2) - 0.6).abs() < 1e-6);
        assert_eq!(to_mono(&[0.5], 1), 0.5);
    }

    #[test]
    fn a_frame_with_nothing_in_it_is_silence_rather_than_a_panic() {
        assert_eq!(to_mono(&[], 1), 0.0);
    }
}
