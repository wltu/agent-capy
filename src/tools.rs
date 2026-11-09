use crate::message::ToolCall;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fmt, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt};

const READ_LIMIT: usize = 20_000;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

// Supported Tools for Capy
#[derive(Debug, Copy, Clone)]
pub enum Tool {
    ReadFile,
    Bash,
}

const ALL_TOOLS: [Tool; 2] = [Tool::ReadFile, Tool::Bash];

impl fmt::Display for Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Tool::ReadFile => "read_file",
            Tool::Bash => "bash",
        };
        write!(f, "{}", name)
    }
}

impl From<Tool> for ToolSpec {
    fn from(t: Tool) -> Self {
        match t {
            Tool::ReadFile => ToolSpec {
                name: t.to_string(),
                description: "Read a UTF-8 text file from the project and return its contents. \
                              Paths are relative to the project root; large files are truncated."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path relative to the project root" }
                    },
                    "required": ["path"]
                }),
            },
            Tool::Bash => ToolSpec {
                name: t.to_string(),
                description: format!(
                    "Run a command with `bash -c` in the project root. The user approves each \
                     command first; a declined command is a normal result. There is no terminal \
                     and no stdin, so interactive programs fail: use non-interactive flags such as \
                     `git commit -m`, `git --no-pager` and `-y`. Commands are killed after {} \
                     seconds. Returns the exit status, then stdout, then stderr; each stream is cut \
                     after its first {} KB.",
                    DEFAULT_TIMEOUT.as_secs(),
                    READ_LIMIT / 1000,
                ),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Command to run in the project root" },
                    },
                    "required": ["command"]
                }),
            },
        }
    }
}

pub fn specs() -> Vec<ToolSpec> {
    ALL_TOOLS.iter().copied().map(ToolSpec::from).collect()
}

#[derive(Deserialize)]
struct ReadFileArgs {
    path: String,
}

#[derive(Deserialize)]
struct BashArgs {
    command: String,
}

pub async fn run(
    call: &ToolCall,
    approve: impl Fn(&str) -> bool,
    timeout: Duration,
) -> anyhow::Result<String> {
    let name = call.name.as_str();
    match ALL_TOOLS.iter().find(|t| t.to_string() == name) {
        Some(Tool::ReadFile) => {
            let args: ReadFileArgs = serde_json::from_str(&call.arguments).with_context(|| {
                format!(
                    "{} expected {}: got {}",
                    call.name,
                    ToolSpec::from(Tool::ReadFile).parameters,
                    call.arguments
                )
            })?;
            read_file(&args.path).await
        }
        Some(Tool::Bash) => {
            let args: BashArgs = serde_json::from_str(&call.arguments).with_context(|| {
                format!(
                    "{} expected {}: got {}",
                    call.name,
                    ToolSpec::from(Tool::Bash).parameters,
                    call.arguments
                )
            })?;
            // never ask the user to approve a blank command
            if args.command.trim().is_empty() {
                anyhow::bail!("bash needs a non-empty command");
            }
            if !approve(&args.command) {
                return Ok(format!(
                    "The user declined to run this command: {}\n\
                     Do not run it or a variant of it. Continue without it, \
                     or ask the user what to do instead.",
                    args.command
                ));
            }
            run_command(&args.command, timeout).await
        }
        None => anyhow::bail!(
            "unknown tool {name}: available tools {}",
            ALL_TOOLS
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

async fn run_command(cmd: &str, timeout: Duration) -> anyhow::Result<String> {
    let mut child = tokio::process::Command::new("bash")
        .arg("-c")
        .arg(cmd)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to start bash")?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let run = async { tokio::try_join!(read_capped(stdout), read_capped(stderr), child.wait()) };
    // on timeout `child` is dropped at return, and kill_on_drop kills bash
    let ((out, out_total), (err, err_total), status) = tokio::time::timeout(timeout, run)
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "command timed out and was killed after {timeout:?}. It may have partly run \
                 (check for changes it made); its output is not shown. Try a faster command, \
                 limit its work (e.g. one test, `head`), or run it in the background."
            )
        })?
        .context("failed to read command output")?;

    // stdout first and each stream capped on its own, so a flood of
    // compiler warnings on stderr can't push the results out
    let mut text = format!("{status}\n");
    for (label, bytes, total) in [("stdout", out, out_total), ("stderr", err, err_total)] {
        let s = truncate(String::from_utf8_lossy(&bytes).into_owned(), total);
        let s = s.trim_end(); // trim_end, not trim: leading indentation matters
        if !s.is_empty() {
            text.push_str(&format!("{label}:\n{s}\n"));
        }
    }
    Ok(text)
}

