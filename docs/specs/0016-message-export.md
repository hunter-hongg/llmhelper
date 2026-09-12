---
id: 0016
title: "message export — transcript-level export of the search corpus"
status: done
created: 2026-09-13
updated: 2026-09-13
triage: done
---

## Problem Statement

`export` (spec 0014) emits one row per **Session**: aggregated token counts,
cost, and timestamps. `search` (spec 0010) is the only command that reads
message-level transcript text, and it only ever returns **hits for a query** —
it cannot dump the corpus. Spec 0010 names the gap explicitly: "Message-level
transcript browsing or export with no query — that remains out of scope for
`sessions` and is a separate future command."

So there is no way to answer "give me every assistant message from the last
week as JSONL", "dump this project's transcript to CSV for a spreadsheet", or
"feed my local transcripts into a RAG pipeline". Every message the Sources
already parse for `search` is readable in memory, but the only exit is through
a query or a full-screen TUI. Downstream tooling has no machine-readable door.

## Solution

Extend `export` with a `--messages` switch that flips its unit of export from
Session to Message. With `--messages`, `export` loads through the existing
`Registry::load_messages_all`, applies the existing `Filter` via
`matches_message`, and emits one row per `Message` using the same
`--format` (`jsonl`/`json`/`csv`/`tsv`) and `--fields` projection machinery
already shipped for Records.

This is deliberately an **extension of `export`, not a new subcommand**:
Records and Messages are the two units the same data model already exposes,
and a user asking for "a flat dump of my agent data" should not have to learn
two commands. Session-level export is unchanged and remains the default.

A `--role` filter is added, mirroring `search --role`, because the message
corpus mixes `user`, `assistant`, and `thinking` rows and a consumer usually
wants exactly one of them.

## User Stories

1. As a developer, I want `llmhelper export --messages` to dump every
   transcript message I have as JSONL so that I can build a dataset without
   opening a TUI.
2. As a developer, I want `--messages --role assistant` so that I can export
   only assistant replies and skip the user/thinking rows.
3. As a developer, I want the same `--format json|csv|tsv` as session export
   so that the transcript dump drops into the same scripts.
4. As a developer, I want `--fields` to select and order message columns so
   that a pipeline that only needs `session_id,role,text` is not forced to
   carry the rest.
5. As a developer, I want every existing read-only filter (`--since`/`--last`,
   `--project`, `--model`, `--source`) to narrow the message set exactly as it
   narrows sessions, so that `--source claude --last 7d --messages` scopes my
   export the same way `usage` does.
6. As a developer, I want a message with no timestamp to fail a time filter
   rather than sneak into a windowed export, so that `--last 1d` means what it
   says.
7. As a developer, I want rows ordered deterministically so that two identical
   invocations are byte-identical and diffable.
8. As a user, I want a Source that fails to load to warn on stderr without
   aborting the export, so a partially-readable corpus still exports.
9. As a user, I want a zero-match filter to exit 0 with clean output (`[]`,
   header-only, or nothing), because an empty corpus is a valid answer.
10. As a user, I want `--fields` with a message-only name (e.g. `role`, `text`)
    to keep working in session mode if it is a valid session field, and an
    unknown name to fail loudly in both modes — no silent column drops.

## Implementation Decisions

- **`--messages` is a boolean flag on the existing `ExportArgs`**, not a new
  subcommand. Absent, `export` behaves exactly as today (Record-level). The
  existing `--format` and `--fields` flags are reused unchanged in meaning but
  resolve against a **message** field set when `--messages` is set.
- **Two field sets, one render engine.** `src/export.rs` gains a message
  `Field` set: `source`, `session_id`, `project`, `model`, `role`,
  `timestamp`, `text`. The existing Record `Field` set is untouched. Because
  the two sets share names (`source`, `session_id`, `project`, `model`), the
  type is **not** merged into one enum: a single enum with optional variants
  would let `export --messages --fields cost` validate against a cost column
  that a `Message` has no value for. Instead there are two enums,
  `RecordField` (the current `Field`, renamed) and `MessageField`, each with
  its own `ALL`, `name()`, and `parse()`. This keeps "an unknown name for this
  mode" honest: `--fields cost --messages` is an error naming `cost` as
  invalid **for messages**, and `--fields text` (no `--messages`) is an error
  naming `text` as invalid for sessions.
- **Rename `Field` → `RecordField`** in `src/export.rs`, updating the
  `ExportOptions`/`project`/`render` signatures accordingly. This is an
  internal rename; no CLI surface changes. A shared private `render_rows`
  helper (or a small generic) is acceptable only if it does not erase the
  two-enum distinction — a `Vec<(&'static str, Value)>` per row is the common
  currency, since `project` already produces exactly that.
