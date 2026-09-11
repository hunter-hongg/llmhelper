---
id: 0011
title: "request — documentation and spec sync"
status: ready-for-agent
created: 2026-09-11
triage: ready-for-agent
---

## Problem Statement

`llmhelper request` shipped with SSE streaming support (`--stream`) two days after the command itself, and the documentation was never brought across with it. A developer reading the README sees no sign that streaming exists: the flags list omits `--stream`, there is no example, and the behaviour of `stream_options.include_usage` is undocumented. The command's own spec (0008) now describes the command as if streaming were still unbuilt — its Solution promises a streaming TUI viewer "with streaming support" as future work, its Out of Scope lists "streaming SSE parsing beyond SSE lines", and its exit-code contract claims a distinct code 2 for I/O errors, which the implementation does not honour. A third spec file was meant to be the companion for the streaming work and was never marked as published.

The `search` subcommand has the mirror-image problem: it exists, has a full spec, is fully documented in the changelog, and has no README section at all. The README's "Specs" pointer still lists only usage, diff, sessions and report.

A developer onboarding onto the repository therefore reads instructions that are either missing or contradicted by the code, and cannot tell from the docs whether a feature is planned, missing, or already there.

## Solution

Bring the documentation to match the shipped behaviour, without changing a line of Rust.

Three surfaces are updated.

The README `request` section gains a `--stream` example for each of the three output forms — TUI, `--text`, and `--json` — states that the payload carries `stream_options.include_usage` so providers report token counts, and adds `--stream` to the flags list. A new `search` section is added describing the full-text subcommand with its flag surface and its default-TUI behaviour, and the "Specs" pointer at the foot of the README is corrected to name every published spec.

Spec 0008 is corrected to describe the command as it now stands: streaming is a first-class feature documented as a spec-0009 concern rather than listed as out of scope; the exit-code contract is restated to match what the binary actually does; and features the spec promised but the code never built are moved into Out of Scope so the spec stops over-promising.

The request-stream ticket file that recorded the documentation follow-up is checked off against what is now true, and the streaming integration test suite is extended to cover the two streaming output modes that currently have no end-to-end assertion.

## User Stories

1. As a developer, I want the README to show a `--stream` example so that I know long generations can be watched token by token without opening the source.
2. As a developer, I want a `--stream` example using `--text` so that I can pipe a live response into another process and see the exact command to do it.
3. As a developer, I want a `--stream` example using `--json` so that I can build a script over per-event NDJSON and know the output shape from the docs.
4. As a developer, I want `--stream` in the README flags list so that I do not have to read clap `--help` to discover it exists.
5. As a developer, I want the README to explain that `stream_options.include_usage` is sent automatically with `--stream` so that I understand where the token counts in the TUI header come from.
6. As a developer, I want to know that a provider rejecting unknown request fields surfaces as an HTTP error so that I can anticipate that failure mode when switching providers.
7. As a developer, I want a README section for `search` so that I can find the full-text subcommand alongside `usage`, `diff`, `sessions`, `report` and `request`.
8. As a developer, I want the `search` README section to name its filter flags — source, project, model, role, time window — so that I can compose a narrow query without reading the source.
9. As a developer, I want the `search` README section to state that the interactive viewer is the default output so that I know `--json`, `--csv` and `--text` are opt-outs.
10. As a developer, I want the `search` README section to mention `--case-sensitive` and `--context` so that I know the matching semantics are tunable.
11. As a developer, I want the README "Specs" pointer to list every published spec so that I can navigate to the `search`, `request` and streaming specs directly.
12. As a maintainer, I want spec 0008's Solution to stop describing streaming as unbuilt so that a reader does not conclude the TUI viewer is still missing.
13. As a maintainer, I want spec 0008's Out of Scope to stop excluding "streaming SSE parsing beyond SSE lines" so that the spec no longer contradicts spec 0009, which is precisely about that.
14. As a maintainer, I want spec 0008's exit codes restated to match the binary, which reports client errors and request failures with a single non-zero code and never uses the distinct I/O code it documented.
15. As a maintainer, I want spec 0008's promised request logging moved into Out of Scope so that an unimplemented feature is no longer described as a delivery decision.
16. As a maintainer, I want spec 0008's TUI story corrected to read-only-with-no-interaction, which is what the viewer actually is.
17. As a maintainer, I want spec 0008 to point at spec 0009 for streaming rather than duplicating it, so that the two documents stay in agreement.
18. As a maintainer, I want the request-stream ticket marked complete so that the ticket board stops showing ready work that is already delivered.
19. As a maintainer, I want an end-to-end test that `--json --stream` emits one NDJSON line per SSE event so that the documented output shape is enforced by the suite.
20. As a maintainer, I want an end-to-end test asserting a streamed payload carries `stream: true` and `stream_options.include_usage` so that the payload contract is pinned at the wire boundary.
21. As a maintainer, I want an end-to-end test asserting a non-streaming payload carries neither key so that streaming cannot silently leak into one-shot calls.
22. As a maintainer, I want the new tests to run through the existing local HTTP server harness so that no mocking crate is introduced.
23. As a maintainer, I want the existing request tests to remain green so that documentation work provably changes no behaviour.
24. As a reviewer, I want every flag and example in the README to match clap's parsed names exactly so that copy-pasted commands cannot silently fail.
25. As a reviewer, I want the streaming TUI's documented behaviour — live/done marker and token counts — to be traceable from the README to spec 0009.

