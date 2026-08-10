//! The model client.
//!
//! Hand-rolled, because there is no official Rust SDK. The surface kept here is
//! deliberately small — send a conversation, receive a stream of events — so
//! that the loop above it never learns anything provider-shaped. The moment an
//! interface like this grows provider-specific parameters, the abstraction
//! stops paying for itself.

use anyhow::{Result, anyhow};
use futures_util::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::sse;

const API: &str = "https://api.anthropic.com/v1/messages";
const VERSION: &str = "2023-06-01";

pub const DEFAULT_MODEL: &str = "claude-opus-5";

/// Caps thinking and reply text **together**, so this cannot be tuned down for
/// latency without risking a truncated answer mid-sentence.
const MAX_TOKENS: u32 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    fn wire(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Message {
    pub role: Role,
    pub text: String,
}

/// What the caller sees. Thinking is deliberately absent: the model reasons
/// before answering, and that reasoning is not the answer. Passing it on as
/// reply text would put it on screen and, later, into a voice.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A fragment of the reply, in order.
    Text(String),
    /// The turn ended. `stop_reason` is `refusal` when the model declined,
    /// which arrives as a perfectly ordinary success with little or no content.
    Done { stop_reason: Option<String> },
    /// The turn failed. Carries something a person can read.
    Failed(String),
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    api_key: String,
    model: String,
}

/// Written by hand so the key cannot be printed. Deriving `Debug` here would
/// put it into the first error message that formatted a `Client`, and into
/// whatever file that message landed in. Redaction is a safety net for what
/// leaks by accident; this is simply not having the accident available.
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("model", &self.model)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl Client {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key,
            model,
        }
    }

    /// Streams one turn, sending events as they arrive.
    ///
    /// Never returns an error to the caller: a failure is delivered as
    /// `Event::Failed` on the same channel as everything else, so a caller has
    /// exactly one place to watch and cannot leave the user staring at a
    /// half-finished reply with no explanation.
    pub async fn stream(&self, history: &[Message], out: UnboundedSender<Event>) {
        if let Err(e) = self.stream_inner(history, &out).await {
            let _ = out.send(Event::Failed(e.to_string()));
        }
    }

    async fn stream_inner(&self, history: &[Message], out: &UnboundedSender<Event>) -> Result<()> {
        let started = std::time::Instant::now();

        tracing::debug!(
            target: "ward::model",
            model = %self.model,
            messages = history.len(),
            "requesting"
        );

        let body = Request {
            model: &self.model,
            max_tokens: MAX_TOKENS,
            stream: true,
            system: SYSTEM,
            // Effort is the latency lever. Thinking itself stays on: with it
            // disabled the model can write a tool call into its visible text
            // instead of making the call, and the turn then succeeds while
            // nothing runs and nothing errors. For something that speaks its
            // answers aloud, that is claiming an action it never took.
            output_config: OutputConfig { effort: "low" },
            messages: history
                .iter()
                .map(|m| WireMessage {
                    role: m.role.wire(),
                    content: &m.text,
                })
                .collect(),
        };

        let response = self
            .http
            .post(API)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow!("could not reach the model: {e}"))?;

        let status = response.status();

        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            let explained = explain(status, &detail);
            tracing::warn!(
                target: "ward::model",
                status = status.as_u16(),
                error = %explained,
                "request rejected"
            );
            return Err(anyhow!("{explained}"));
        }

        // Time to first byte is the number that decides whether a spoken reply
        // feels like conversation, so it is measured from the first turn
        // rather than added once it becomes a problem.
        tracing::debug!(
            target: "ward::model",
            ms = started.elapsed().as_millis() as u64,
            "response opened"
        );

        let mut parser = sse::Parser::new();
        let mut stream = response.bytes_stream();
        let mut stop_reason = None;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow!("the connection dropped mid-reply: {e}"))?;
            let text = String::from_utf8_lossy(&chunk).into_owned();

            for payload in parser.push(&text) {
                match handle(&payload, &mut stop_reason) {
                    Some(Event::Failed(m)) => return Err(anyhow!("{m}")),
                    Some(event) => {
                        // A closed receiver means the window went away. Stop
                        // rather than carrying on producing tokens nobody will
                        // ever read.
                        if out.send(event).is_err() {
                            return Ok(());
                        }
                    }
                    None => {}
                }
            }
        }

        tracing::info!(
            target: "ward::model",
            ms = started.elapsed().as_millis() as u64,
            stop_reason = stop_reason.as_deref().unwrap_or("none"),
            "reply complete"
        );

        let _ = out.send(Event::Done { stop_reason });

        Ok(())
    }
}

/// Interprets one event payload. Unknown event types return `None` — the
/// protocol is explicitly allowed to grow, and a client that fails on an event
/// it has not been taught about breaks the day the server adds one.
fn handle(payload: &str, stop_reason: &mut Option<String>) -> Option<Event> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;

    match v.get("type")?.as_str()? {
        "content_block_delta" => {
            let delta = v.get("delta")?;
            // `thinking_delta` and `signature_delta` also arrive here and are
            // deliberately dropped; only text is the reply.
            if delta.get("type")?.as_str()? != "text_delta" {
                return None;
            }
            Some(Event::Text(delta.get("text")?.as_str()?.to_string()))
        }
        "message_delta" => {
            if let Some(reason) = v
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(|r| r.as_str())
            {
                *stop_reason = Some(reason.to_string());
            }
            None
        }
        "error" => {
            let message = v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("the model reported an error with no detail");
            Some(Event::Failed(message.to_string()))
        }
        _ => None,
    }
}

