# agent-capy

[![CI](https://github.com/wltu/agent-capy/actions/workflows/ci.yml/badge.svg)](https://github.com/wltu/agent-capy/actions/workflows/ci.yml)

A terminal AI coding agent in Rust, shipped as one binary. It runs Claude or local models through a tool-use loop to read, edit and run code, and it asks before risky actions. Supports MCP tools, saved sessions and a headless mode for scripts, and is meant to be the base for multi-agent pipelines.

## Status

Early development. Today `capy` sends one hardcoded prompt to a local model through Ollama's OpenAI-compatible API and prints the reply, any reasoning, and token usage.

v1 roadmap:

- [x] Single model call to a local model (Ollama)
- [ ] Agent loop with tool use
- [ ] Built-in tools: read, write, edit, glob, grep, bash
- [ ] Permission prompts and a config allowlist
- [ ] Anthropic API behind a shared `Provider` trait
- [ ] Streaming output
- [ ] Saved, resumable sessions
- [ ] Context compaction
- [ ] MCP client support
- [ ] Headless mode (`--print`, JSON output)

## Try it

Needs Rust and a running [Ollama](https://ollama.com) server (`ollama serve`) with the model pulled (`ollama pull qwen3.8-27b`).

```sh
cargo run
```

To use a different server, set `CAPY_BASE_URL` (default `http://localhost:11434/v1`).

## License

MIT
