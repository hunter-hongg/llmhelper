---
id: 0008
title: "request — OpenAI Compatible request command"
status: ready-for-agent
created: 2026-09-07
triage: ready-for-agent
---

## Problem Statement

`llmhelper` currently surfaces usage data read from local Agent/LLM sources but provides no means to interact with an OpenAI-compatible endpoint. Users need a CLI/TUI command to send requests to an OpenAI-compatible API, supporting both interactive exploration and scriptable invocation, with the same configuration and error handling conventions used by the rest of `llmhelper`.

## Solution

Add a new subcommand `llmhelper request` that sends an OpenAI-compatible Chat Completions request and renders the response. Two output modes: **CLI** (JSON output to stdout or plain text) and **TUI** (interactive viewer with streaming support). The command reuses `llmhelper` conventions: config file for defaults, path overrides, TUI default with `--json`/`--text` overrides, consistent error handling and Source-scoped thinking.

## User Stories

1. As a developer, I want to send a chat completion request to an OpenAI-compatible endpoint from the CLI so that I can script LLM calls without writing a wrapper.
2. As a developer, I want to specify the endpoint URL, API key, model, and optional headers via CLI flags or a config file so that I can switch providers without changing code.
3. As a user, I want a TUI viewer that displays the streaming response and usage metadata so that I can interactively explore completions.
4. As a user, I want `--json` output to return the full response object so that I can pipe it into other tools.
5. As a user, I want `--text` output to return only the assistant message content so that I can embed it in shell pipelines.
6. As a user, I want to pass messages as a JSON file, as a single prompt string, or interactively in the TUI so that I can test different prompt styles.
7. As a user, I want the request command to respect the same `--config` discovery and `--refresh-interval` conventions as other subcommands so that the tool feels consistent.
8. As a developer, I want errors (HTTP non-2xx, network, JSON parsing) surfaced clearly with exit code non-zero so that scripts can detect failures.
9. As a developer, I want request logging in `~/.config/llmhelper/logs/` with minimal PII so that debugging is possible without exposing keys.
10. As a power user, I want to supply temperature, top_p, max_tokens, stop sequences, and tools via flags or JSON so that I can control generation.
11. As a user, I want the TUI to show request/response headers, timing, token usage delta, and allow copy/share so that inspection is easy.

## Implementation Decisions

- New `Command::Request(RequestArgs)` variant in `cli.rs`. `RequestArgs` includes:
  - `--base-url` (required unless in config)
  - `--api-key` (optional, env `LLMHELPER_API_KEY` fallback)
  - `--model` (required)
  - `--messages` JSON file path or `--prompt` string (mutually exclusive with TUI interactive mode)
  - `--json`, `--text` output modes; TUI is default when neither is specified
  - `--temperature`, `--top-p`, `--max-tokens`, `--stop` (repeatable)
  - `--config` discovery via `Config::load` extended with a `[request]` section
- New `src/request.rs` module:
  - `RequestClient` with `reqwest` (blocking or async via tokio) to POST `/chat/completions` following OpenAI schema.
  - `RequestArgs` parsed via `clap` derives; JSON messages parsed with `serde_json::Value` for flexibility.
  - Pure function `build_payload` for unit testing.
  - `RequestResponse` struct holding status code, headers, usage, and content; serializable for `--json`.
- TUI integration:
  - Reuse `src/tui/` patterns: `ReportTuiApp`-style viewer for streaming text.
  - New `RequestTuiState` with scroll, streaming buffer, and header panel showing timing and token usage.
  - No background refresh loop; request is one-shot, streaming via `reqwest` chunked reader.
- Config:
  - Extend `Config` deserialization with `[request] base_url`, `api_key`, `default_model`, `timeout_seconds`.
  - Flags override config; env var overrides flag? Decision: CLI > env > config. API key never logged.
- Error handling:
  - Map `reqwest::Error` to user-friendly messages; non-2xx responses include response body snippet.
  - Exit codes: 0 success, 1 client error, 2 I/O/error.
- No new external crates beyond `reqwest` and `tokio`. If `reqwest` is not present, add to `Cargo.toml` with `json` feature.
- Security: API key read from flag, env, or config; never printed, masked in logs.
- Out of scope: streaming SSE parsing beyond SSE lines, tool/function calling, image inputs, OAuth flows.

## Testing Decisions

- Unit tests for payload builder: verify messages serialization, temperature defaults, stop sequence array, model field.
- Unit tests for config merge logic: flag > env > config precedence.
- Integration tests with a mock server (via `mockito` or `wiremock` in tests) to validate request/response roundtrip, error handling, and streaming line parsing.
- No TUI automated tests; rely on manual PTY smoke and state unit tests mirroring existing `report` TUI tests.
- Existing `usage`, `diff`, `sessions`, `report` tests must remain unaffected.

## Out of Scope

- OpenAI-specific features beyond Chat Completions (e.g., embeddings, completions, fine-tuning).
- Authentication beyond Bearer token.
- Proxy configuration.
- Multi-turn conversation persistence across invocations.
- Automatic retry with exponential backoff.
- Editing messages in TUI; TUI is read-only viewer.

## Further Notes

- Keep `request` read-only with respect to local usage data; it does not write into Source DBs.
- Align README with new `request` subcommand and example usage.
- Follow existing code conventions: `src/tui/` patterns, `Config::load` merge, `clap` derives, no comments in code per project norm.
