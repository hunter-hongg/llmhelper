---
id: 0013
title: "request — reasoning (thinking) capture and streaming"
status: done
created: 2026-09-11
triage: done
---

## Problem Statement

`llmhelper request` throws away everything the provider returns except
`choices[0].message.content`. A reasoning model that answers behind a
chain-of-thought, or a self-hosted endpoint that exposes its thinking, produces
that thinking in a sibling field which the command never reads: the one-shot
path extracts only `message.content`, and the streaming path extracts only
`choices[0].delta.content`. There is no flag, no output mode, and no TUI region
where reasoning can appear.

This is a gap with two halves. It is a **parsing** gap — the provider's
reasoning lives in a field the command does not know about, so `--json` is the
only way to see it, and even that is undiscoverable. It is also an
**observability** gap — reasoning streams out *before* the answer, so a live
thinking channel is the single most useful signal that a long generation is
alive and on track, and today the TUI shows nothing until the first content
delta arrives.

The project already treats reasoning as a first-class concept on the read side:
`search` stores reasoning blocks as messages with `role: "thinking"` and can
filter on them with `--role thinking`. The request side of the same project
should not be blind to the same concept.

## Solution

Add first-class reasoning support to `llmhelper request`, modelled on
`--tools`: a **file-driven, pass-through** capture that is off by default and
that never rewrites what the provider hands back.

Three additive capabilities:

- **`--reasoning-field <field>` (capture).** The flag names where in each
  response object the reasoning text lives. The one-shot path reads that field
  from the response and exposes it; the streaming path reads the same field
  from each SSE event's `delta` and accumulates it. The flag may be repeated:
  Azure-style endpoints use `reasoning_content`, OpenRouter uses `reasoning`,
  and a user should be able to name both without a synonym table in this code.
- **`--reasoning <file>` (request).** A JSON file whose contents are embedded
  verbatim into the request payload as a `reasoning` object, for endpoints that
  accept a reasoning configuration (effort/budget/exclude). Pass-through, like
  `--tools` — this command validates the shape and does not interpret it.
- **`--thinking` (view).** In the TUI, split the reasoning channel from the
  answer into its own labeled `thinking` panel with its own scroll offset, and
  let the user choose what the answer pane shows. `--thinking` composes with
  `--interactive`, which is where a multi-turn reasoning conversation is
  actually useful.

The default experience is unchanged. Without `--reasoning-field`, no field is
read, nothing new is rendered, and every existing payload, output, and exit
code is byte-for-byte what it is today.

The read side benefits for free: a captured one-shot response may be piped to
`llmhelper search`'s corpus or simply read as text, and the vocabulary matches
— reasoning is *thinking* on both sides of the tool.

## User Stories

### Capturing reasoning from a response

1. As a developer using a reasoning model, I want to name the response field the reasoning arrives in, so that I can read the model's thinking without hand-parsing NDJSON.
2. As a developer, I want `--reasoning-field` to be repeatable, so that I can point one command at an endpoint that uses `reasoning_content` and another at one that uses `reasoning` without memorising which is which.
3. As a developer, I want the reasoning text appended to `--text` output, so that a shell pipeline receives the full model output and not just the final answer.
4. As a developer, I want `--text --thinking` to print only the reasoning channel, so that I can inspect a chain of thought on its own.
5. As a developer, I want `--json` to gain a top-level `reasoning` field when a capture flag was used and reasoning was found, so that scripts can read the thinking without re-deriving the field path.
6. As a developer, I want an explicit empty `reasoning` marker in the `--json` envelope when a capture flag was used but no reasoning was present, so that "asked for reasoning, got none" is distinguishable from "did not ask".
7. As a developer, I want `--json` to remain the untouched provider response when no `--reasoning-field` is given, so that existing consumers keep the exact object they expect.
8. As a developer, I want the *first* non-empty value to win when a field path matches more than one place in the response, so that reading is deterministic rather than dependent on key order.
9. As a developer, I want a missing or mistyped reasoning field to be silently empty rather than an error, so that switching providers does not break a script that merely asked for reasoning.