## Implementation Decisions

- No Rust source file is modified. The change set is README, spec documents, and the ticket files that track this work, plus the request integration test file where the streaming wire-boundary assertions are missing.
- The README `request` section keeps its existing structure — one example block per output mode, then a single flags sentence — and adds `--stream` to it rather than restructuring the section, so the addition reads as a feature of the same command.
- The three streaming examples distinguish output shape explicitly: TUI as the default, `--text --stream` as incremental pipable text, `--json --stream` as one JSON object per line. The README states NDJSON for the JSON stream so a reader expecting the one-shot response shape knows to drop `--stream`.
- `stream_options.include_usage` is documented as sent automatically whenever `--stream` is set, never as a separate flag, matching the payload builder's behaviour.
- The `search` README section mirrors the `usage`/`diff` convention: two example commands, then one flags sentence naming `--source`, `--project`, `--model`, `--role`, `--since`/`--last`, `--context`, `--limit`, `--case-sensitive`, and the `--json`/`--csv`/`--text` opt-outs.
- Spec 0008's exit-code decision is restated to the single non-zero code the binary emits for both request failures and HTTP client errors. The previously documented distinction is recorded as not implemented rather than deleted, so the gap is visible to whoever later wants it.
- Spec 0008's request-logging story moves to Out of Scope. The codebase writes no logs for the request path, so describing it as an Implementation Decision was an over-promise.
- Spec 0008's remaining Out of Scope entries are kept as-is; embeddings, fine-tuning, tool calling, OAuth, proxy configuration, conversation persistence, retry with backoff, and TUI message editing are all still genuinely unimplemented.
- Spec 0008 gains an explicit pointer to the streaming companion spec so the two documents cannot drift again.
- The streaming ticket files' checkboxes are checked against what is actually shipped, and the remaining unchecked item is resolved by publishing the documentation work that constitutes this spec.
- The streaming test additions extend the existing local HTTP server harness rather than adopting a mocking crate, consistent with the one-shot request tests already in the suite.
- Ticket acceptance is judged by diffing the documented flag names against clap's `--help` output, not by re-reading prose.

## Testing Decisions

Good tests here assert the contract a caller observes: the wire bytes a streamed request sends, the lines a streamed response emits, and the absence of streaming keys on a one-shot request. Nothing tests README prose or spec wording — those are verified by reading the clap output.

- The request integration test file is the single seam. It already drives the real binary against a local HTTP server that records the request it received, so the new assertions extend that harness rather than adding one.
- End-to-end cases to add: `--json --stream` emits one JSON line per parsed SSE event; a streamed request body carries `stream: true` and `stream_options.include_usage`; a request without `--stream` carries neither key. The last assertion is a regression guard — the payload builder's streaming branch must stay gated.
- The existing streaming text test, which already covers concatenated delta output through `--text --stream`, remains the template for the new cases: the harness serves SSE frames, the binary is invoked, and stdout plus the recorded request are asserted.
- The full suite must stay green unchanged in count for everything except the added cases, proving the documentation work touched no behaviour.
- No TUI automation. Streaming TUI behaviour is already covered by state unit tests and PTY smoke, as for the report viewer.

## Out of Scope

- Any Rust behaviour change. This work is documentation and test coverage for shipped behaviour, not new features.
- Implementing the features spec 0008 promised but never built: request logging, `--refresh-interval` for the request command, embedding or fine-tuning endpoints, tool/function calling, OAuth, proxy configuration, conversation persistence, retry with backoff, and TUI message editing. Recording these as Out of Scope is in scope; building them is not.
- Changing the request command's exit-code scheme. The documentation is brought to the implementation, not the reverse.
- A CLI change to add the `--stream` flag to more output modes than the three already supported.
- Rewriting the README's structure, tone, or layout beyond adding the two sections and the pointer correction.

## Further Notes

- The spec 0009 frontmatter reads as an open, ready-for-agent document even though the streaming feature is shipped and tested. Its status field is left alone because nothing in the repository's history shows a convention for flipping spec status to done, and inventing one here would be scope creep.
- The README's `request` flags sentence does not mention `timeout_seconds`; it is included in this work only if it fits the sentence naturally, since the config section is already referenced there.
- Documentation for `search` is new ground in the README, so its wording follows the `report` section's style of stating the default output mode explicitly and then naming the opt-outs, which is the convention that most clearly reads for a command with a default TUI.
