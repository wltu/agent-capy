use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct PostData {
    model: String,
    messages: Vec<Message>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: WireMessage,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // wire type: `role` is read by step 2's conversion
struct WireMessage {
    role: String,
    content: Option<String>,   // null when the model only makes tool calls
    reasoning: Option<String>, // Ollama extra; thinking models fill it
}

#[derive(Debug, Deserialize)]
struct Usage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

// ANSI escape codes: dim text, then reset. No crate needed for two styles.
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let base_url =
        std::env::var("CAPY_BASE_URL").unwrap_or_else(|_| "http://localhost:11434/v1".to_string());
    let url = format!("{base_url}/chat/completions");

    let payload = PostData {
        model: String::from("qwen3.8-27b"),
        messages: vec![Message {
            role: String::from("user"),
            content: String::from("Say hi in five words."),
        }],
    };

    // Diagnostics go to stderr, so stdout holds only the answer.
    eprintln!("{DIM}→ {} @ {url}{RESET}", payload.model);

    let response = reqwest::Client::new()
        .post(&url)
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("couldn't reach Ollama at {url}. Is `ollama serve` running?"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await?;
        anyhow::bail!("Failed to call the model: {status}: {body}");
    }

    let data: ChatResponse = response
        .json()
        .await
        .context("Ollama sent a response capy can't parse")?;
    let message = &data
        .choices
        .first()
        .context("Ollama returned no choices")?
        .message;

    if let Some(reasoning) = &message.reasoning {
        eprintln!("{DIM}{}{RESET}\n", reasoning.trim());
    }
    println!("{}", message.content.as_deref().unwrap_or("").trim());

    if let Some(u) = &data.usage {
        eprintln!(
            "{DIM}tokens: {} in, {} out{RESET}",
            u.prompt_tokens, u.completion_tokens
        );
    }

    Ok(())
}
