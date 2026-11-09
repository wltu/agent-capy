# agent-capy

[![CI](https://github.com/wltu/agent-capy/actions/workflows/ci.yml/badge.svg)](https://github.com/wltu/agent-capy/actions/workflows/ci.yml)

A terminal AI coding agent in Rust, shipped as one binary. It runs Claude or local models through a tool-use loop to read, edit and run code, and it asks before risky actions. Supports MCP tools, saved sessions and a headless mode for scripts, and is meant to be the base for multi-agent pipelines.

## Status

Phase 1 done. `capy` takes one task, runs a tool-use loop against a local model through Ollama's OpenAI-compatible API, and prints the answer. It has two tools: `read_file` and `bash`. The Gate 1 run against a real repo is in [`evals/gate1.md`](evals/gate1.md).

v1 roadmap:

- [x] Single model call to a local model (Ollama)
- [x] Agent loop with tool use
- [ ] Built-in tools: read, write, edit, glob, grep, bash (read and bash done)
- [ ] Permission prompts and a config allowlist (prompts done)
- [ ] Anthropic API behind a shared `Provider` trait
- [ ] Streaming output
- [ ] Saved, resumable sessions
- [ ] Context compaction
- [ ] MCP client support
- [ ] Headless mode (`--print`, JSON output)

## Try it

Needs Rust and a running [Ollama](https://ollama.com) server with the model pulled (`ollama pull qwen3.8-27b`).

```sh
cargo install --path .
cd some/project
capy "what does this repo do?"
```

The directory you start `capy` in is the project root. The answer goes to stdout; reasoning, tool calls and token counts go to stderr.

| Flag             | Default                     |                                       |
| ---------------- | --------------------------- | ------------------------------------- |
| `-e, --endpoint` | `http://localhost:11434/v1` | OpenAI-compatible base URL            |
| `-m, --model`    | `qwen3.8-27b`               | Model name                            |
| `--max-turns`    | `20`                        | Model calls before giving up (exit 1) |
| `--dry-run`      |                             | Print the request JSON and exit       |

## Approval

`read_file` runs without asking but refuses paths outside the project root. Every `bash` command is shown, with control and invisible characters escaped, and runs only if you answer `y`. Anything else, including Enter or EOF on piped stdin, declines. A declined command goes back to the model as a normal result.

## Known limits

- `bash` can read anything your user can; the approval prompt is the only guard. Read-only commands ask too.
- Tool output is cut at 10 KB per file or stream. `read_file` has no offset, so the rest of a large file needs `bash`.
- Commands are killed after 30 seconds, and there's no stdin, so interactive programs fail.
- One task per run: no REPL, and the model can't wait for your reply when it asks a question.
- No streaming; you wait for each full reply.

## License

MIT
