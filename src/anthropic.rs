//! The model client.
//!
//! Hand-rolled, because there is no official Rust SDK. The surface kept here is
//! deliberately small — send a conversation, receive a stream of events — so
//! that the loop above it never learns anything provider-shaped. The moment an
//! interface like this grows provider-specific parameters, the abstraction
//! stops paying for itself.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::capability::Registry;
use crate::sse;

const API: &str = "https://api.anthropic.com/v1/messages";
const CATALOG: &str = "https://api.anthropic.com/v1/models";
const VERSION: &str = "2023-06-01";

pub const DEFAULT_MODEL: &str = "claude-opus-5";

/// Caps thinking and reply text **together**, so this cannot be tuned down for
/// latency without risking a truncated answer mid-sentence.
const MAX_TOKENS: u32 = 4096;

/// How many times one turn may call tools before Ward stops it.
///
/// A model that keeps asking for tools spends money on every round while the
/// Commander waits and nothing improves. The cap turns an unbounded cost into a
/// bounded one that has something to say for itself.
const MAX_TOOL_ROUNDS: usize = 5;

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
    /// A capability is being used. Worth surfacing: the Commander is waiting,
    /// and knowing Ward is checking something beats an unexplained pause.
    Using(String),
    /// The turn ended.
    Done { stop_reason: Option<String> },
    /// The turn failed. Carries something a person can read.
    Failed(String),
}