/// Turns a failed response into something worth reading. The status alone is
/// not enough: 401 and 400 mean quite different things to somebody who has just
/// pasted a key for the first time.
fn explain(status: reqwest::StatusCode, detail: &str) -> String {
    let hint = match status.as_u16() {
        401 => "the API key was rejected — check it was pasted whole",
        403 => "that key is not permitted to use this model",
        404 => "no such model",
        429 => "rate limited — wait a moment and try again",
        500..=599 => "the service is having trouble; this is not something you did",
        _ => "",
    };

    let message = serde_json::from_str::<serde_json::Value>(detail)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| detail.chars().take(200).collect());

    if hint.is_empty() {
        format!("the model returned {status}: {message}")
    } else {
        format!("{hint} ({status}: {message})")
    }
}

/// Static, and deliberately so: it is the cached prefix of every turn, and the
/// guardrails in it must never be removable by a latency or cost setting.
const SYSTEM: &str = "\
You are Ward, a companion for a Commander playing Elite Dangerous.

Never state a fact about the Commander's ship, location, holdings or the game \
world unless it was given to you in this conversation. If you were not told, \
say you do not know and say what would let you find out. A wrong jump range, \
a wrong distance or an invented station stock is worse than no answer.

Never say you have done something unless you actually did it.

Keep replies short enough to listen to. You will often be heard rather than \
read.";

#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    system: &'a str,
    output_config: OutputConfig<'a>,
    messages: Vec<WireMessage<'a>>,
}

#[derive(Serialize)]
struct OutputConfig<'a> {
    effort: &'a str,
}

#[derive(Serialize)]
struct WireMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(payload: &str) -> Option<Event> {
        handle(payload, &mut None)
    }

    #[test]
    fn a_text_delta_becomes_reply_text() {
        let got =
            ev(r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}"#);
        assert_eq!(got, Some(Event::Text("Hello".into())));
    }

    #[test]
    fn thinking_never_reaches_the_reply() {
        // This is the one that matters. Thinking runs by default, so if it
        // leaked through here it would be shown on screen and later spoken.
        //
        // The payload carries a `text` field deliberately, even though a real
        // thinking delta does not. Without it this test passes whether or not
        // the type check exists — the `text` lookup returns nothing either way
        // — which would make it a test that cannot fail. With it, deleting the
        // type check turns the model's reasoning into reply text and this goes
        // red, which is the whole point of writing it down.
        let got = ev(
            r#"{"type":"content_block_delta","delta":{"type":"thinking_delta","text":"reasoning"}}"#,
        );
        assert_eq!(got, None);
    }

    #[test]
    fn a_signature_delta_is_ignored() {
        let got = ev(
            r#"{"type":"content_block_delta","delta":{"type":"signature_delta","signature":"x"}}"#,
        );
        assert_eq!(got, None);
    }

    #[test]
    fn a_stop_reason_is_captured() {
        let mut reason = None;
        handle(
            r#"{"type":"message_delta","delta":{"stop_reason":"refusal"}}"#,
            &mut reason,
        );
        assert_eq!(reason.as_deref(), Some("refusal"));
    }

    #[test]
    fn an_error_event_is_surfaced() {
        let got =
            ev(r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#);
        assert_eq!(got, Some(Event::Failed("Overloaded".into())));
    }

    #[test]
    fn an_unknown_event_type_is_ignored_rather_than_fatal() {
        // The protocol is allowed to grow. Failing here would break Ward on
        // the day the server starts sending something new.
        assert_eq!(ev(r#"{"type":"something_new_in_2027","x":1}"#), None);
    }

    #[test]
    fn malformed_json_does_not_panic() {
        assert_eq!(ev("{not json"), None);
        assert_eq!(ev(""), None);
    }

    #[test]
    fn an_unauthorized_response_says_what_to_check() {
        let msg = explain(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"error":{"message":"invalid x-api-key"}}"#,
        );
        assert!(msg.contains("pasted whole"), "got: {msg}");
        assert!(msg.contains("invalid x-api-key"), "got: {msg}");
    }

    #[test]
    fn an_unparseable_error_body_still_produces_a_message() {
        let msg = explain(reqwest::StatusCode::BAD_GATEWAY, "<html>gateway</html>");
        assert!(msg.contains("not something you did"), "got: {msg}");
    }

    #[test]
    fn the_api_key_cannot_be_printed() {
        // Deriving Debug on Client would put the key into the first error
        // message that formatted one, and from there into a log file. This
        // fails the moment somebody replaces the hand-written impl with a
        // derive.
        let client = Client::new(
            "sk-ant-secret-value-do-not-log".to_string(),
            DEFAULT_MODEL.to_string(),
        );
        let printed = format!("{client:?}");

        assert!(
            !printed.contains("sk-ant-secret-value"),
            "leaked: {printed}"
        );
        assert!(printed.contains("redacted"), "got: {printed}");
    }

    #[test]
    fn the_guardrails_are_in_the_system_prompt() {
        // These are static, cached prompt material. If a later change moves
        // them somewhere a budget or effort setting can strip, this fails.
        assert!(SYSTEM.contains("unless it was given to you"));
        assert!(SYSTEM.contains("Never say you have done something"));
    }
}
