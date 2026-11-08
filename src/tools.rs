use crate::message::ToolCall;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

const READ_LIMIT: usize = 20_000;

#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "read_file".into(),
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
        // TODO(step 5): the approval prompt is the safety gate; no command runs without it
        ToolSpec {
            name: "bash".into(),
            description: "Runs a shell command in the project root; the user approves each command; it returns stdout, stderr and the exit code".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Command to run in the project root" }
                },
                "required": ["command"]
            }),
        },
    ]
}

#[derive(Deserialize)]
struct ReadFileArgs {
    path: String,
}

pub async fn run(call: &ToolCall) -> anyhow::Result<String> {
    match call.name.as_str() {
        "read_file" => {
            let args: ReadFileArgs = serde_json::from_str(&call.arguments)?;
            read_file(&args.path).await
        }
        other => anyhow::bail!("{other} is not available"),
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

    let mut contents = match String::from_utf8(bytes) {
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
    if contents.len() > READ_LIMIT {
        let keep = contents.floor_char_boundary(READ_LIMIT);
        contents.truncate(keep);
        contents.push_str(&format!(
            "\n[truncated: showing the first {keep} of {total} bytes]."
        ));
    }
    Ok(contents)
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
        match run(call).await {
            Ok(text) => text,
            Err(e) => format!("{e:#}"),
        }
    }

    #[tokio::test]
    async fn read_file_returns_contents() {
        let out = run(&call("read_file", json!({ "path": "Cargo.toml" })))
            .await
            .unwrap();
        assert!(out.contains("[package]"), "{out}");
    }

    #[tokio::test]
    async fn missing_file_is_err_naming_the_path() {
        let err = run(&call("read_file", json!({ "path": "nope.txt" })))
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
        assert!(run(&not_json).await.is_err());
        assert!(
            run(&call("read_file", json!({}))).await.is_err(),
            "missing path"
        );
        assert!(
            run(&call("read_file", json!({ "path": 7 }))).await.is_err(),
            "path not a string"
        );
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
    async fn bash_says_not_available() {
        let out = model_sees(&call("bash", json!({ "command": "ls" }))).await;
        assert!(out.contains("not available"), "{out:?}");
    }

    #[tokio::test]
    async fn unknown_tool_names_the_tool() {
        let out = model_sees(&call("rm_rf", json!({}))).await;
        assert!(out.contains("rm_rf"), "{out:?}");
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

            let out = run(&call("read_file", json!({ "path": path })))
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
}
