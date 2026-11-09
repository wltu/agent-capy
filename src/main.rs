mod agent;
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

    /// Model calls before capy gives up on the task
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..))]
    max_turns: u32,

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

// Default deny: only y/yes approves; Enter, EOF and read errors all decline.
fn prompt_approval(cmd: &str) -> bool {
    use std::io::IsTerminal;

    eprint!("{}\nRun? [y/N] ", shown(cmd));
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_line(&mut input) {
        eprintln!("\n{DIM}Failed to get approval input: {e:#}{RESET}");
        return false;
    }
    let answer = input.trim();
    let yes = answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes");

    // A typed answer echoes itself and its newline; piped input and EOF don't,
    // so say what we decided instead of leaving the cursor after the prompt.
    if !std::io::stdin().is_terminal() {
        let decision = if yes { "yes" } else { "no" };
        eprintln!("{decision} {DIM}(stdin is not a terminal){RESET}");
    }
    yes
}

// The user approves what the terminal shows, so show the real bytes: escape
// control chars (\r, ANSI codes) and invisible or bidi chars that could
// make `rm -rf ~` render as `ls`.
fn shown(cmd: &str) -> String {
    cmd.chars()
        .map(|c| {
            let hidden = (c.is_control() && c != '\n')
                || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}'
                    | '\u{2060}'..='\u{2069}' | '\u{feff}');
            if hidden {
                c.escape_unicode().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

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
    if args.dry_run {
        let payload = ollama::build_request(model, &messages, &tools::specs());
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let client = reqwest::Client::new();
    println!(
        "{}",
        agent::run(
            &client,
            &url,
            model,
            args.max_turns,
            messages,
            prompt_approval
        )
        .await?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::shown;

    #[test]
    fn approval_prompt_cannot_be_disguised() {
        let sneaky = "rm -rf ~ #\r\x1b[2Kls\u{202e}txt";
        let out = shown(sneaky);
        assert!(
            !out.chars().any(|c| c.is_control() || c == '\u{202e}'),
            "{out:?}"
        );
        assert!(out.starts_with("rm -rf ~ #"), "{out:?}");
        assert_eq!(shown("ls\ncat x"), "ls\ncat x", "newlines stay readable");
    }
}
