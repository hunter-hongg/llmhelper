---
id: 0012
title: "request — tool calling, logging, exit codes, --config, interactive, search fail-open, --copy, release metadata"
status: ready-for-agent
created: 2026-09-11
---

# request enhancements and CLI hardening

## Summary

Nine changes spanning the `request` subcommand, `search` subcommand, global CLI
surface, and release metadata. No existing behavior is broken; every change is
additive or a documented correction of a prior gap.

## Changes

### 1. `request --tools` (pass-through tool calling)

`--tools <file>` accepts a path to a JSON file containing an array of OpenAI
tool definitions. The array is embedded verbatim in the `tools` field of the
Chat Completions request body. No client-side JSON Schema validation is
performed — the file must be a valid JSON array, and each entry is passed
through to the provider as-is. The flag is mutually exclusive with nothing; it
composes with `--stream`, `--json`, `--text`, and `--interactive`.

- `build_payload` gains a `tools: Option<&Value>` parameter.
- `load_tools_file(path)` validates the file is a JSON array and returns the
  parsed `Value`.
- When `--tools` is absent, the `tools` key is omitted from the payload.

### 2. `request --log` (opt-in request/response logging)

`--log` enables appending the request payload and response body (or the full
SSE stream for `--stream`) to a per-day log file under
`~/.config/llmhelper/logs/`. The file is named
`request-YYYY-MM-DD.log`. Each entry is a JSON object with `timestamp`,
`direction` (`request` or `response`), and `body`. The API key header is never
written. Default is off. Log write failures are non-fatal (warn to stderr).

- `write_request_log(dir, direction, body)` in `request.rs`.
- `log_dir()` returns `~/.config/llmhelper/logs/`.
- The `run_request` function calls the logger after building the payload and
  after receiving the response (or after the stream completes).

### 3. Exit code split for `request`

- **Exit 0**: success.
- **Exit 1**: request-level failure — HTTP non-2xx status, response JSON parse
  error, or empty content when `--text` is set.
- **Exit 2**: client-level failure — connection refused, DNS failure, timeout,
  or stream interruption.

`RequestError` enum in `request.rs` replaces bare `anyhow::Error` for the
two send functions. `main.rs` maps the variant to the process exit code.

### 4. `--config` global flag

`--config <path>` is a global flag on the top-level `Cli` struct. When set,
`Config::load_with(path)` reads the TOML from the given path instead of the
default `~/.config/llmhelper/config.toml`. When absent, behavior is unchanged.
The flag applies to all subcommands uniformly.

- `Config::load_with(path: Option<&Path>) -> Config` in `config.rs`.
- All `run_*` functions in `main.rs` accept the config path.

### 5. `request --interactive` (TUI multi-turn)

`--interactive` opens the request TUI with an input line at the bottom. After
the initial response renders, the user can type a follow-up message and press
Enter to send it. Each follow-up appends a user message to the conversation
history and re-sends the full message list. The response body is replaced with
the new response. `q` or `Esc` exits.

- `RequestTuiState` gains `input: String`, `interactive: bool`, and
  `sending: bool`.
- `run_request_tui` in `main.rs` handles key events: printable characters
  append to `input`, Backspace removes, Enter triggers a re-send.
- Re-sends run on the same tokio runtime via `rt.spawn`.
- The TUI footer shows `type a message, Enter to send, q to quit` in
  interactive mode.
- `--interactive` is mutually exclusive with `--json` and `--text` (those
  modes are non-interactive).

### 6. `search --fail-open` (untimestamped messages pass time filters)

`--fail-open` allows messages without a recorded timestamp to pass through
`--since` and `--last` time predicates. Default behavior (fail-closed) is
unchanged: untimestamped messages are excluded when a time filter is active.
This flag only affects `search`, not `usage`, `diff`, `report`, or `sessions`,
because records always have a timestamp.

- `Filter` gains a `fail_open: bool` field (default `false`).
- `matches_at` checks `fail_open` before rejecting untimestamped entries.
- `SearchArgs` gains `--fail-open`.
- `build_filter_search` sets `fail_open` from the flag.

### 7. `request --copy` (OSC 52 clipboard)

`--copy` writes the assistant content to the terminal clipboard using the
OSC 52 escape sequence `\x1b]52;c;<base64>\x07`. This works in terminals that
support OSC 52 (iTerm2, Alacritty, Kitty, WezTerm, foot, etc.) without
requiring a system clipboard utility. The flag is mutually exclusive with
`--json` and `--text` (it only makes sense in TUI mode, where the content is
already rendered — `--copy` additionally copies it). In practice, `--copy`
implies TUI mode; if `--json` or `--text` is also set, the flag is rejected.

- `copy_to_clipboard(text: &str)` in `request.rs` emits the OSC 52 sequence.
- Called after the TUI exits successfully in `run_request`.

### 8. `--refresh-interval` consistency verification

`request` and `report` are both one-shot commands with no refresh loop. The
`refresh_interval_seconds` config key applies only to the long-running TUIs
(`usage`, `diff`, `sessions`, `search`). No code change is needed; this
ticket verifies the existing documentation is correct and closes the gap.

### 9. Cargo.toml release metadata

Adds `description`, `license`, `repository`, `homepage`, and `keywords`
fields to the `[package]` section for `cargo publish` readiness.

## Testing

- Unit tests for `build_payload` with tools, `load_tools_file`,
  `RequestError` variants, `copy_to_clipboard`, and `Filter.fail_open`.
- Integration tests in `tests/request.rs` for `--tools` payload passthrough
  and exit-code verification.
- Existing test suite must pass unchanged.
