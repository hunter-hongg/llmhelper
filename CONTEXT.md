# LLM Helper

A Rust + Clap command-line tool for Agent/LLM usage introspection. The `usage` subcommand is the first shipped command: it reads local usage data from Claude Code and OpenCode and renders it in a ratatui terminal UI.

## Language

**Usage**:
The token consumption and activity of a single agent session, broken into input, output, reasoning, cache-read, and cache-write tokens, plus session and message counts and an active time window. Cost is an optional attribute — present only for sources that record it.
_Avoid_: stats, metrics, usage-data

**Source**:
One of the agent tools whose local files the CLI reads. The two known sources are Claude Code (transcript JSONL) and OpenCode (SQLite). Each source has a distinct storage format and field coverage. Sources are auto-discovered at default paths and may be overridden by flags or a config file; reading all OpenCode DB variants and merging them is part of Source behavior.
_Avoid_: provider, backend, agent

**Project**:
The working directory a Session ran in. Claude Code encodes it into the transcript folder name; OpenCode stores it in the session's `directory` field. Used to group Sessions across sources.
_Avoid_: repo, workspace

**Cost**:
The monetary spend attributed to a Session by a Source that records it. OpenCode exposes it directly; Claude Code does not, so its Cost is absent. Cost is always source-scoped — it must never be summed or averaged across Sources, only displayed per Source.
_Avoid_: price, spend, expense

**Token Breakdown**:
The canonical five-part token accounting of a Session: input, output, reasoning, cache_read, cache_write. Every Source maps its raw token fields onto these five; there is no source-specific token field downstream.
_Avoid_: token-count, token-usage

**Model**:
The model identifier a Session ran against, as recorded by the Source (raw value). Claude Code often records the routing alias `auto`; OpenCode records a resolved id (e.g. `big-pickle`). Grouping is by this raw value — `auto` is a known routing-alias bucket, not a resolved model, and is shown as-is rather than guessed.
_Avoid_: engine, model-name



