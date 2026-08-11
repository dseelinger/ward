//! Speech, out loud.
//!
//! This is the free voice, and the reason it matters more than its size
//! suggests: it is the half of the zero-cost path that lets somebody try Ward
//! without paying for anything. It also has no official client, so all of it is
//! hand-rolled against a protocol nobody documents for this purpose.
//!
//! That has a consequence worth stating plainly. The service is Microsoft's
//! browser read-aloud endpoint, and it can change without notice: the token
//! scheme below is checked against the current client version, and if the
//! service moves, synthesis starts failing and this file is where the fix goes.
//! Ward is built so that failing here costs the voice and nothing else — the
//! turn still happens, the answer still arrives on screen.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const HOST: &str = "speech.platform.bing.com";
const TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";

/// The browser version the service is told it is talking to. If synthesis
/// starts refusing connections, this is the first thing to check.
const CLIENT: &str = "143.0.3650.75";

/// 1601-01-01 to 1970-01-01, in seconds. The token is derived from a Windows
/// file time, not a Unix one.
const WINDOWS_EPOCH_OFFSET: u64 = 11_644_473_600;

pub const DEFAULT_VOICE: &str = "en-US-AndrewNeural";

/// Audio as the service returns it.
///
/// Compressed, because it is the only format this service reliably returns -
/// asking for uncompressed produced no audio at all. The decoder is chosen for
/// its license: the one the audio library reaches for by default is weak
/// copyleft, which the license gate caught before it could ship.
pub struct Speech {
    pub audio: Vec<u8>,
}

impl Speech {
    /// How long this takes to say.
    ///
    /// Worked out from the size, because the format is fixed and known: the
    /// service is asked for mono at 48 kilobits a second, so the bytes are the
    /// duration. Captions are paced by this rather than by an assumed reading
    /// speed, which is the only way they can keep step with a voice whose pace
    /// they cannot otherwise see.
    pub fn duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(self.audio.len() as f64 * 8.0 / 48_000.0)
    }
}

/// A token the service checks on every connection.
///
/// The timestamp is deliberately rounded down to a five-minute boundary, so a
/// token stays valid for a few minutes and a small clock difference between
/// this machine and the service does not reject every request.
fn access_token(now_unix: u64) -> String {
    let mut ticks = now_unix + WINDOWS_EPOCH_OFFSET;
    ticks -= ticks % 300;

    // Windows file time counts in hundreds of nanoseconds.
    let ticks = ticks as u128 * 10_000_000;

    let mut hasher = Sha256::new();
    hasher.update(format!("{ticks}{TOKEN}").as_bytes());

    let digest = hasher.finalize();

    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02X}");
    }
    hex
}

/// Text destined for a markup document, with the characters that would end it
/// early removed.
///
/// Not decoration: a reply is model output, and model output is text Ward did
/// not write. An unescaped angle bracket in an answer would be read as markup
/// and change what gets spoken, or break synthesis entirely.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\'' => out.push_str("&apos;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

fn ssml(voice: &str, text: &str, rate: &str) -> String {
    format!(
        "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>\
         <voice name='{voice}'>\
         <prosody pitch='+0Hz' rate='{rate}' volume='+0%'>{}</prosody>\
         </voice></speak>",
        escape(text)
    )
}

/// Splits a binary frame into its headers and its audio.
///
/// The service prefixes each frame with a two-byte length and then a block of
/// text headers, so the audio does not start at a fixed offset.
fn split_frame(frame: &[u8]) -> Option<(&[u8], &[u8])> {
    if frame.len() < 2 {
        return None;
    }

    let header_len = u16::from_be_bytes([frame[0], frame[1]]) as usize;

    // A length longer than the frame is a frame that cannot be trusted.
    let end = 2usize.checked_add(header_len)?;
    if end > frame.len() {
        return None;
    }

    Some((&frame[2..end], &frame[end..]))
}

fn is_audio(headers: &[u8]) -> bool {
    // `Path:audio.metadata` also starts with `Path:audio`. Matching the
    // prefix alone would splice timing JSON into the middle of the audio.
    String::from_utf8_lossy(headers).contains("Path:audio\r\n")
}

/// One voice the service offers.
pub struct Voice {
    /// What the setting stores, such as `en-US-AndrewNeural`.
    pub name: String,
    /// What a person reads, such as `Andrew`.
    pub friendly: String,
    pub locale: String,
    pub gender: String,
}