// Keeps the first READ_LIMIT (+4, see read_file) bytes and returns the full
// byte count. It keeps reading past the cap and throws the rest away: if it
// stopped, the child would block on a full pipe and never exit.
async fn read_capped(mut r: impl AsyncRead + Unpin) -> std::io::Result<(Vec<u8>, u64)> {
    let mut kept = Vec::new();
    let mut total: u64 = 0; // u64, not usize: a 32-bit usize overflows at 4 GB
    let mut chunk = [0u8; 8192];
    loop {
        let n = r.read(&mut chunk).await?;
        if n == 0 {
            return Ok((kept, total));
        }
        total += n as u64;
        let room = (READ_LIMIT + 4).saturating_sub(kept.len());
        kept.extend_from_slice(&chunk[..n.min(room)]);
    }
}

// Reads at most READ_LIMIT bytes of a file inside the project (the cwd).
async fn read_file(path: &str) -> anyhow::Result<String> {
    // canonicalize resolves `..` and symlinks, so the prefix check can't be dodged
    let full = match tokio::fs::canonicalize(path).await {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("missing file {path}: {e}")
        }
        Err(e) => anyhow::bail!("failed to read file {path}: {e}"),
    };
    let root = std::env::current_dir()?.canonicalize()?;
    if !full.starts_with(&root) {
        anyhow::bail!(
            "{path} is outside the project; only files under {} can be read",
            root.display()
        );
    }

    let file = tokio::fs::File::open(&full)
        .await
        .with_context(|| format!("failed to read file {path}"))?;
    let total = file.metadata().await?.len();
    let mut bytes = Vec::new();
    // +4: one extra char past the limit, so we know whether we cut anything
    file.take(READ_LIMIT as u64 + 4)
        .read_to_end(&mut bytes)
        .await
        .with_context(|| format!("failed to read file {path}"))?;

    let contents = match String::from_utf8(bytes) {
        Ok(s) => s,
        // an incomplete char at the very end is our own cut; keep what's before it
        Err(e) if e.utf8_error().error_len().is_none() => {
            let valid = e.utf8_error().valid_up_to();
            let mut bytes = e.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).expect("valid up to this point")
        }
        Err(_) => anyhow::bail!("{path} is not UTF-8 text"),
    };

    Ok(truncate(contents, total))
}

