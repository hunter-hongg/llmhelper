---
id: 0008
title: "request — OpenAI Compatible request command"
status: done
created: 2026-09-07
triage: done
---

## Problem Statement

`llmhelper` currently surfaces usage data read from local Agent/LLM sources but provides no means to interact with an OpenAI-compatible endpoint. Users need a CLI/TUI command to send requests to an OpenAI-compatible API, supporting both interactive exploration and scriptable invocation, with the same configuration and error handling conventions used by the rest of `llmhelper`.

## Solution

Add a new subcommand `llmhelper request` that sends an OpenAI-compatible Chat Completions request and renders the response. Each of the three output modes — **TUI** (default), `--json`, and `--text` — has both a one-shot form and a streaming form selected by `--stream`. Streaming is specified by its companion document, `docs/specs/0009-request-stream.md`; this spec covers the one-shot contract. Reasoning (thinking) capture, the `--reasoning-field`/`--reasoning`/`--thinking` flags, and the TUI thinking pane are specified by the reasoning companion, `docs/specs/0013-request-reasoning.md`. The command reuses `llmhelper` conventions: config file for defaults, path overrides, TUI default with `--json`/`--text` overrides, consistent error handling and Source-scoped thinking.

## User Stories

1. As a developer, I want to send a chat completion request to an OpenAI-compatible endpoint from the CLI so that I can script LLM calls without writing a wrapper.
2. As a developer, I want to specify the endpoint URL, API key, model, and optional headers via CLI flags or a config file so that I can switch providers without changing code.
3. As a user, I want a TUI viewer that displays the response and usage metadata so that I can interactively explore completions.
4. As a user, I want `--json` output to return the full response object so that I can pipe it into other tools.
5. As a user, I want `--text` output to return only the assistant message content so that I can embed it in shell pipelines.
6. As a user, I want to pass messages as a JSON file or as a single prompt string so that I can test different prompt styles.
7. As a developer, I want errors (HTTP non-2xx, network, JSON parsing) surfaced clearly with a non-zero exit code so that scripts can detect failures.
8. As a power user, I want to supply temperature, top_p, max_tokens, and stop sequences via flags so that I can control generation.
9. As a user, I want the TUI to show the host, model, elapsed time, and token usage so that inspection is easy.

## Implementation Decisions

 - New `Command::Request(RequestArgs)` variant in `cli.rs`. `RequestArgs` includes:
  - `--base-url` (required unless in config)
  - `--api-key` (optional, env `LLMHELPER_API_KEY` fallback)
  - `--model` (required)
  - `--messages` JSON file path or `--prompt` string, mutually exclusive with each other
  - `--json`, `--text` output modes; TUI is default when neither is specified
  - `--temperature`, `--top-p`, `--max-tokens`, `--stop` (repeatable)
  - Config discovery via `Config::load`, extended with a `[request]` section. There is no `--config` flag in this spec; a global `--config` flag was added later, see `docs/specs/0012-request-enhancements.md`. Without it the path is `~/.config/llmhelper/config.toml`.
- New `src/request.rs` module:
  - `send_chat_completion` posts the payload to `/v1/chat/completions` following the OpenAI schema, using `reqwest` under a `tokio` runtime.
  - `RequestArgs` parsed via `clap` derives; JSON messages parsed with `serde_json::Value` for flexibility.
  - Pure function `build_payload` for unit testing.
  - `RequestResponse` struct holding status code, the raw provider response, usage, and assistant content; serializable for `--json`.
- TUI integration:
  - Reuse `src/tui/` patterns: `ReportTuiApp`-style viewer for streamed and one-shot text.
  - New `RequestTuiState` with scroll, response body, and header panel showing host, model, elapsed time, and token usage.
  - No background refresh loop; the request is one-shot. Streaming delivery is specified in `docs/specs/0009-request-stream.md`.
- Config:
  - Extend `Config` deserialization with `[request] base_url`, `api_key`, `default_model`, `timeout_seconds`.
   - Flags override config. Decision: CLI > env > config. API key never logged.
  - `--refresh-interval` does not apply to this command; the request path is one-shot and reads no refresh interval.
- Error handling:
  - Map `reqwest::Error` to user-friendly messages; non-2xx responses include response body snippet.
  - Exit codes: 0 on success, a single non-zero code for both HTTP client errors and request failures. This is the behaviour the binary implements. The earlier draft of this spec documented a distinct code 2 for I/O errors; that distinction was never implemented and is recorded here as a gap rather than silently dropped.
- No new external crates beyond `reqwest` and `tokio`. If `reqwest` is not present, add to `Cargo.toml` with `json` feature.
- Security: API key read from flag, env, or config; never printed, masked in logs.

## Testing Decisions

- Unit tests for the payload builder: verify messages serialization, model field, sampling parameter omission when unsupplied, and stop sequence array.
- Unit tests for config resolution: verify CLI flag > env > config precedence for base URL, API key and model.
- Integration tests drive the real binary against a local HTTP server that records the request it received. No mocking crate is introduced. These cover request/response roundtrip and non-2xx error handling.
- Streaming tests, including SSE line parsing, live with the streaming work and are specified in `docs/specs/0009-request-stream.md`.
- No TUI automated tests; rely on manual PTY smoke and state unit tests mirroring existing `report` TUI tests.
- Existing `usage`, `diff`, `sessions`, `report` tests must remain unaffected.

## Out of Scope

- OpenAI-specific features beyond Chat Completions (e.g., embeddings, completions, fine-tuning).
- Tool and function calling.
- Authentication beyond Bearer token.
- Proxy configuration.
- Multi-turn conversation persistence across invocations.
- Automatic retry with exponential backoff.
- Request logging. The request path writes no logs; debugging relies on the error message returned to the caller. (Superseded: `request --log` was added in `docs/specs/0012-request-enhancements.md`.)
- TUI message editing and interactive TUI message input. The viewer is read-only, and input must come from `--messages` or `--prompt`.
- Displaying request or response headers in the TUI, and copy/share actions.

## Further Notes

- Keep `request` read-only with respect to local usage data; it does not write into Source DBs.
- Reasoning capture is not covered here; see `docs/specs/0013-request-reasoning.md`.
- Tool pass-through (`--tools`), request logging (`--log`), split exit codes, the global `--config` flag, and interactive TUI mode are covered by `docs/specs/0012-request-enhancements.md`.
- README documents the `request` subcommand with example usage, including the streaming forms.
- Follow existing code conventions: `src/tui/` patterns, `Config::load`, `clap` derives, no comments in code per project norm.
