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
pub struct Speech {
    pub mp3: Vec<u8>,
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

/// Plays audio and returns when it has finished.
///
/// Blocking, and called from a thread set aside for it. Speaking is the one
/// thing Ward does that takes as long as it takes, and hurrying the caller
/// along would only mean starting the next line over the top of this one.
pub fn play(mp3: Vec<u8>) -> Result<()> {
    let device = rodio::DeviceSinkBuilder::open_default_sink().context("no audio output device")?;

    let player = rodio::play(device.mixer(), std::io::Cursor::new(mp3))
        .context("could not decode the speech")?;

    player.sleep_until_end();

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

    let mut mp3 = Vec::new();

    while let Some(message) = socket.next().await {
        match message.map_err(|e| anyhow!("the voice connection dropped: {e}"))? {
            Message::Binary(frame) => {
                if let Some((_, audio)) =
                    split_frame(&frame).filter(|(headers, _)| is_audio(headers))
                {
                    mp3.extend_from_slice(audio);
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

    if mp3.is_empty() {
        return Err(anyhow!(
            "the voice service returned no audio. It may have changed; \
             Ward will keep answering in text."
        ));
    }

    Ok(Speech { mp3 })
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
            speech.mp3.len() > 1000,
            "suspiciously little audio: {} bytes",
            speech.mp3.len()
        );

        // An MPEG audio frame begins with eleven set bits, or the stream opens
        // with an identification tag. Anything else means what came back was
        // not audio, whatever its length.
        let head = &speech.mp3[..3];
        let framed = head[0] == 0xFF && (head[1] & 0xE0) == 0xE0;
        let tagged = &head[..3] == b"ID3";

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

        tokio::task::spawn_blocking(move || play(speech.mp3))
            .await
            .expect("playback thread failed")
            .expect("playback failed");
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
