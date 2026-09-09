---
id: 0010
title: "search — full-text search across session transcripts"
status: ready-for-agent
created: 2026-09-09
updated: 2026-09-10
triage: ready-for-agent
---

## Problem Statement

Every existing subcommand operates at the Session level. `Record` carries only aggregated token counts, cost, and timestamps; the actual text of user prompts and assistant replies is discarded during `Source::load()`. There is therefore no way to answer "what did we say about X?", "which session introduced this regex?", or "where did we decide to use this approach?" The `sessions` spec (0003) explicitly puts message-level transcript content out of scope, leaving a gap: this CLI can tell you how much an agent spent, but not what it said.

All four Sources record message text locally, in formats that are already read by the existing adapters:

- **Claude Code** — transcript JSONL; `type: "user"` has `message.content` as a string or a block list, `type: "assistant"` has `message.content` as a list of `text` / `thinking` / `tool_use` blocks.
- **OMP** — per-session JSONL; `type: "message"` has `message.content` as a block list with roles `user`, `assistant`, `developer`, `toolResult`.
- **OpenCode** and **Kilo Code** — SQLite; `message` rows hold metadata and `part` rows hold `{"type":"text","text":...}` or `{"type":"reasoning","text":...}`. Kilo's own `recall_part_search_idx` index over `part` text confirms this table is the intended search surface.

## Solution

Add a `search` subcommand: `llmhelper search <query>` performs full-text matching over message text across all configured Sources, reusing the same filter vocabulary as the other read-only commands (`--since`/`--last`, `--project`, `--model`, `--source`) and defaulting to the TUI per repo convention. The TUI is a list of hits with `Enter` opening the full message text, mirroring the `sessions` list+detail pattern. `--json`, `--csv`, and `--text` provide non-TUI output.

Search is implemented as a pure, independently testable function over an in-memory message slice; the Sources only add a second read path. Matching is case-insensitive substring by default, with `--case-sensitive` and `--role` to narrow.

## User Stories

1. As a developer, I want to search all of my agent transcripts for a term so that I can find a prior decision or command without reading sessions one by one.
2. As a developer, I want each hit to show a snippet with the match in context so that I can tell whether a result is relevant without opening it.
3. As a developer, I want hits to show source, session id, project, role, and timestamp so that I can navigate to the exact place a match occurred.
4. As a developer, I want to press `Enter` on a hit to read the full message text so that long responses are not truncated by the snippet.
5. As a developer, I want to filter a search by project, model, source, or time window so that I can scope a search the same way I scope `usage` or `sessions`.
6. As a developer, I want `--role` so that I can restrict a search to user prompts, assistant replies, or model thinking.
7. As a developer, I want `--case-sensitive` so that I can find an exact symbol or identifier when case matters.
8. As a developer, I want `--json` so that scripts can consume hits programmatically.
9. As a developer, I want `--csv` so that hits can be pasted into a spreadsheet.
10. As a developer, I want `--text` so that I can pipe a plain hit list into `head` or `grep`.
11. As a user, I want to see how many messages each Source contributed to the filtered corpus so that I know whether a search covered the corpus I expected.
12. As a user, I want an empty result to be a clean "no matches" state rather than an error, so that a negative search is informative.

## Implementation Decisions

- **New domain type `Message`** in `src/domain/message.rs`: `source`, `session_id`, `project`, `model: Option<String>`, `role`, `timestamp: Option<DateTime<Utc>>`, `text`. Role is a raw string, consistent with how `Model` is treated as a recorded value rather than a resolved one. Reasoning and thinking blocks are stored as messages with `role: "thinking"` rather than as a separate field, so downstream code never needs a source-specific token notion.
- **`Source` trait gains `load_messages(&self) -> Result<Vec<Message>, SourceError>`** with a default implementation returning `Ok(Vec::new())`. This follows ADR 0002: a new Source is still one struct registered in the `Registry`, with no change to any dispatch site, and a Source that stores no text degrades to an honest empty set without a special case.
- **`Registry::load_messages_all(&self) -> (Vec<Message>, Vec<MessageStatus>)`** mirrors `load_all()` and reuses `SourceError` for per-Source absence/unreadable handling. `SourceStatus` is left untouched so `usage`, `diff`, `sessions`, and `report` are unaffected; search does not need `Record` because `Filter` is extended to apply to messages directly.
- **`Filter` gains `matches_message(&self, m: &Message) -> bool`**, sharing predicate logic with the existing `matches(&Record)` through a private helper so the two cannot drift. Every `Filter` field (`since`, `last`, `until`, `project`, `model`, `source`) is present on `Message`, so the same vocabulary applies — but the two predicates are deliberately **not** equivalent on time: `Record.started_at` is a required `DateTime<Utc>` while `Message.timestamp` is `Option<DateTime<Utc>>`, and a message with no timestamp **fails** every time predicate (fail closed). This is the correct call for search: an untimed message is unverifiable against a window, so admitting it would silently return out-of-window results. `--project` narrows messages directly, so it composes with `--json`, `--csv`, and `--text` and is rejected by nothing.
- **`src/search.rs`** holds the pure engine: `SearchOptions`, `SearchHit`, `search()`, and `snippet()`. Hits carry `matches` (count of occurrences in the message), `snippet`, and the full `text` for the TUI detail view. Snippet extraction is a standalone pure function: it finds the first match, takes a character window around it, replaces newlines with single spaces, and marks both ends with `…` when truncated. Snippet extraction must be tested in isolation.
- **Matching is substring only.** No regex, no word boundaries, no stemming. A single query is required and matching is case-insensitive unless `--case-sensitive` is given.
- **Ranking** is by match count descending, then timestamp descending, so the most repeated and most recent matches surface first.
- **`toolResult` and `tool_use` content is excluded from the searchable corpus.** OMP alone records thousands of tool-result blocks that are file and command dumps; including them would drown real conversation. Reasoning blocks are included, tagged as `thinking`, because they are model-authored text and genuinely searchable — but they are a distinct role the user can filter out with `--role`.
- **Output modes.** The TUI is the default and reuses the `sessions` list+detail structure: a header showing the query, the active non-default filters (`--project`, `--model`, `--source`, `--since`, `--last`, `--context`, `--limit`) as one muted line, and per-Source message counts, then a hit table (role, source, project, time, match count, snippet) and `Enter` for a scrollable full-text detail view with `Esc` returning to the list. `r` re-runs the search manually. Search is static rather than background-refreshed: unlike `usage`, a search does not need to watch for new sessions, and rescanning hundreds of megabytes of transcripts on a timer is wasteful. The header shows the active filters rather than implying an unscoped search, which would mislead a scoped query. The session id is not a list column — it is a long opaque identifier and `Project` already carries the navigation value — but it appears in the detail title and in all three non-TUI outputs. `--text` prints a plain hit list; `--json` prints the query, options, hits, and per-Source message counts; `--csv` prints a flat hit table.
- **The per-Source message counts describe the corpus that was searched.** They are recomputed after the filter is applied, not echoed from the raw load, so a `--project` search that excludes three Sources reports `0` for them instead of implying they were scanned. Load errors still surface per Source, so an absent or unreadable Source is distinguishable from a Source that simply held no matching messages.
- **Exit code is 0 with no matches.** An empty result is a valid answer to a search, not a failure; the TUI shows a "no matches" state and `--json` emits an empty hits array.

