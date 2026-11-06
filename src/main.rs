mod message;
mod ollama;
mod tools;

use clap::Parser;

/// capy: a small coding agent that drives a local model
#[derive(Parser, Debug)]
#[command(author, version)]
struct Args {
    #[arg(short, long, default_value = "http://localhost:11434/v1")]
    endpoint: String,

    #[arg(short, long, default_value = "qwen3.8-27b")]
    model: String,

    #[arg(long)]
    dry_run: bool,

    /// The task for capy, e.g. "what does this repo do?"
    task: String,
}

// ANSI escape codes: dim text, then reset. No crate needed for two styles.
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

// Loaded at compile time; edit prompts/system.md and rebuild.
const SYSTEM_PROMPT: &str = include_str!("../prompts/system.md");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let url = format!("{}/chat/completions", args.endpoint.trim_end_matches('/'));

    let model = &args.model;

    // Diagnostics go to stderr, so stdout holds only the answer.
    eprintln!("{DIM}→ {} @ {url}{RESET}", model);
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".into());
    let messages = vec![
        message::Message::System(SYSTEM_PROMPT.replace("{cwd}", &cwd)),
        message::Message::User(args.task),
    ];
    let tools = tools::specs();
    if args.dry_run {
        let payload = ollama::build_request(model, &messages, &tools);
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let client = reqwest::Client::new();

    let reply = ollama::chat(&client, &url, model, &messages, &tools).await?;

    if let Some(reasoning) = reply.reasoning {
        eprintln!("{DIM}{}{RESET}\n", reasoning.trim());
    }

    if let message::Message::Assistant { text, tool_calls } = &reply.message {
        // text: print it if Some
        if let Some(message) = text {
            println!("{}", message);
        }
        // tool_calls: print name and arguments for each
        for tool in tool_calls {
            println!("→ {} {} {}", tool.id, tool.name, tool.arguments);
        }
    }

    if let Some((prompt_tokens, completion_tokens)) = &reply.usage {
        eprintln!(
            "{DIM}tokens: {} in, {} out{RESET}",
            prompt_tokens, completion_tokens
        );
    }

    Ok(())
}