/// Which models this key can actually use.
///
/// Asked rather than hardcoded, because a list compiled in goes stale the week
/// a model ships and there is no way for a Commander to notice. It costs
/// nothing in tokens, which is also what makes it the honest way to find out
/// whether a key works at all.
pub async fn models(api_key: &str) -> Result<Vec<String>, String> {
    let response = reqwest::Client::new()
        .get(format!("{CATALOG}?limit=100"))
        .header("x-api-key", api_key)
        .header("anthropic-version", VERSION)
        .send()
        .await
        .map_err(|e| format!("could not reach the model service: {e}"))?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    if !status.is_success() {
        // The service's own words. A guess at what a rejection meant is how a
        // Commander spends an evening on the wrong problem.
        let said: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        let reason = said["error"]["message"]
            .as_str()
            .unwrap_or("no reason given")
            .to_string();

        return Err(match status.as_u16() {
            401 => format!("that key was refused: {reason}"),
            _ => format!("the model service said {status}: {reason}"),
        });
    }

    let listed: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("could not read the model list: {e}"))?;

    let models: Vec<String> = listed["data"]
        .as_array()
        .map(|models| {
            models
                .iter()
                .filter_map(|m| m["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    match models.is_empty() {
        true => Err("the model service returned no models".to_string()),
        false => Ok(models),
    }
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    api_key: String,
    model: String,
    registry: Arc<Registry>,
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
    /// The same key against a different model and a rebuilt registry.
    ///
    /// The key never travels back out through a settings page to get here,
    /// which is the point: rotating a model is not an occasion to handle a
    /// secret.
    pub fn with(&self, model: String, registry: Arc<Registry>) -> Self {
        Self {
            http: self.http.clone(),
            api_key: self.api_key.clone(),
            model,
            registry,
        }
    }

    pub fn new(api_key: String, model: String, registry: Arc<Registry>) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key,
            model,
            registry,
        }
    }

    /// Runs one turn to completion, sending events as they arrive.
    ///
    /// Never returns an error to the caller: a failure is delivered as
    /// `Event::Failed` on the same channel as everything else, so a caller has
    /// exactly one place to watch and cannot leave the Commander staring at a
    /// half-finished reply with no explanation.
    pub async fn stream(
        &self,
        history: &[Message],
        situation: String,
        out: UnboundedSender<Event>,
    ) {
        if let Err(e) = self.run_turn(history, &situation, &out).await {
            let _ = out.send(Event::Failed(e.to_string()));
        }
    }

    /// One turn, which may take several requests.
    ///
    /// A turn is not a request. If the model asks for a tool, Ward runs it,
    /// hands back the result and asks again — and all of that is still the one
    /// question the Commander asked. Only the final reply is theirs to read.
    async fn run_turn(
        &self,
        history: &[Message],
        situation: &str,
        out: &UnboundedSender<Event>,
    ) -> Result<()> {
        let started = std::time::Instant::now();

        let mut wire: Vec<WireMessage> = history
            .iter()
            .map(|m| WireMessage {
                role: m.role.wire(),
                content: vec![Block::Text {
                    text: m.text.clone(),
                }],
            })
            .collect();

        // Live state rides a trailing system message rather than being pasted
        // onto the Commander's question. Two reasons, and both matter.
        //
        // It leaves the cached prefix untouched: editing the system prompt
        // invalidates the whole conversation, while appending here does not.
        //
        // And it keeps the operator channel separate from the person's. Text
        // arriving in a user turn is text the model may reasonably treat as
        // something the Commander said; this is Ward telling the model what is
        // true, which is a different kind of statement.
        wire.push(WireMessage {
            role: "system",
            content: vec![Block::Text {
                text: format!("Current situation, from the game's own journal. {situation}"),
            }],
        });

        for round in 0..=MAX_TOOL_ROUNDS {
            let collected = self.request(&wire, out).await?;
            let calls = collected.tool_calls();

            if calls.is_empty() {
                tracing::info!(
                    target: "ward::model",
                    ms = started.elapsed().as_millis() as u64,
                    rounds = round,
                    stop_reason = collected.stop_reason.as_deref().unwrap_or("none"),
                    "reply complete"
                );
                let _ = out.send(Event::Done {
                    stop_reason: collected.stop_reason,
                });
                return Ok(());
            }

            if round == MAX_TOOL_ROUNDS {
                return Err(anyhow!(
                    "Ward kept needing tools and stopped after {MAX_TOOL_ROUNDS} rounds \
                     rather than carrying on indefinitely."
                ));
            }

            // The assistant's own blocks go back exactly as they arrived, tool
            // requests included. Sending only the text would ask the model to
            // interpret results for calls it has no record of making.
            wire.push(WireMessage {
                role: "assistant",
                content: collected.assistant_blocks(),
            });

            let mut results = Vec::new();

            for call in calls {
                tracing::info!(target: "ward::capability", tool = %call.name, "running");
                let _ = out.send(Event::Using(call.name.clone()));

                let output = self.registry.run(&call.name, &call.input);

                tracing::debug!(
                    target: "ward::capability",
                    tool = %call.name,
                    chars = output.len(),
                    "returned"
                );

                results.push(Block::ToolResult {
                    tool_use_id: call.id,
                    content: output,
                });
            }

            wire.push(WireMessage {
                role: "user",
                content: results,
            });
        }

        unreachable!("the loop returns or errors on the final round")
    }

    /// One request and the stream it produces.
    async fn request(
        &self,
        wire: &[WireMessage],
        out: &UnboundedSender<Event>,
    ) -> Result<Collected> {
        let started = std::time::Instant::now();
        let tools = self.registry.tools();

        tracing::debug!(
            target: "ward::model",
            model = %self.model,
            messages = wire.len(),
            tools = tools.len(),
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
            tools,
            messages: wire,
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
        // feels like conversation, so it is measured from the first turn rather
        // than added once it becomes a problem.
        tracing::debug!(
            target: "ward::model",
            ms = started.elapsed().as_millis() as u64,
            "response opened"
        );

        let mut parser = sse::Parser::new();
        let mut collected = Collected::default();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow!("the connection dropped mid-reply: {e}"))?;
            let text = String::from_utf8_lossy(&chunk).into_owned();

            for payload in parser.push(&text) {
                match collected.absorb(&payload) {
                    Some(Event::Failed(m)) => return Err(anyhow!("{m}")),
                    Some(event) => {
                        // A closed receiver means the window went away. Stop
                        // rather than carrying on producing tokens nobody will
                        // ever read.
                        if out.send(event).is_err() {
                            return Ok(collected);
                        }
                    }
                    None => {}
                }
            }
        }

        Ok(collected)
    }
}

/// A tool the model asked for.
#[derive(Debug, PartialEq)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// What one request produced.
///
/// Kept apart from the network so all of it — which blocks arrived, what the
/// model asked for, what goes back — is testable against recorded payloads,
/// with no key, no bill and no waiting.
#[derive(Default)]
pub struct Collected {
    blocks: BTreeMap<usize, Partial>,
    pub stop_reason: Option<String>,
}

enum Partial {
    Text(String),
    /// Tool arguments arrive as JSON in fragments, so nothing can be parsed
    /// until the block closes.
    Tool {
        id: String,
        name: String,
        json: String,
    },
    /// Reasoning. Tracked so its deltas do not land in a text block, and
    /// otherwise discarded.
    Thinking,
}