impl Voice {
    /// Everything worth matching a search against, in one string.
    pub fn searchable(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name, self.friendly, self.locale, self.gender
        )
        .to_lowercase()
    }
}

/// Every voice the service will speak in.
///
/// Hundreds of them, which is why choosing one needs browsing and searching
/// rather than a name typed from memory. Fetched rather than compiled in: the
/// list changes, and a stale list offers voices that no longer answer.
pub async fn voices() -> Result<Vec<Voice>> {
    let url = format!(
        "https://{HOST}/consumer/speech/synthesize/readaloud/voices/list\
         ?trustedclienttoken={TOKEN}"
    );

    let listed: serde_json::Value = reqwest::Client::new()
        .get(&url)
        .header(
            "User-Agent",
            format!(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36 Edg/{major}.0.0.0",
                major = CLIENT.split('.').next().unwrap_or("143")
            ),
        )
        .send()
        .await
        .map_err(|e| anyhow!("could not reach the voice service: {e}"))?
        .json()
        .await
        .map_err(|e| anyhow!("could not read the voice list: {e}"))?;

    let mut voices: Vec<Voice> = listed
        .as_array()
        .map(|all| {
            all.iter()
                .filter_map(|v| {
                    let name = v["ShortName"].as_str()?.to_string();

                    // The service's friendly name is a sentence. The part
                    // worth showing is the person's name inside it.
                    let friendly = v["FriendlyName"]
                        .as_str()
                        .and_then(|f| f.split_whitespace().nth(1))
                        .unwrap_or(&name)
                        .to_string();

                    Some(Voice {
                        name,
                        friendly,
                        locale: v["Locale"].as_str().unwrap_or_default().to_string(),
                        gender: v["Gender"].as_str().unwrap_or_default().to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if voices.is_empty() {
        return Err(anyhow!("the voice service returned no voices"));
    }

    voices.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(voices)
}

/// How much of a reply is synthesized before Ward starts talking.
///
/// The first piece is deliberately short and the rest are not. Ward used to
/// synthesize a whole reply before playing a word of it, and the cost was
/// measured rather than guessed: a median of six seconds of silence after the
/// answer was already on screen, and fifteen seconds on a long one. Synthesis
/// runs faster than real time, so the opening sentence was ready in under a
/// second every time and then waited for the rest of the paragraph to be made.
///
/// Why not split every sentence: each piece is its own connection to the
/// service, and a connection costs a few hundred milliseconds. Small first for
/// the fast start, larger afterwards because by then Ward is already speaking
/// and the next piece only has to be ready before the current one runs out.
const FIRST: usize = 120;
const THEN: usize = 320;

/// Where the first sentence in `text` ends, or the end of the text.
///
/// A period inside a figure or an abbreviation has no space after it, and that
/// is the whole rule — it tells "127.32" from the end of a sentence without
/// needing a list of abbreviations to consult, and without a list to keep up to
/// date as Commanders say new things.
fn sentence_end(text: &str) -> usize {
    text.char_indices()
        .find(|(at, c)| {
            matches!(c, '.' | '!' | '?')
                && text[at + c.len_utf8()..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
        })
        .map(|(at, c)| at + c.len_utf8())
        .unwrap_or(text.len())
}

/// The byte offset of the `room`th character, or the end.
fn at_character(text: &str, room: usize) -> usize {
    text.char_indices()
        .nth(room)
        .map(|(at, _)| at)
        .unwrap_or(text.len())
}

/// Where to cut so that what comes off is no longer than `room`.
///
/// A whole sentence when one fits, because that is where a voice pauses anyway.
/// When one does not fit, a clause break — the model writes sentences of three
/// hundred characters, and refusing to cut one meant the first piece was the
/// whole sentence and the fast start was no faster at all.
///
/// A comma is a worse seam than a full stop and a much better one than silence.
/// The pieces are synthesized separately and joined by playing one after
/// another, so the seam is audible wherever it falls; the question is only
/// whether it sounds like a pause or like a stumble.
fn break_at(text: &str, room: usize) -> usize {
    let end = sentence_end(text);

    if text[..end].chars().count() <= room {
        return end;
    }

    let cut = at_character(text, room);

    // A clause break, taken with its punctuation so the pause is spoken.
    if let Some(at) = text[..cut].rfind([',', ';', ':', '\u{2014}']) {
        return at + 1;
    }

    // Failing that, a word boundary. Only a single unbroken run of characters
    // longer than the whole allowance falls past this, and cutting one of those
    // anywhere is equally wrong.
    match text[..cut].rfind(' ') {
        Some(at) => at + 1,
        None => cut,
    }
}

/// Breaks a reply into the pieces that will be spoken, in order.
pub fn into_pieces(text: &str) -> Vec<String> {
    let mut pieces: Vec<String> = Vec::new();
    let mut piece = String::new();
    let mut rest = text.trim();

    while !rest.is_empty() {
        let room = match pieces.is_empty() {
            true => FIRST,
            false => THEN,
        };

        let left = room.saturating_sub(piece.chars().count());

        // What is left of this piece will not hold another sentence, so send it
        // and start the next one.
        if !piece.is_empty() && sentence_end(rest) > at_character(rest, left) {
            pieces.push(std::mem::take(&mut piece));
            continue;
        }

        let (chunk, remainder) = rest.split_at(break_at(rest, room));
        rest = remainder.trim_start();

        if !piece.is_empty() {
            piece.push(' ');
        }
        piece.push_str(chunk.trim());

        if piece.chars().count() >= room {
            pieces.push(std::mem::take(&mut piece));
        }
    }

    if !piece.is_empty() {
        pieces.push(piece);
    }

    pieces
}

/// A handle on speech in progress, so it can be cut off mid-word.
///
/// This is the ducking half of barge-in. A Commander who presses the key while
/// Ward is talking has decided that whatever Ward is saying matters less than
/// what they are about to say, and a companion that finishes its sentence first
/// is a companion you talk over rather than talk to.
///
/// Cloning shares the handle rather than copying it, so the window can stop
/// what a background thread is playing.
#[derive(Clone, Default)]
pub struct Playing {
    slot: std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<rodio::Player>>>>,
    /// Set by an interruption and read between pieces.
    ///
    /// Stopping the sound that is playing is not enough once a reply is spoken
    /// in pieces: without this, the piece being cut off would be followed by
    /// the next one, and Ward would carry on talking through the interruption
    /// one sentence at a time.
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Playing {
    /// A new reply is starting, so an earlier interruption no longer applies.
    pub fn begin(&self) {
        self.stopped
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether the Commander has cut this reply off.
    pub fn cut_off(&self) -> bool {
        self.stopped.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Stops whatever is being said, at once and mid-word.
    ///
    /// Doing nothing is an ordinary outcome: there is usually nothing playing.
    pub fn stop(&self) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::Relaxed);

        // Cloned out from under the lock rather than used beneath it. Holding
        // the lock across the stop would deadlock against the thread that is
        // playing, which is the one thread this has to be able to reach.
        let player = match self.slot.lock() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };

        if let Some(player) = player {
            player.stop();
        }
    }

    fn holds(&self, player: Option<std::sync::Arc<rodio::Player>>) {
        match self.slot.lock() {
            Ok(mut slot) => *slot = player,
            Err(poisoned) => *poisoned.into_inner() = player,
        }
    }
}

/// Which device Ward will speak through, or why it cannot.
///
/// Opens it rather than asking whether one exists, because the failure worth
/// catching is a device that is listed and will not start.
pub fn speakers() -> Result<String> {
    let device = rodio::DeviceSinkBuilder::open_default_sink().context("no audio output device")?;
    let _ = device.mixer();
    Ok("the default output device".to_string())
}

/// Plays audio and returns when it has finished, or when it was cut off.
///
/// Blocking, and called from a thread set aside for it. Speaking is the one
/// thing Ward does that takes as long as it takes, and hurrying the caller
/// along would only mean starting the next line over the top of this one.
pub fn play(audio: Vec<u8>, playing: &Playing) -> Result<()> {
    let device = rodio::DeviceSinkBuilder::open_default_sink().context("no audio output device")?;

    let player = std::sync::Arc::new(
        rodio::play(device.mixer(), std::io::Cursor::new(audio))
            .context("could not decode the speech")?,
    );

    // Registered before the wait, not after. An interruption arriving in the
    // gap between starting and being reachable would find nothing to stop, and
    // Ward would talk over the Commander for the length of a reply.
    playing.holds(Some(player.clone()));

    // Sound is leaving the speaker from here. Everything before it is a wait
    // the Commander is sitting through, so this is the end of the interval
    // worth measuring rather than the moment synthesis returned.
    tracing::info!(target: "ward::voice", "sound out");

    player.sleep_until_end();

    playing.holds(None);

    Ok(())
}

/// Turns text into speech, or explains why it could not.
pub async fn synthesize(text: &str, voice: &str, rate: &str) -> Result<Speech> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();

    // Identifies this connection to the service. It only has to be unique and
    // shaped like a browser's, which is what the timestamp and hash give it.
    let connection_id = access_token(now.wrapping_mul(2_654_435_761))[..32].to_string();

    let url = format!(
        "wss://{HOST}/consumer/speech/synthesize/readaloud/edge/v1\
         ?TrustedClientToken={TOKEN}\
         &ConnectionId={connection_id}\
         &Sec-MS-GEC={}\
         &Sec-MS-GEC-Version=1-{CLIENT}",
        access_token(now)
    );

    let mut request = url
        .into_client_request()
        .context("could not build the synthesis request")?;

    {
        let headers = request.headers_mut();
        headers.insert("Pragma", "no-cache".parse()?);
        headers.insert("Cache-Control", "no-cache".parse()?);
        headers.insert(
            "Origin",
            "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold".parse()?,
        );
        headers.insert(
            "User-Agent",
            format!(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36 Edg/{major}.0.0.0",
                major = CLIENT.split('.').next().unwrap_or("143")
            )
            .parse()?,
        );
    }

    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| anyhow!("could not reach the voice service: {e}"))?;

    let timestamp = "Thu Jan 01 1970 00:00:00 GMT+0000 (Coordinated Universal Time)";

    socket
        .send(Message::Text(
            format!(
                "X-Timestamp:{timestamp}\r\n\
                 Content-Type:application/json; charset=utf-8\r\n\
                 Path:speech.config\r\n\r\n\
                 {{\"context\":{{\"synthesis\":{{\"audio\":{{\
                 \"metadataoptions\":{{\"sentenceBoundaryEnabled\":\"false\",\
                 \"wordBoundaryEnabled\":\"false\"}},\
                 \"outputFormat\":\"audio-24khz-48kbitrate-mono-mp3\"}}}}}}}}\r\n"
            )
            .into(),
        ))
        .await
        .context("could not configure synthesis")?;

    socket
        .send(Message::Text(
            format!(
                "X-RequestId:{connection_id}\r\n\
                 Content-Type:application/ssml+xml\r\n\
                 X-Timestamp:{timestamp}Z\r\n\
                 Path:ssml\r\n\r\n{}",
                ssml(voice, text, rate)
            )
            .into(),
        ))
        .await
        .context("could not send the text to be spoken")?;

    let mut audio = Vec::new();

    while let Some(message) = socket.next().await {
        match message.map_err(|e| anyhow!("the voice connection dropped: {e}"))? {
            Message::Binary(frame) => {
                if let Some((_, chunk_audio)) =
                    split_frame(&frame).filter(|(headers, _)| is_audio(headers))
                {
                    audio.extend_from_slice(chunk_audio);
                }
            }
            Message::Text(text) => {
                if text.contains("Path:turn.end") {
                    break;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    let _ = socket.close(None).await;

    if audio.is_empty() {
        return Err(anyhow!(
            "the voice service returned no audio. It may have changed; \
             Ward will keep answering in text."
        ));
    }

    Ok(Speech { audio })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_a_sha256_hex_digest() {
        let token = access_token(1_700_000_000);
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(token, token.to_uppercase(), "the service expects uppercase");
    }

    #[test]
    fn a_token_holds_still_within_a_five_minute_window() {
        // Rounding down to a boundary is what lets a small clock difference
        // between this machine and the service go unnoticed. Without it, every
        // request would be a fresh gamble on the two agreeing.
        let base = 1_700_000_400; // exactly on a boundary
        assert_eq!(access_token(base), access_token(base + 299));
    }

    #[test]
    fn a_token_changes_across_a_boundary() {
        let base = 1_700_000_400;
        assert_ne!(access_token(base), access_token(base + 300));
    }

    #[test]
    fn markup_characters_in_a_reply_cannot_change_what_is_spoken() {
        // A reply is model output, which is text Ward did not write. An
        // unescaped bracket would be read as markup and change the utterance
        // or break synthesis outright.
        let hostile = "5 < 6 & \"quoted\" <break time='9999ms'/>";
        let document = ssml("v", hostile, "+0%");

        assert!(!document.contains("<break"), "markup survived: {document}");
        assert!(document.contains("&lt;"));
        assert!(document.contains("&amp;"));
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        assert_eq!(
            escape("Sol is 0 light years away."),
            "Sol is 0 light years away."
        );
    }

    #[test]
    fn the_document_names_the_voice_and_rate() {
        let document = ssml("en-US-AndrewNeural", "hello", "+10%");
        assert!(document.contains("name='en-US-AndrewNeural'"));
        assert!(document.contains("rate='+10%'"));
    }

    #[test]
    fn a_frame_splits_into_headers_and_audio() {
        let headers = b"Path:audio\r\n\r\n";
        let mut frame = (headers.len() as u16).to_be_bytes().to_vec();
        frame.extend_from_slice(headers);
        frame.extend_from_slice(&[0xFF, 0xFB, 0x90]);

        let (h, audio) = split_frame(&frame).unwrap();
        assert!(is_audio(h));
        assert_eq!(audio, &[0xFF, 0xFB, 0x90]);
    }

    #[test]
    fn a_frame_claiming_more_header_than_it_has_is_refused() {
        // Trusting the length would read past the end of the buffer.
        let frame = [0xFF, 0xFF, 0x00, 0x01];
        assert!(split_frame(&frame).is_none());
    }

    #[test]
    fn a_frame_too_short_to_have_a_length_is_refused() {
        assert!(split_frame(&[0x00]).is_none());
        assert!(split_frame(&[]).is_none());
    }

    /// Reaches the live service. Permanently ignored, never run by CI: tiers
    /// that talk to the network must not be part of a gate, and an accidental
    /// call from a test run is a call nobody meant to make.
    ///
    /// Run deliberately with `cargo test -- --ignored`. This is the only thing
    /// that can tell you the protocol still works, because the service can
    /// change without notice and nothing local would notice.
    #[tokio::test]
    #[ignore = "reaches the live voice service"]
    async fn the_live_service_returns_speech() {
        let speech = synthesize("Ward is speaking.", DEFAULT_VOICE, "+0%")
            .await
            .expect("synthesis failed");

        assert!(
            speech.audio.len() > 1000,
            "suspiciously little audio: {} bytes",
            speech.audio.len()
        );

        // An MPEG audio frame opens with eleven set bits, or the stream begins
        // with an identification tag. Anything else means what came back was
        // not audio, whatever its length.
        let head = &speech.audio[..3];
        let framed = head[0] == 0xFF && (head[1] & 0xE0) == 0xE0;
        let tagged = head == b"ID3";

        assert!(framed || tagged, "not audio: {head:02X?}");
    }

    /// Synthesizes and plays, audibly, through the same path a reply takes.
    /// Permanently ignored: it needs a sound device and a person to hear it.
    #[tokio::test]
    #[ignore = "needs an audio device and somebody listening"]
    async fn ward_can_be_heard() {
        let speech = synthesize(
            "Issue six has landed. I can speak now, through my own voice, not a script.              Nothing needed from you. Carrying on with seven through ten.",
            DEFAULT_VOICE,
            "+0%",
        )
        .await
        .expect("synthesis failed");

        let playing = Playing::default();

        tokio::task::spawn_blocking(move || play(speech.audio, &playing))
            .await
            .expect("playback thread failed")
            .expect("playback failed");
    }

    #[test]
    fn one_long_sentence_is_still_broken_up() {
        // The bug this replaced a passing test for. The model writes sentences
        // of three hundred characters, and a splitter that only cuts between
        // sentences made the whole first one the first piece - so the fast
        // start measured five and a half seconds, exactly as slow as before.
        let one_sentence = "The Thargoids are an ancient alien species that                             humanity first clashed with centuries ago, went                             dormant for a very long time afterwards, and then                             began resurfacing a few years back to attack                             outposts and infest systems across the bubble.";

        assert_eq!(
            sentence_end(one_sentence),
            one_sentence.len(),
            "not one sentence"
        );

        let pieces = into_pieces(one_sentence);

        assert!(
            pieces.len() > 1,
            "a single long sentence was not broken: {pieces:?}"
        );
        assert!(
            pieces[0].chars().count() <= FIRST,
            "the first piece is {} characters: {:?}",
            pieces[0].chars().count(),
            pieces[0]
        );
    }

    #[test]
    fn a_forced_break_falls_on_a_clause_rather_than_mid_word() {
        let one_sentence = "The Thargoids are an ancient alien species that                             humanity first clashed with centuries ago, went                             dormant for a very long time afterwards, and then                             began resurfacing.";

        let first = &into_pieces(one_sentence)[0];
        let last = first.trim().chars().last().unwrap();

        assert!(
            matches!(last, '.' | '!' | '?' | ',' | ';' | ':'),
            "cut mid-clause: {first:?}"
        );
    }

    #[test]
    fn the_first_thing_ward_says_is_short() {
        // The whole point. A reply is not synthesized before it is started, so
        // the opening piece has to be small enough to be ready quickly.
        let reply = "Your jump range is forty two light years. That is with a                      full tank and no cargo. Dropping the cargo rack would take                      it to forty five, and stripping the shield another two on                      top of that, which is rarely worth it.";

        let pieces = into_pieces(reply);

        assert!(pieces.len() > 1, "it was not broken up at all: {pieces:?}");
        assert!(
            pieces[0].chars().count() <= FIRST + 40,
            "the first piece is {} characters: {:?}",
            pieces[0].chars().count(),
            pieces[0]
        );
    }

    #[test]
    fn nothing_is_lost_or_repeated_in_the_breaking_up() {
        // The pieces are played one after another, so together they have to be
        // the reply and nothing else.
        let reply = "First. Second! Third? And a fourth that runs on without                      any punctuation to speak of";

        let joined = into_pieces(reply).join(" ");
        let flatten = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");

        assert_eq!(flatten(&joined), flatten(reply));
    }

    #[test]
    fn a_break_falls_where_a_speaker_would_pause() {
        // Each piece is synthesized separately and they are joined by playing
        // one after another, so a seam inside a clause sounds like a stumble.
        for piece in into_pieces(
            "Docking now. Watch the pad number, it is easy to miss.              The station is busy tonight and the traffic is stacked.",
        ) {
            let last = piece.trim().chars().last().unwrap();
            assert!(
                matches!(last, '.' | '!' | '?'),
                "a piece ended mid-sentence: {piece:?}"
            );
        }
    }

    #[test]
    fn a_number_does_not_end_a_sentence() {
        // Tested on the rule itself rather than through the splitting. Two
        // short sentences end up in one piece whichever way the boundary is
        // found, so asking how many pieces came out proves nothing about
        // whether a figure was mistaken for a full stop.
        let text = "The distance is 127.32 light years exactly. Bring fuel.";
        assert_eq!(
            &text[..sentence_end(text)],
            "The distance is 127.32 light years exactly."
        );
    }

    #[test]
    fn a_sentence_ends_where_a_sentence_ends() {
        assert_eq!(
            sentence_end("Docking now. Watch the pad."),
            "Docking now.".len()
        );
        assert_eq!(sentence_end("Are you sure? I am."), "Are you sure?".len());
        assert_eq!(
            sentence_end("No punctuation at all"),
            "No punctuation at all".len()
        );
    }

    #[test]
    fn a_one_sentence_answer_is_one_piece() {
        // Nothing gained by splitting what is already short, and every piece
        // costs a connection.
        assert_eq!(
            into_pieces("Forty two light years."),
            vec!["Forty two light years."]
        );
    }

    /// Times the things between synthesis finishing and sound coming out.
    ///
    /// Permanently ignored: it opens the real audio device on this machine.
    /// Run deliberately when the wait before Ward speaks does not match what
    /// the logs say it should be.
    #[test]
    #[ignore = "opens the real audio device"]
    fn how_long_does_it_take_to_start_making_a_noise() {
        for attempt in 1..=3 {
            let began = std::time::Instant::now();
            let device = rodio::DeviceSinkBuilder::open_default_sink();
            let opened = began.elapsed();

            let device = device.expect("no audio output device");
            let mixed = began.elapsed();
            let _ = device.mixer();

            println!(
                "attempt {attempt}: opening the device {} ms, ready to play at {} ms",
                opened.as_millis(),
                mixed.as_millis()
            );
        }
    }

    #[test]
    fn stopping_nothing_is_an_ordinary_outcome() {
        // Barge-in fires whenever the key goes down, and most of the time Ward
        // is not talking. That path has to be silent rather than exceptional.
        Playing::default().stop();
    }

    #[test]
    fn a_non_audio_frame_contributes_nothing() {
        let headers = b"Path:audio.metadata\r\n\r\n";
        let mut frame = (headers.len() as u16).to_be_bytes().to_vec();
        frame.extend_from_slice(headers);
        frame.extend_from_slice(b"{}");

        let (h, _) = split_frame(&frame).unwrap();
        assert!(!is_audio(h), "metadata was mistaken for audio");
    }
}
