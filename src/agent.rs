use crate::message::Message;
use crate::ollama;
use crate::tools;
use crate::{DIM, RESET};

pub async fn run(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    max_turns: u32,
    mut history: Vec<Message>,
    approve: impl Fn(&str) -> bool,
) -> anyhow::Result<String> {
    let specs = tools::specs();
    for _ in 0..max_turns {
        let reply = ollama::chat(client, url, model, &history, &specs).await?;
        if let Some(reasoning) = reply.reasoning {
            eprintln!("{DIM}{}{RESET}\n", reasoning.trim());
        }

        if let Some((prompt_tokens, completion_tokens)) = &reply.usage {
            eprintln!(
                "{DIM}tokens: {} in, {} out{RESET}",
                prompt_tokens, completion_tokens
            );
        }

        let Message::Assistant { text, tool_calls } = &reply.message else {
            unreachable!("ollama::chat always returns Message::Assistant")
        };
        history.push(reply.message.clone());

        if tool_calls.is_empty() {
            return Ok(text.clone().unwrap_or_else(|| "(no answer)".into()));
        }

        for tool in tool_calls {
            eprintln!("{DIM}→ {} {}{RESET}", tool.name, tool.arguments);
            let tool_result = Message::ToolResult {
                call_id: tool.id.clone(),
                content: tools::run(tool, &approve)
                    .await
                    .unwrap_or_else(|e| format!("error: {e:#}")),
            };
            history.push(tool_result);
        }
    }
    anyhow::bail!(
        "stopped after {max_turns} turns without a final answer. \
         Rerun with a higher --max-turns, or give capy a smaller task."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    type Seen = Arc<Mutex<Vec<Value>>>;

    // A fake Ollama: answers each request with the next canned reply (the last
    // one repeats forever) and records every request body it received.
    async fn fake_ollama(replies: Vec<Value>) -> (String, Seen) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/chat/completions", listener.local_addr().unwrap());
        let seen: Seen = Arc::default();
        let log = seen.clone();
        tokio::spawn(async move {
            for i in 0..usize::MAX {
                let (mut sock, _) = listener.accept().await.unwrap();
                let request = read_json(&mut sock).await; // lock after the await, not across it
                log.lock().unwrap().push(request);
                let body = replies[i.min(replies.len() - 1)].to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        (url, seen)
    }

    // Reads one HTTP request and returns its JSON body.
    async fn read_json(sock: &mut TcpStream) -> Value {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = sock.read(&mut chunk).await.unwrap();
            assert!(n > 0, "client closed before sending a full request");
            buf.extend_from_slice(&chunk[..n]);
            let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                continue;
            };
            let head = String::from_utf8_lossy(&buf[..end]).to_lowercase();
            let len: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .map_or(0, |v| v.trim().parse().unwrap());
            let body = end + 4;
            if buf.len() >= body + len {
                return serde_json::from_slice(&buf[body..body + len]).unwrap();
            }
        }
    }

    fn text_reply(text: &str) -> Value {
        json!({ "choices": [{ "message": { "role": "assistant", "content": text } }] })
    }

    fn tool_reply(calls: &[(&str, &str, Value)]) -> Value {
        let calls: Vec<Value> = calls
            .iter()
            .map(|(id, name, args)| {
                json!({ "id": id, "type": "function",
                        "function": { "name": name, "arguments": args.to_string() } })
            })
            .collect();
        json!({ "choices": [{ "message": { "role": "assistant", "content": "", "tool_calls": calls } }] })
    }

    async fn run_with(
        replies: Vec<Value>,
        approve: impl Fn(&str) -> bool,
    ) -> (anyhow::Result<String>, Vec<Value>) {
        run_with_turns(replies, approve, 10).await
    }

    async fn run_with_turns(
        replies: Vec<Value>,
        approve: impl Fn(&str) -> bool,
        max_turns: u32,
    ) -> (anyhow::Result<String>, Vec<Value>) {
        let (url, seen) = fake_ollama(replies).await;
        let history = vec![Message::User("task".into())];
        let out = run(
            &reqwest::Client::new(),
            &url,
            "fake",
            max_turns,
            history,
            approve,
        )
        .await;
        let requests = seen.lock().unwrap().clone();
        (out, requests)
    }

    // The `messages` array of request `i`.
    fn messages(requests: &[Value], i: usize) -> &Vec<Value> {
        requests[i]["messages"].as_array().unwrap()
    }

    #[tokio::test]
    async fn plain_answer_takes_one_turn() {
        let (out, requests) = run_with(vec![text_reply("hi")], |_| true).await;
        assert_eq!(out.unwrap(), "hi");
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn empty_reply_returns_instead_of_looping() {
        let (out, requests) = run_with(vec![text_reply("")], |_| true).await;
        assert!(out.is_ok(), "{out:?}");
        assert_eq!(
            requests.len(),
            1,
            "called the model again after an empty answer"
        );
    }

    #[tokio::test]
    async fn tool_result_follows_its_call_then_answer() {
        let (out, requests) = run_with(
            vec![
                tool_reply(&[("c1", "read_file", json!({ "path": "Cargo.toml" }))]),
                text_reply("done"),
            ],
            |_| true,
        )
        .await;
        assert_eq!(out.unwrap(), "done");
        assert_eq!(requests.len(), 2);

        // second request: [user, assistant(call c1), tool(c1)]
        let m = messages(&requests, 1);
        assert_eq!(m.len(), 3, "{m:#?}");
        assert_eq!(m[1]["role"], "assistant");
        assert_eq!(m[1]["tool_calls"][0]["id"], "c1");
        assert_eq!(m[2]["role"], "tool");
        assert_eq!(m[2]["tool_call_id"], "c1");
        assert!(m[2]["content"].as_str().unwrap().contains("[package]"));
    }

    #[tokio::test]
    async fn every_call_gets_one_result_in_order() {
        let (out, requests) = run_with(
            vec![
                tool_reply(&[
                    ("c1", "read_file", json!({ "path": "Cargo.toml" })),
                    ("c2", "read_file", json!({ "path": "nope.txt" })),
                    ("c3", "bash", json!({ "command": "ls" })),
                ]),
                text_reply("done"),
            ],
            |_| false,
        )
        .await;
        assert_eq!(out.unwrap(), "done");

        let m = messages(&requests, 1);
        let results: Vec<&Value> = m.iter().filter(|x| x["role"] == "tool").collect();
        let ids: Vec<&str> = results
            .iter()
            .map(|r| r["tool_call_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["c1", "c2", "c3"]);

        let content: Vec<&str> = results
            .iter()
            .map(|r| r["content"].as_str().unwrap())
            .collect();
        // only a failed tool is marked; output and a decline are not
        assert!(!content[0].starts_with("error:"), "{}", content[0]);
        // failures reach the model as text instead of ending the run
        assert!(content[1].starts_with("error:"), "{}", content[1]);
        assert!(content[1].contains("nope.txt"), "{}", content[1]);
        assert!(!content[2].starts_with("error:"), "{}", content[2]);
        assert!(
            content[2].contains("The user declined to run this command"),
            "{}",
            content[2]
        );
    }

    #[tokio::test]
    async fn stops_after_max_turns() {
        // the model asks for the same file forever
        let (out, requests) = run_with_turns(
            vec![tool_reply(&[(
                "c1",
                "read_file",
                json!({ "path": "Cargo.toml" }),
            )])],
            |_| true,
            3,
        )
        .await;
        let msg = format!("{:#}", out.expect_err("expected an error after 3 turns"));
        assert!(msg.contains('3') && msg.contains("--max-turns"), "{msg}");
        assert_eq!(requests.len(), 3);
    }

    // Runs one `touch {path}` call through the agent with the given approver.
    // Returns the tool message the model saw and whether the file now exists.
    async fn touch_through_agent(name: &str, approve: impl Fn(&str) -> bool) -> (String, bool) {
        let path = format!("target/capy_{name}_{}", std::process::id());
        std::fs::remove_file(&path).ok(); // a leftover would fake the result
        let (out, requests) = run_with(
            vec![
                tool_reply(&[("c1", "bash", json!({ "command": format!("touch {path}") }))]),
                text_reply("ok"),
            ],
            approve,
        )
        .await;
        assert_eq!(out.unwrap(), "ok");
        let tool_msg = messages(&requests, 1)
            .iter()
            .find(|m| m["role"] == "tool")
            .expect("a tool result")["content"]
            .as_str()
            .unwrap()
            .to_string();
        let exists = std::path::Path::new(&path).exists();
        std::fs::remove_file(&path).ok();
        (tool_msg, exists)
    }

    #[tokio::test]
    async fn declined_command_does_not_run() {
        // Fn closures can't mutate captures directly; RefCell allows it at runtime
        let asked = std::cell::RefCell::new(Vec::new());
        let (tool_msg, exists) = touch_through_agent("declined", |cmd| {
            asked.borrow_mut().push(cmd.to_string());
            false
        })
        .await;
        assert!(!exists, "declined command ran anyway");
        assert!(
            tool_msg.contains("The user declined to run this command"),
            "{tool_msg}"
        );
        // the user must be shown the command itself, not the JSON arguments
        let asked = asked.into_inner();
        assert_eq!(asked.len(), 1, "asked {asked:?}");
        assert!(
            asked[0].starts_with("touch target/capy_declined_"),
            "{asked:?}"
        );
    }

    #[tokio::test]
    async fn approved_command_runs() {
        let (tool_msg, exists) = touch_through_agent("approved", |_| true).await;
        assert!(exists, "approved command didn't run");
        assert!(tool_msg.contains("exit status: 0"), "{tool_msg}");
    }
}