### Streaming reasoning

10. As a user, I want the TUI to show reasoning as it streams in — before the answer arrives — so that a long-thinking generation gives me feedback instead of a blank pane.
11. As a user, I want the reasoning channel to land in its own panel rather than being concatenated with the answer, so that I can always tell where thinking stops and the answer begins.
12. As a user, I want the TUI header to report whether the current generation is thinking, answering, or done, so that the state of a stream is legible at a glance.
13. As a developer, I want `--text --stream` to emit reasoning deltas to stderr and answer deltas to stdout, so that `llmhelper request --text --stream > answer.txt` yields a clean answer while the thinking is still visible.
14. As a developer, I want `--json --stream` to annotate each event's channel rather than restructure it, so that an existing NDJSON consumer can ignore the new key and keep working.
15. As a developer, I want `--text --thinking --stream` to print only thinking deltas, so that I can watch a chain of thought without the answer interleaved.

### Configuring reasoning on the request

16. As a developer, I want `--reasoning <file>` to embed a JSON object verbatim into the payload, so that I can pass an endpoint's reasoning configuration without this command inventing flags for every vendor's vocabulary.
17. As a developer, I want `--reasoning <file>` to reject a file that is not a JSON object, so that a malformed config fails fast and locally rather than as an opaque HTTP 400.
18. As a developer, I want `--reasoning <file>` to compose with `--stream`, `--json`, `--text`, `--interactive`, and `--tools`, so that it is one more knob rather than a mode.
19. As a developer, I want the `reasoning` key entirely absent from the payload when the flag is not given, so that providers that reject unknown fields are never sent one.
20. As a developer, I want a `[request] reasoning` config key so that I can make a reasoning model my default without repeating a flag on every invocation.

### Choosing what the view shows

21. As a user, I want the default view to show the answer followed by a collapsed reasoning panel, so that the answer stays the thing I see first and reasoning is opt-out rather than in my face.
22. As a user, I want a `--thinking` flag that expands the reasoning panel, so that I can decide per invocation whether the thinking is worth the screen space.
23. As a user, I want to cycle the view with `t` between answer-only, thinking-only, and both, so that I can switch without leaving the TUI or re-running the request.
24. As a user, I want the answer pane and the reasoning pane to scroll independently, so that reading an earlier passage of the answer does not drag the reasoning away with it.
25. As a user, I want tail-follow to apply to both panes while a stream is live, so that the newest reasoning and the newest answer are both on screen without manual scrolling.
26. As a user, I want a settled (non-live) view to start scrolled to the top of both panes, so that I begin reading at the beginning rather than at the end.
27. As a user, I want a click-free, key-driven footer that names the new keys, so that the thinking affordance is discoverable in the same place as the existing ones.

### Interactive multi-turn with reasoning

28. As a user, I want `--interactive` to keep the reasoning of the current turn while I send follow-ups, so that each turn shows its own thinking rather than accumulating every turn's.
29. As a user, I want the reasoning channel to reset at the start of each turn, so that a stale chain of thought is never mistaken for the current one.
30. As a user, I want to be able to copy the reasoning text the same way I copy the answer, so that I can lift a chain of thought out of the terminal.

### Housekeeping

31. As a maintainer, I want the reasoning capture to live entirely in the existing request module and the existing request TUI state, so that no new module or Source is introduced for a provider-response concern.
32. As a maintainer, I want the field-path lookup to be a pure function with its own tests, so that the vendor-vocabulary plumbing is verified without a socket.
33. As a maintainer, I want every existing request test to pass unchanged, so that "off by default" is proved rather than asserted.
34. As a developer, I want `--copy` and a documented clipboard story for the reasoning channel, so that the OSC 52 mechanism already shipped for the answer is not answer-only.
35. As a maintainer, I want the README `request` section to document the four new flags and the stderr/answer split, so that the documented surface matches clap exactly.
36. As a maintainer, I want spec 0008 to stop listing reasoning as out of scope, so that the spec set does not contradict shipped behaviour a second time.