- **`MessageField` canonical order** is `source, session_id, project, model,
  role, timestamp, text` — identity/location first, then the payload text
  last, mirroring how Record export puts `cost` last. `text` is emitted
  verbatim (newlines and all) in JSON; in CSV/TSV the `csv` crate quotes it,
  which is correct and requires no manual escaping.
- **Ordering** is `(source, session_id, timestamp)` ascending with `None`
  timestamps sorted **last within their session**, then original load order as
  the final tiebreak. This is a change in spine from Record export (which is
  `started_at` descending): messages are read as a transcript, so within a
  session they should come out in the order they happened. Across sessions,
  `source`/`session_id` give a stable, diffable grouping. Two invocations on
  identical data must be byte-identical.
- **Message time filtering fails closed**, per spec 0010's decision: a
  `Message` with `timestamp: None` fails any `--since`/`--last`. This is
  inherited from `Filter::matches_message` unchanged — no new predicate.
- **`--role` is a new flag**, `Option<String>`, exact case-insensitive match on
  `Message::role`, consistent with `search --role`. It is rejected (exit 1)
  when `--messages` is **not** given, because roles are a message-only concept.
- **Validation order.** `ExportArgs::validate` keeps the `--since`/`--last`
  mutual exclusion. A new check: `--role` without `--messages` errors naming
  `--role`. Field resolution happens after the mode is known, so the error for
  a wrong-mode field name is precise.
- **Source errors, zero matches, and exit codes** are unchanged from spec
  0014: a failed Source warns to stderr and never aborts; a zero-match set
  exits 0 with `[]` / header-only / empty stdout; no header or filters-echo is
  ever printed to stdout.
- **`render_messages<W: Write>(&[Message], &MessageExportOptions, &mut W)`** is
  the new pure seam in `src/export.rs`, unit-testable with hand-built
  `Message` values and no Source access, exactly as `render` is today.

## Testing Decisions

- **Unit tests** in `src/export.rs` cover the message path in isolation:
  JSONL one-object-per-line, canonical message field order, `--fields` subset
  and reorder, absent `model`/`timestamp` → JSON `null` / empty CSV cell (never
  `0`), CSV/TSV header and delimiter with a multi-line `text` value quoted,
  empty set → `[]` / header-only, ordering (in-session chronological,
  `None` timestamp last, cross-session stable), and `MessageField::parse`
  rejecting a Record-only name with a message-specific error.
- **Record-path regression:** the existing export unit tests must still pass;
  the `Field`→`RecordField` rename is mechanical and behavior-preserving.
- **Integration tests** extend `tests/export.rs`, driven through the built
  binary against `tests/fixtures`, asserting: `--messages` emits one row per
  message across all four Sources; `--role assistant` narrows to assistant
  rows; `--source`/`--project`/`--since`/`--last` narrow the message set;
  a message with no timestamp (or out-of-window) is excluded by `--last`;
  `--fields role,text` narrows and reorders; `--format json` is a parseable
  array; `--format csv` has a message header; `--messages --fields cost`
  exits 1 naming `cost`; `--role` without `--messages` exits 1; a zero-match
  filter exits 0 with the right empty shape.
- **No fixture changes are required** — spec 0010 already extended the
  fixtures so every Source carries message text, and `search` integration
  tests already prove those messages load. This feature reads the same
  corpus through the same call, so the fixtures are exercised as-is.

## Out of Scope

- **Browsing or a TUI.** `export` stays a one-shot, non-interactive emitter;
  message browsing is `search`'s TUI.
- **Reconstruction of full session transcripts as a single document** (a
  "conversation" export). This is a flat row-per-message dump; assembling
  turns is the consumer's job.
- **Tool-output / tool-result rows.** Per spec 0010 these are excluded from
  the searchable corpus at the adapter, so they are absent here too. Not
  revisited.
- **New Sources, new formats, new filters** beyond `--role`; no Parquet, no
  SQLite output, no file destination.
- **Changing Record-level export** in any way. `export` without `--messages`
  must be byte-for-byte what it is today.

## Further Notes

- This closes the loop spec 0010 opened: `search` finds messages, `export
  --messages` hands them to a program. The `report`/`export` symmetry for
  Records now has a Message analogue, and `search`/`export --messages` are the
  human/program pair for transcripts.
- The pipeline change is a new flag plus a second pure emitter — no change to
  `Source`, `Registry`, `Message`, or `Filter`. `Registry::load_messages_all`
  and `Filter::matches_message` already exist and are already tested by
  `search`, so message export is a new consumer of proven seams rather than
  new data plumbing.
- The `Field` → `RecordField` rename is the one risky edit: it touches the
  existing export tests and `main.rs`. It is mechanical and must be done as
  its own reviewed step before the message feature lands, so the Record path
  is proven unchanged.