## Testing Decisions

- **Unit tests** in `src/search.rs` cover snippet extraction (match at the very start, the very end, and the middle; truncation markers; single-line output; no-match text) plus `search()` semantics (case-insensitive default, `--case-sensitive`, multiple matches counted, role filter, limit, ranking order, and messages with no timestamp).
- **Fixture extension.** The existing fixtures must gain message text so the adapters are exercised end to end: add `content` blocks to assistant lines and a string `content` to a user line in the Claude fixture; add a user message with `content` blocks and a `custom_message` to the OMP fixture; create `message` and `part` tables in the OpenCode fixture DB; add `part` rows to the Kilo fixture DB. The Kilo fixture intentionally keeps one session whose messages have no text parts, so the honest empty-result path is covered by a real fixture rather than only by a unit test.
- **Integration tests** run `search --json` against the fixture paths with `--claude-dir`, `--opencode-db`, `--omp-dir`, and `--kilo-db`, asserting hit counts, snippet content, ranking order, every filter (`--case-sensitive`, `--role`, `--source`, `--project`, `--since`, `--last`, `--limit`, `--context`), the honest zero-message path for a Source with no searchable text (built in a temp database, not only a unit test), the post-filter per-Source counts, CSV and `--text` output, and each CLI validation error.
- **TUI state tests** cover the list+detail state machine, including a regression test that a freshly constructed state has the first hit selected — without it `Enter` and the arrow keys are all no-ops on first interaction, because ratatui's `TableState::default()` starts with no selection.
- Existing record-level tests must stay green: adding text to fixture lines must not change any token, cost, timestamp, or model assertion, because the record adapters ignore content fields entirely.

## Out of Scope

- **Message-level transcript browsing or export** with no query — that remains out of scope for `sessions` and is a separate future command.
- **Regex, glob, fuzzy, and typo-tolerant matching.** Substring only in this iteration.
- **Semantic or embedding-based search.** No vector store, no model calls.
- **Tool output and code diff search.** `toolResult`, `tool_use`, `patch`, and `image` parts are excluded from the corpus.
- **Cross-session ranking or relevance scoring** beyond match count and recency.
- **Incremental indexing.** The corpus is scanned on each run; caching is a future concern.
- **Editing, deleting, or annotating messages.** This command is read-only, like the other four.

## Further Notes

- Kilo Code's schema is identical to OpenCode's (`message` + `part`, both with a `data` JSON column), so one SQLite query serves both adapters, and the per-database loop plus cross-database dedup is shared too. What stays per-Source is only the database discovery and the source name, which preserves per-Source error semantics. A session claimed by an earlier database keeps all of its messages and drops that session's copies from later databases, mirroring the record dedup — so a session split across two databases cannot produce duplicate messages.
- Kilo's `recall_part_search_idx` proves that `part` is the canonical text store for these SQLite Sources and that the `synthetic` and `ignored` flags on text parts exist to mark non-user-facing content. Those flagged parts are excluded from the searchable corpus.
- `Message.model` is frequently absent for user messages across all Sources; `--model` therefore narrows only messages that recorded a model. This is a known asymmetry and is acceptable because model filtering is optional.
- Because search reads message content, it is the first command to expose raw transcript text on screen. That is the point of the command, but it means output should not be copied verbatim into automated reporting.
- The `AGENT_CHANGELOG.md` entry for this feature must record the message-count corpus size per Source, since that is the number a user needs to interpret any search result.