## Implementation Decisions

**Field capture is a pass-through path, not a parser.** A single pure
`extract_text_by_path(value: &Value, path: &str) -> Option<String>` walks
dot-separated segments (`delta.reasoning_content`, `message.reasoning`),
applying the same rule as `extract_delta_content` — a non-empty JSON string is
text, everything else is absent. The existing field paths become thin wrappers
so capture and answer extraction can never diverge. Every supplied field is
consulted and the non-empty matches are joined in flag order; no synonym table
is compiled into the binary, because vendor field names are data, not
behaviour.

**`RequestResponse` gains `reasoning: Option<String>`.** Multi-field capture
concatenates non-empty matches with a blank line between them; no single field
matching leaves it `None`. `parse_response` stays pure.

**`StreamResponse` gains `reasoning: String`,** accumulated in the same loop as
`content` via the same path walker. `extract_delta_content` is unchanged and
still used for the answer.

**`SamplingParams` gains `reasoning: Option<Value>`,** an optional top-level
`reasoning` key inserted verbatim by `build_payload` and omitted when absent,
exactly as `tools` is today. A non-object `--reasoning` file is a usage error
(exit 1) before any socket is opened.

**`RequestSettings` gains `reasoning_fields: Vec<String>`,** resolved
flag-first with a `[request] reasoning_fields` config fallback, alongside the
existing base-url/api-key/model/timeout resolution. A new `Config` accessor
exposes the resolved config path so a relative `--reasoning <file>` resolves
against the config file's directory when a config file was loaded, and against
the working directory otherwise — the same resolution rule is applied to
`--tools`, which today silently fails for a relative path under `--config`.

**Output contract.** With no `--reasoning-field`, every output is unchanged.
With capture on:

- The `--json` envelope becomes `{ response, reasoning, reasoning_fields }`,
  where `reasoning` is `null` when nothing matched and `reasoning_fields` is
  the list actually consulted. This envelope is emitted *only* when capture is
  on, so the untouched-provider-response contract is preserved for everyone
  else.
- `--text` appends the reasoning to the answer, separated by a labelled blank
  line.
- `--text --thinking` prints the reasoning alone.
- `--json --stream` keeps one raw SSE event per line and adds a sibling
  `"@channel"` key in a fixed position, so the output stays one line per event
  and a consumer reading the raw keys is unaffected.
- `--text --stream` routes answer deltas to stdout and reasoning deltas to
  stderr; `--text --thinking --stream` prints reasoning to stdout only.

**TUI.** `RequestTuiState` gains a reasoning channel and an independent scroll
offset, but keeps a single `Scrollable` impl so no existing key routing
changes. A `ThinkingView` enum (`Answer`, `Both`, `ThinkingOnly`) is driven by
the `--thinking` flag and cycled by `t`. The header gains a generation state
(`idle` / `thinking` / `answering` / `done`), and its token field is fed from a
full usage object so prompt/completion counts are available, with the
`total_tokens` reading kept for the narrow header. `--copy` copies the answer
by default and the reasoning under the view that is showing reasoning. Rendering
switches to a three-pane layout (answer above, reasoning below) only when
reasoning exists or the view is not answer-only; the existing two-pane layout is
untouched for the no-reasoning case. Each interactive turn resets the reasoning
channel.

**Refactor included, not deferred.** `run_request_interactive` and
`run_request_stream_tui` duplicate the entire scroll/quit key dispatch, and
`handle_request_event` duplicates the append-buffer logic that
`run_request_stream_tui` inlines. Cargo will not grow for this feature, so the
duplication is paid down here rather than tripled: one key handler and one
append helper, shared by both loops. This is the prefactor the tickets sequence
first.

