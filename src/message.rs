#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // ToolResult is built by the agent loop (step 4)
pub enum Message {
    System(String),
    User(String),
    Assistant {
        text: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    ToolResult {
        call_id: String,
        content: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String, // still JSON text: parsed when the tool runs
}
