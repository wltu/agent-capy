use crate::message::{Message, ToolCall};
use crate::tools::ToolSpec;
use anyhow::Context;
use serde::{Deserialize, Serialize};

pub struct Reply {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub reasoning: Option<String>, // display only, never sent back
    pub usage: Option<(u32, u32)>, // (prompt, completion); a small struct is nicer later
}

pub fn build_request(model: &str, messages: &[Message], tools: &[ToolSpec]) -> PostData {
    PostData {
        model: model.into(),
        messages: messages.iter().map(to_wire).collect(),
        tools: tools.iter().map(WireTool::from).collect(),
    }
}

pub async fn chat(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    messages: &[Message],
    tools: &[ToolSpec],
) -> anyhow::Result<Reply> {
    let payload = build_request(model, messages, tools);

    let response = client
        .post(url)
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("couldn't reach Ollama at {url}. Is `ollama serve` running?"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "model call failed ({status}): {}{}",
            snippet(&body),
            status_hint(status)
        );
    }

    // text first, then parse: `.json()` would consume the body, leaving
    // nothing to show when parsing fails
    let body = response
        .text()
        .await
        .context("couldn't read Ollama's response")?;
    let data: ChatResponse = serde_json::from_str(&body).with_context(|| {
        format!(
            "Ollama sent a response capy can't parse ({} bytes): {}",
            body.len(),
            snippet(&body)
        )
    })?;

    let ChatResponse { choices, usage } = data; // split the struct
    let mut wire = choices
        .into_iter()
        .next()
        .context("Ollama returned no choices")?
        .message;
    let reasoning = wire.reasoning.take(); // move it out before `.into()` consumes `wire`

    Ok(Reply {
        text: wire.content.filter(|c| !c.is_empty()),
        tool_calls: wire
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tool_call| ToolCall {
                id: tool_call.id,
                name: tool_call.function.name,
                arguments: tool_call.function.arguments,
            })
            .collect(),
        reasoning,
        usage: usage.map(|u| (u.prompt_tokens, u.completion_tokens)),
    })
}

const SNIPPET_LIMIT: usize = 200;

// Bodies can be huge (or sensitive), so errors show only the start.
fn snippet(body: &str) -> String {
    let body = body.trim();
    if body.len() <= SNIPPET_LIMIT {
        return body.to_string();
    }
    let cut = body.floor_char_boundary(SNIPPET_LIMIT);
    format!("{}… [first {cut} of {} bytes]", &body[..cut], body.len())
}

// What to try next, by status. Specific codes come before the range guards.
fn status_hint(status: reqwest::StatusCode) -> &'static str {
    use reqwest::StatusCode;
    match status {
        StatusCode::NOT_FOUND => {
            "\nhint: `ollama list` shows installed models; check --model and --endpoint"
        }
        StatusCode::BAD_REQUEST => {
            "\nhint: the message above says why. If it's about tools, pick a model that \
             supports them; otherwise run with --dry-run to see the request"
        }
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE => {
            "\nhint: Ollama is busy; try again shortly"
        }
        s if s.is_server_error() => {
            "\nhint: check the Ollama server log; the model may not fit in memory, \
             so a smaller one may work"
        }
        _ => "",
    }
}

#[derive(Debug, Serialize)]
pub struct WireRequestMessage {
    pub role: &'static str, // "system" | "user" | "assistant" | "tool"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<WireToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>, // only for role "tool"
}

#[derive(Debug, Serialize)]
pub struct PostData {
    pub model: String,
    pub messages: Vec<WireRequestMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<WireTool>,
}

#[derive(Debug, Deserialize)]
pub struct ChatResponse {
    pub choices: Vec<Choice>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub message: WireMessage,
}

#[derive(Debug, Deserialize)]
pub struct WireMessage {
    pub content: Option<String>, // null when the model only makes tool calls
    pub reasoning: Option<String>, // Ollama extra; thinking models fill it
    pub tool_calls: Option<Vec<WireToolCall>>,
}

#[derive(Debug, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WireToolCall {
    #[serde(default)] // some backends omit it
    pub id: String,
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: WireFunction,
}

fn function_kind() -> String {
    "function".into()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WireFunction {
    pub name: String,
    pub arguments: String,
}

impl From<WireMessage> for Message {
    fn from(w: WireMessage) -> Self {
        Message::Assistant {
            text: w.content.filter(|c| !c.is_empty()),
            tool_calls: w
                .tool_calls
                .unwrap_or_default()
                .into_iter()
                .map(|tool_call| ToolCall {
                    id: tool_call.id,
                    name: tool_call.function.name,
                    arguments: tool_call.function.arguments,
                })
                .collect(),
        }
    }
}

// Owned wire structs copy every message on each request. A borrowed
// WireRequestMessage<'a> would remove the clones; worth it once histories get long (Phase 4).
pub fn to_wire(m: &Message) -> WireRequestMessage {
    match m {
        Message::System(text) => WireRequestMessage {
            role: "system",
            content: Some(text.clone()),
            tool_calls: None,
            tool_call_id: None,
        },
        Message::User(text) => WireRequestMessage {
            role: "user",
            content: Some(text.clone()),
            tool_calls: None,
            tool_call_id: None,
        },
        Message::Assistant { text, tool_calls } => WireRequestMessage {
            role: "assistant",
            // strict servers reject an assistant message with neither content nor tool_calls
            content: text
                .clone()
                .or_else(|| tool_calls.is_empty().then(String::new)),
            tool_calls: (!tool_calls.is_empty()).then(|| {
                tool_calls
                    .iter()
                    .map(|c| WireToolCall {
                        id: c.id.clone(),
                        kind: function_kind(),
                        function: WireFunction {
                            name: c.name.clone(),
                            arguments: c.arguments.clone(),
                        },
                    })
                    .collect()
            }),
            tool_call_id: None,
        },
        Message::ToolResult { call_id, content } => WireRequestMessage {
            role: "tool",
            content: Some(content.clone()),
            tool_calls: None,
            tool_call_id: Some(call_id.clone()),
        },
    }
}

#[derive(Debug, Serialize)]
pub struct WireTool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolSpec,
}