## Testing Decisions

A good test here pins an external contract: the bytes the binary sends, the
bytes it prints, the exit code it returns, and the state a viewer reaches after
a sequence of events and keys. Nothing tests internals such as buffer offsets
or private helpers, and no test asserts that a capture path *was* taken — only
what a caller can observe.

The seams, all pre-existing:

1. **The pure request module** (`src/request.rs` unit tests) — the same seam
   the payload builder, SSE parser, and extraction functions are already
   tested at. New tests: the dot-path walker (correct segment, missing segment,
   wrong type, empty string, nested path, non-object intermediate), payload
   embedding of a `reasoning` object, omission when absent, `parse_response`
   with a reasoning field, and `SamplingParams` round-tripping the object.
2. **The request TUI state** (`src/tui/request_app.rs` unit tests) — the same
   seam the stream/scroll/follow state machine is already tested at. New tests:
   view cycling order, independent pane offsets, reasoning reset per turn, and
   the generation-state transitions.
3. **The binary over a real socket** (`tests/request.rs`) — the existing local
   HTTP server harness, extended with an SSE body that interleaves reasoning
   and content frames. New tests: one-shot capture reaching `--text` and
   `--json`, `--text --thinking`, reasoning on stderr versus stdout under
   `--stream`, `"@channel"` annotation under `--json --stream`, the
   `--reasoning` file appearing verbatim on the wire and its rejection when
   not an object, and the regression that a run without capture flags produces
   the exact payload and output it does today.

Prior art is the shipped suite: `build_payload_*` and `sse_parser_*` for seam 1,
the request app's scroll/follow tests for seam 2, and
`request_stream_json_emits_one_line_per_event` for seam 3 — including that
test's decision to assert per field rather than against a serialised whole
line, because `serde_json` map ordering is not stable.

TUI layout is not automated; the repo's convention is a PTY smoke script per
TUI (see `.scratch/*_smoke.py`), and one is added for the thinking split.

## Out of Scope

- **Anthropic's native `/v1/messages` API.** Reasoning capture is defined
  against OpenAI-shaped response objects only; the `thinking` content blocks of
  the Messages API are not parsed, and no request-shape translation is added.
- **An on/off reasoning flag and a budget/effort flag.** Both are configuration,
  and configuration is passed through `--reasoning <file>` verbatim rather than
  re-modelled as flags this command would have to keep in step with vendors.
- **A response-header TUI pane** (rate-limit and request-id inspection).
- **Reasoning-aware cost accounting** — pricing reasoning tokens differently
  from completion tokens, or reporting a reasoning-token split.
- **Persisting conversations or reasoning** to disk, and re-loading them.
- **Regex or fuzzy reasoning detection** — the field name is exact, and a
  non-matching name is simply empty.
- **Extracting reasoning from `search`-visible local corpora**; this spec is
  about the provider response path only.

## Further Notes

- The duplication paid down in Implementation Decisions is the reason this
  spec ships a prefactor ticket first; without it, adding a second buffer would
  mean maintaining four copies of the same append-and-follow logic.
- `--reasoning-field` accepting either a bare field name (`reasoning_content`)
  or a dotted path (`delta.reasoning_content`) would be more forgiving, but
  ambiguity is worse than punctuation: the flag takes dotted paths only, and
  the OpenAI delta case is expressed as a `delta.`-prefixed path for
  documentation rather than special-cased in code.
- The `--json` envelope is a deliberate asymmetry with the "never change the
  shape" instinct. It is scoped to runs that explicitly asked for reasoning,
  and the alternative — injecting keys into the provider's own object — would
  corrupt a response body the provider owns.
- This is the second time the `request` spec set has trailed the code (0008 did
  the same for `--stream`); the README and spec updates are therefore tickets in
  their own right rather than an afterthought.
