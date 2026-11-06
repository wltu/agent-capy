use crate::message::{Message, ToolCall};
use anyhow::Context;
use serde::{Deserialize, Serialize};

pub struct Reply {
    pub message: Message,          // Assistant { text, tool_calls }
    pub reasoning: Option<String>, // display only, never sent back
    pub usage: Option<(u32, u32)>, // (prompt, completion); a small struct is nicer later
}

pub fn build_request(model: &str, messages: &[Message]) -> PostData {
    PostData {
        model: model.into(),
        messages: messages.iter().map(to_wire).collect(),
    }
}

pub async fn chat(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    messages: &[Message],
) -> anyhow::Result<Reply> {
    let payload = build_request(model, messages);

    let response = client
        .post(url)
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("couldn't reach Ollama at {url}. Is `ollama serve` running?"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let body = body.trim();
        anyhow::bail!(
            "model call failed ({status}): {body}\n\
             hint: `ollama list` shows installed models; pass one with --model"
        );
    }

    let data: ChatResponse = response
        .json()
        .await
        .context("Ollama sent a response capy can't parse")?;

    let ChatResponse { choices, usage } = data; // split the struct
    let mut wire = choices
        .into_iter()
        .next()
        .context("Ollama returned no choices")?
        .message;
    let reasoning = wire.reasoning.take(); // move it out before `.into()` consumes `wire`
    let message: Message = wire.into();

    Ok(Reply {
        message,
        reasoning,
        usage: usage.map(|u| (u.prompt_tokens, u.completion_tokens)),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