impl Collected {
    /// Takes one event payload, returning anything the Commander should see.
    ///
    /// An event type this does not know about returns nothing rather than
    /// failing. The protocol is allowed to grow, and a client that falls over
    /// on an unfamiliar event breaks the day the server adds one.
    pub fn absorb(&mut self, payload: &str) -> Option<Event> {
        let v: Value = serde_json::from_str(payload).ok()?;

        match v.get("type")?.as_str()? {
            "content_block_start" => {
                let index = v.get("index")?.as_u64()? as usize;
                let block = v.get("content_block")?;

                let partial = match block.get("type")?.as_str()? {
                    "text" => Partial::Text(
                        block
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    "tool_use" => Partial::Tool {
                        id: block.get("id")?.as_str()?.to_string(),
                        name: block.get("name")?.as_str()?.to_string(),
                        json: String::new(),
                    },
                    _ => Partial::Thinking,
                };

                self.blocks.insert(index, partial);
                None
            }

            "content_block_delta" => {
                let index = v.get("index")?.as_u64()? as usize;
                let delta = v.get("delta")?;

                match delta.get("type")?.as_str()? {
                    "text_delta" => {
                        let text = delta.get("text")?.as_str()?;
                        if let Some(Partial::Text(existing)) = self.blocks.get_mut(&index) {
                            existing.push_str(text);
                        }
                        Some(Event::Text(text.to_string()))
                    }
                    "input_json_delta" => {
                        let fragment = delta.get("partial_json")?.as_str()?;
                        if let Some(Partial::Tool { json, .. }) = self.blocks.get_mut(&index) {
                            json.push_str(fragment);
                        }
                        None
                    }
                    // thinking_delta and signature_delta land here and are
                    // deliberately dropped: reasoning is not the reply.
                    _ => None,
                }
            }

            "message_delta" => {
                if let Some(reason) = v
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(|r| r.as_str())
                {
                    self.stop_reason = Some(reason.to_string());
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

    /// The tools the model asked for, in the order it asked.
    ///
    /// A tool whose arguments did not parse is dropped rather than guessed at.
    /// Inventing an argument would call something with input the model never
    /// chose, which is worse than not calling it at all.
    pub fn tool_calls(&self) -> Vec<Call> {
        self.blocks
            .values()
            .filter_map(|b| match b {
                Partial::Tool { id, name, json } => {
                    let input = if json.trim().is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str(json).ok()?
                    };
                    Some(Call {
                        id: id.clone(),
                        name: name.clone(),
                        input,
                    })
                }
                _ => None,
            })
            .collect()
    }

    /// The assistant's message, to be handed back with the tool results.
    fn assistant_blocks(&self) -> Vec<Block> {
        self.blocks
            .values()
            .filter_map(|b| match b {
                Partial::Text(text) if !text.is_empty() => Some(Block::Text { text: text.clone() }),
                Partial::Tool { id, name, json } => Some(Block::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: serde_json::from_str(json).unwrap_or_else(|_| json!({})),
                }),
                _ => None,
            })
            .collect()
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

    let message = serde_json::from_str::<Value>(detail)
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
world unless it was given to you in this conversation or returned by a tool. \
If you were not told, say you do not know and say what would let you find out. \
A wrong jump range, a wrong distance or an invented station stock is worse \
than no answer.

Never say you have done something unless you actually called the tool that \
does it.

Keep replies short enough to listen to. You will often be heard rather than \
read.";

#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    system: &'a str,
    output_config: OutputConfig<'a>,
    tools: Vec<Value>,
    messages: &'a [WireMessage],
}

#[derive(Serialize)]
struct OutputConfig<'a> {
    effort: &'a str,
}

#[derive(Serialize)]
struct WireMessage {
    role: &'static str,
    content: Vec<Block>,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type")]
enum Block {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(payloads: &[&str]) -> (Collected, Vec<Event>) {
        let mut c = Collected::default();
        let events = payloads.iter().filter_map(|p| c.absorb(p)).collect();
        (c, events)
    }

    #[test]
    fn a_text_delta_becomes_reply_text() {
        let (_, events) = collect(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        ]);
        assert_eq!(events, vec![Event::Text("Hello".into())]);
    }

    #[test]
    fn thinking_never_reaches_the_reply() {
        // Thinking runs by default, so a leak here would be shown on screen and
        // later spoken. The payload carries a `text` field a real thinking
        // delta would not, because without one this passes whether or not the
        // guard exists — which would make it a test that cannot fail.
        let (_, events) = collect(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","text":"reasoning"}}"#,
        ]);
        assert!(events.is_empty(), "reasoning leaked: {events:?}");
    }

    #[test]
    fn a_tool_call_is_assembled_from_fragments() {
        // Arguments arrive as JSON split across deltas, so nothing can be
        // parsed until the block is complete.
        let (c, events) = collect(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tu_1","name":"ward_version"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"a\""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":":1}"}}"#,
        ]);

        assert!(events.is_empty(), "a tool call is not reply text");

        let calls = c.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "tu_1");
        assert_eq!(calls[0].name, "ward_version");
        assert_eq!(calls[0].input, json!({"a": 1}));
    }

    #[test]
    fn a_tool_call_with_no_arguments_still_runs() {
        let (c, _) = collect(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tu_2","name":"ward_version"}}"#,
        ]);

        let calls = c.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].input, json!({}));
    }

    #[test]
    fn a_tool_call_whose_arguments_are_broken_is_dropped() {
        // Guessing at the arguments would call something with input the model
        // never chose, which is worse than not calling it.
        let (c, _) = collect(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tu_3","name":"ward_version"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{not json"}}"#,
        ]);

        assert!(c.tool_calls().is_empty());
    }

    #[test]
    fn text_and_a_tool_call_keep_their_order() {
        let (c, _) = collect(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Let me check."}}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tu_4","name":"ward_version"}}"#,
        ]);

        let blocks = c.assistant_blocks();
        assert_eq!(blocks.len(), 2);

        // Handed back exactly as they arrived: sending only the text would ask
        // the model to interpret a result for a call it has no record of.
        let json = serde_json::to_value(&blocks).unwrap();
        assert_eq!(json[0]["type"], "text");
        assert_eq!(json[1]["type"], "tool_use");
        assert_eq!(json[1]["id"], "tu_4");
    }

    #[test]
    fn a_tool_result_serializes_the_way_the_api_expects() {
        let block = Block::ToolResult {
            tool_use_id: "tu_5".into(),
            content: "Ward 0.1.0".into(),
        };
        let json = serde_json::to_value(&block).unwrap();

        assert_eq!(json["type"], "tool_result");
        assert_eq!(json["tool_use_id"], "tu_5");
        assert_eq!(json["content"], "Ward 0.1.0");
    }

    #[test]
    fn a_stop_reason_is_captured() {
        let (c, _) = collect(&[r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#]);
        assert_eq!(c.stop_reason.as_deref(), Some("tool_use"));
    }

    #[test]
    fn an_error_event_is_surfaced() {
        let (_, events) = collect(&[
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        ]);
        assert_eq!(events, vec![Event::Failed("Overloaded".into())]);
    }

    #[test]
    fn an_unknown_event_type_is_ignored_rather_than_fatal() {
        let (_, events) = collect(&[r#"{"type":"something_new_in_2027","x":1}"#]);
        assert!(events.is_empty());
    }

    #[test]
    fn malformed_json_does_not_panic() {
        let (_, events) = collect(&["{not json", ""]);
        assert!(events.is_empty());
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
            Arc::new(crate::capabilities::registry(
                &crate::config::Settings::default(),
                &std::env::temp_dir().join("ward-anthropic-test"),
            )),
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
        // Static, cached prompt material. If a later change moves them
        // somewhere a budget or effort setting can strip, this fails.
        assert!(SYSTEM.contains("unless it was given to you"));
        assert!(SYSTEM.contains("Never say you have done something"));
    }

    #[test]
    fn the_tools_go_out_with_the_request() {
        let body = serde_json::to_value(Request {
            model: "m",
            max_tokens: 1,
            stream: true,
            system: "s",
            output_config: OutputConfig { effort: "low" },
            tools: crate::capabilities::registry(
                &crate::config::Settings::default(),
                &std::env::temp_dir().join("ward-anthropic-test"),
            )
            .tools(),
            messages: &[],
        })
        .unwrap();

        let tools = body["tools"].as_array().unwrap();
        assert!(!tools.is_empty(), "the model was offered no tools");
        assert!(tools.iter().any(|t| t["name"] == "ward_version"));
    }
}