fn truncate(mut contents: String, total: u64) -> String {
    if contents.len() > READ_LIMIT {
        let keep = contents.floor_char_boundary(READ_LIMIT);
        contents.truncate(keep);
        contents.push_str(&format!(
            "\n[truncated: showing the first {keep} of {total} bytes]."
        ));
    }
    contents
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_are_valid_object_schemas() {
        for spec in specs() {
            let p = &spec.parameters;
            assert_eq!(p["type"], "object", "{}", spec.name);
            for key in p["required"].as_array().expect("required list") {
                let key = key.as_str().unwrap();
                assert!(
                    p["properties"].get(key).is_some(),
                    "{}: required `{key}` missing from properties",
                    spec.name
                );
            }
        }
    }

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: "123".into(),
            name: name.into(),
            arguments: arguments.to_string(),
        }
    }

    // What the model would see: Ok text as-is, Err as its message chain.
    async fn model_sees(call: &ToolCall) -> String {
        match run(call, |_| true, DEFAULT_TIMEOUT).await {
            Ok(text) => text,
            Err(e) => format!("{e:#}"),
        }
    }

    #[tokio::test]
    async fn read_file_returns_contents() {
        let out = run(
            &call("read_file", json!({ "path": "Cargo.toml" })),
            |_| true,
            DEFAULT_TIMEOUT,
        )
        .await
        .unwrap();
        assert!(out.contains("[package]"), "{out}");
    }

    #[tokio::test]
    async fn missing_file_is_err_naming_the_path() {
        let err = run(
            &call("read_file", json!({ "path": "nope.txt" })),
            |_| true,
            DEFAULT_TIMEOUT,
        )
        .await
        .unwrap_err();
        // The model needs to know which path failed, not just "os error 2".
        assert!(format!("{err:#}").contains("nope.txt"), "{err:#}");
    }

    #[tokio::test]
    async fn bad_arguments_are_err() {
        let not_json = ToolCall {
            arguments: "{oops".into(),
            ..call("read_file", json!({}))
        };
        let err = model_sees(&not_json).await;
        assert!(err.contains("read_file") && err.contains("path"), "{err}");
        let err = model_sees(&call("read_file", json!({}))).await;
        assert!(err.contains("read_file") && err.contains("path"), "{err}");
        let err = model_sees(&call("read_file", json!({ "path": 7 }))).await;
        assert!(err.contains("read_file") && err.contains("path"), "{err}");
        let err = model_sees(&call("bash", json!({}))).await;
        assert!(err.contains("bash") && err.contains("command"), "{err}");
        let err = model_sees(&call("bash", json!({ "command": 7 }))).await;
        assert!(err.contains("bash") && err.contains("command"), "{err}");
    }

    #[tokio::test]
    async fn paths_outside_the_project_are_refused() {
        for path in ["/etc/hosts", "../", "src/../../"] {
            let out = model_sees(&call("read_file", json!({ "path": path }))).await;
            assert!(out.contains("outside the project"), "{path}: {out}");
        }
    }

    // The reason read_file canonicalizes: a link inside the project that points out.
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_out_of_the_project_is_refused() {
        let link = format!("target/capy_link_{}", std::process::id());
        std::os::unix::fs::symlink("/etc/hosts", &link).unwrap();
        let out = model_sees(&call("read_file", json!({ "path": link }))).await;
        std::fs::remove_file(&link).ok();
        assert!(out.contains("outside the project"), "{out}");
    }

    #[tokio::test]
    async fn non_utf8_file_is_err() {
        let path = format!("target/capy_bin_{}.bin", std::process::id());
        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        let out = model_sees(&call("read_file", json!({ "path": path }))).await;
        std::fs::remove_file(&path).ok();
        assert!(out.contains("not UTF-8"), "{out}");
    }

    #[tokio::test]
    async fn unknown_tool_names_the_tool() {
        let out = model_sees(&call("web_search", json!({}))).await;
        assert!(out.contains("unknown tool web_search"), "{out}");
        assert!(out.contains("read_file"), "{out}");
        assert!(out.contains("bash"), "{out}");
    }

    // Two files whose char boundaries fall on even vs odd bytes, so whatever
    // limit you pick, one of them puts the cut inside a 2-byte "é".
    #[tokio::test]
    async fn large_file_is_truncated_without_panic() {
        for (name, prefix) in [("even", ""), ("odd", "a")] {
            // inside the project (cargo runs tests from the crate root); pid avoids clashes
            let path = format!("target/capy_trunc_{}_{name}.txt", std::process::id());
            let body = format!("{prefix}{}", "é".repeat(500_000)); // ~1 MB
            std::fs::write(&path, &body).unwrap();

            let out = run(
                &call("read_file", json!({ "path": path })),
                |_| true,
                DEFAULT_TIMEOUT,
            )
            .await
            .unwrap();

            assert!(
                out.len() < body.len(),
                "{name}: not truncated. {}",
                out.len()
            );
            assert!(
                out.contains(&format!("of {} bytes", body.len())),
                "{name}: {}",
                &out[out.len() - 80..]
            );
            std::fs::remove_file(&path).ok();
        }
    }

    #[tokio::test]
    async fn run_command_echo_hi() {
        let out = model_sees(&call("bash", json!({ "command": "echo hi" }))).await;
        assert!(out.contains("exit status: 0"), "{out}");
        assert!(out.contains("hi"), "stdout missing hi: {out}");
    }

    #[tokio::test]
    async fn run_command_exit_3() {
        let out = model_sees(&call("bash", json!({ "command": "exit 3" }))).await;
        assert!(out.contains("exit status: 3"), "{out}");
    }

    // generous: bash can be slow to start when many tests spawn it at once
    const ENOUGH: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn failing_command_shows_stdout_stderr_and_code() {
        let out = run_command("echo partial; echo oops >&2; exit 3", ENOUGH)
            .await
            .unwrap();
        assert!(out.contains("exit status: 3"), "{out}");
        assert!(out.contains("partial"), "stdout lost on failure: {out}");
        assert!(out.contains("oops"), "stderr lost: {out}");
    }

    #[tokio::test]
    async fn signal_killed_command_is_reported() {
        // code() is None here; the model still needs to see something
        let out = run_command("kill -9 $$", ENOUGH).await.unwrap();
        assert!(out.contains("signal"), "{out}");
    }

    #[tokio::test]
    async fn slow_command_times_out_quickly() {
        let start = std::time::Instant::now();
        let err = run_command("sleep 5", Duration::from_millis(200))
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("timed out") && msg.contains("200ms"), "{msg}");
        assert!(msg.contains("partly run"), "{msg}");
        assert!(start.elapsed() < Duration::from_secs(2), "waited for sleep");
    }

    // kill_on_drop: without it, bash keeps running after the timeout and touches the file.
    #[tokio::test]
    async fn timed_out_command_is_killed() {
        let marker = format!("target/capy_killed_{}", std::process::id());
        let cmd = format!("sleep 0.5; touch {marker}");
        assert!(run_command(&cmd, Duration::from_millis(100)).await.is_err());
        tokio::time::sleep(Duration::from_millis(800)).await;
        let survived = std::path::Path::new(&marker).exists();
        std::fs::remove_file(&marker).ok();
        assert!(!survived, "command kept running after the timeout");
    }

    // Stdio::null: `cat` with no file reads stdin; inherited stdin would hang to the timeout.
    #[tokio::test]
    async fn command_gets_no_stdin() {
        let out = run_command("cat", ENOUGH).await.unwrap();
        assert!(out.contains("exit status: 0"), "{out}");
    }

    #[tokio::test]
    async fn runs_in_the_project_root() {
        let out = run_command("pwd", ENOUGH).await.unwrap();
        assert!(out.contains(env!("CARGO_MANIFEST_DIR")), "{out}");
    }

    #[tokio::test]
    async fn non_utf8_output_is_not_an_error() {
        let out = run_command(r"printf '\xff\xfe'", ENOUGH).await.unwrap();
        assert!(out.contains("exit status: 0"), "{out}");
    }

    #[tokio::test]
    async fn empty_command_is_err_without_asking() {
        for cmd in ["", "   ", "\n\t"] {
            let res = run(
                &call("bash", json!({ "command": cmd })),
                |_| panic!("asked to approve {cmd:?}"),
                DEFAULT_TIMEOUT,
            )
            .await;
            assert!(res.is_err(), "{cmd:?}: {res:?}");
        }
    }

    #[tokio::test]
    async fn huge_output_is_truncated() {
        // 200 KB is past both the cap and the pipe buffer: finishing at all
        // proves read_capped keeps draining
        let out = run_command("head -c 200000 /dev/zero | tr '\\0' a", ENOUGH)
            .await
            .unwrap();
        assert!(out.len() < 30_000, "not truncated: {}", out.len());
        assert!(
            out.contains("of 200000 bytes"),
            "{}",
            &out[out.len() - 80..]
        );
        assert!(out.contains("[truncated"), "{}", &out[out.len() - 80..]);
        assert!(out.starts_with("exit status: 0"), "exit code cut off");
    }

    #[tokio::test]
    async fn big_stderr_does_not_hide_stdout() {
        let cmd = "head -c 200000 /dev/zero | tr '\\0' w >&2; echo RESULT";
        let out = run_command(cmd, ENOUGH).await.unwrap();
        assert!(out.contains("RESULT"), "stdout lost behind stderr");
    }

    #[tokio::test]
    async fn leading_indentation_is_kept() {
        let out = run_command("printf '    indented\\n'", ENOUGH)
            .await
            .unwrap();
        assert!(out.contains("\n    indented"), "{out:?}");
    }
}
