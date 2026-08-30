# 04 — Claude Code adapter (JSONL → Record)

**What to build:** A `ClaudeSource` impl of `Source` that reads Claude Code transcript JSONL and emits `Record`s. It walks a projects directory, parses each `*.jsonl` session, extracts `type == "assistant"` messages, sums their `usage` blocks into the five canonical token fields (input_tokens→input, output_tokens→output, reasoning_tokens→reasoning, cache_read_input_tokens→cache_read, cache_creation_input_tokens→cache_write), takes the last non-null per-message `model` as the session model (raw value — `auto` kept as-is per ADR-0004), decodes the hyphen-encoded project folder name into a path, and sets `cost: None`. Lines that aren't assistant messages are skipped.

**Blocked by:** 03 — Source trait + registry (must impl `Source`).

**Status:** ready-for-agent

- [ ] `ClaudeSource::new(dir: PathBuf)` constructs a source over a Claude projects directory
- [ ] Hyphen-encoded folder names decode correctly (`-home-hunter-projects-x` → `/home/hunter/projects/x`)
- [ ] Per-message `usage` blocks summed into `TokenBreakdown`; the five-field mapping matches the spec table exactly
- [ ] Session-level model = last non-null assistant-message `model`; `"auto"` passed through unchanged; `"unknown"` if no assistant messages
- [ ] `cost` is always `None` for Claude records
- [ ] A unit test feeds a fixture JSONL (3 assistant messages with known token sums + `model:"auto"`) and asserts the emitted `Record` token sums, model, and project path