impl From<&ToolSpec> for WireTool {
    fn from(t: &ToolSpec) -> Self {
        WireTool {
            kind: "function".into(),
            function: t.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_tool_call_reply() {
        let raw = include_str!("../fixtures/chat_tool_call.json");
        let resp: ChatResponse = serde_json::from_str(raw).unwrap();
        let msg: Message = resp.choices.into_iter().next().unwrap().message.into();
        assert_eq!(
            msg,
            Message::Assistant {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "kCM5JwEAHh9LUBJntaVdoTvShk1Fnf0O".into(),
                    name: "read_file".into(),
                    arguments: r#"{"path":"Cargo.toml"}"#.into()
                }]
            }
        )
    }

    #[test]
    fn converts_plain_reply() {
        let raw = include_str!("../fixtures/chat_plain.json");
        let resp: ChatResponse = serde_json::from_str(raw).unwrap();
        let msg: Message = resp.choices.into_iter().next().unwrap().message.into();
        assert_eq!(
            msg,
            Message::Assistant {
                text: Some("Hello! How can I help you today?".into()),
                tool_calls: vec![]
            }
        )
    }

    #[test]
    fn tool_result_serializes_with_call_id() {
        let m = Message::ToolResult {
            call_id: "abc".into(),
            content: "ok".into(),
        };
        assert_eq!(
            serde_json::to_value(to_wire(&m)).unwrap(),
            serde_json::json!({"role": "tool", "content": "ok", "tool_call_id": "abc"})
        );
    }

    #[test]
    fn system_message_serializes() {
        let m = Message::System("hello world".into());
        assert_eq!(
            serde_json::to_value(to_wire(&m)).unwrap(),
            serde_json::json!({"role": "system", "content": "hello world"})
        );
    }

    #[test]
    fn assistant_tool_call_serializes_nested() {
        let m = Message::Assistant {
            text: None,
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"Cargo.toml"}"#.into(),
            }],
        };
        let v = serde_json::to_value(to_wire(&m)).unwrap();
        let call = &v["tool_calls"][0];
        assert_eq!(v["role"], "assistant");
        assert_eq!(call["id"], "call_1");
        assert_eq!(call["type"], "function");
        assert_eq!(call["function"]["name"], "read_file");
        // arguments must stay a JSON *string*, not become an object
        assert_eq!(call["function"]["arguments"], r#"{"path":"Cargo.toml"}"#);
        assert!(v.get("tool_call_id").is_none());
    }

    #[test]
    fn assistant_without_tool_calls_omits_the_key() {
        let m = Message::Assistant {
            text: Some("hi".into()),
            tool_calls: vec![],
        };
        let v = serde_json::to_value(to_wire(&m)).unwrap();
        assert!(v.get("tool_calls").is_none());
        assert_eq!(v["content"], "hi");
    }

    #[test]
    fn empty_assistant_still_sends_content() {
        let m = Message::Assistant {
            text: None,
            tool_calls: vec![],
        };
        assert_eq!(
            serde_json::to_value(to_wire(&m)).unwrap(),
            serde_json::json!({"role": "assistant", "content": ""})
        );
    }

    #[test]
    fn tool_call_reply_survives_round_trip() {
        let raw = include_str!("../fixtures/chat_tool_call.json");
        let fixture: serde_json::Value = serde_json::from_str(raw).unwrap();
        let sent = &fixture["choices"][0]["message"]["tool_calls"][0];

        let resp: ChatResponse = serde_json::from_str(raw).unwrap();
        let msg: Message = resp.choices.into_iter().next().unwrap().message.into();
        let back = serde_json::to_value(to_wire(&msg)).unwrap();
        let got = &back["tool_calls"][0];

        assert_eq!(got["id"], sent["id"]);
        assert_eq!(got["type"], sent["type"]);
        assert_eq!(got["function"], sent["function"]);
    }

    #[test]
    fn request_includes_tools_in_ollama_shape() {
        let raw = include_str!("../fixtures/request_tools.json");
        let fixture: serde_json::Value = serde_json::from_str(raw).unwrap();
        let sent = &fixture["tools"];

        let tool = vec![ToolSpec {
            name: "read_file".into(),
            description: "Read a UTF-8 text file from the project and return its contents.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path relative to the project root" }
                },
                "required": ["path"]
            }),
        }];
        let req = build_request("test", &[], &tool);
        assert_eq!(serde_json::to_value(&req.tools).unwrap(), *sent);
    }

    #[test]
    fn empty_tools_are_omitted() {
        let v = serde_json::to_value(build_request("test", &[], &[])).unwrap();
        assert!(v.get("tools").is_none());
    }
}
