---
id: 0009
title: "request --stream — SSE streaming"
status: done
created: 2026-09-08
triage: done
---

## Problem Statement

`llmhelper request` only supports a single-shot Chat Completions call: the process blocks until the provider returns the whole response, so there is no token-by-token feedback for long generations and no way to feed incremental output into a shell pipeline. The command's own spec (0008) promised a streaming TUI viewer with a streaming buffer; only the non-streaming half was built.

## Solution

Add `--stream` to `llmhelper request`. With the flag the payload sets `stream: true`, the response body is consumed as an SSE stream, and every `data:` event's `choices[0].delta.content` is appended as it arrives. The three existing output modes all get a streaming form:

- **TUI (default):** the response renders progressively. The header shows a `stream: live` marker while events arrive, flips to `stream: done` at `[DONE]`, and the elapsed time keeps ticking during the stream.
- **`--text --stream`:** each delta is printed as it arrives, so the output is pipable and incremental.
- **`--json --stream`:** every parsed SSE event is printed as one JSON line (NDJSON) — the raw provider objects, pipeable into other tools.

Without `--stream` nothing changes: the current one-shot path, its payload shape, and its output are untouched.

## User Stories

1. As a developer, I want `--stream` to send `stream: true` so that I can watch a long generation appear token by token instead of waiting for the full response.
2. As a developer, I want `--text --stream` to print each delta the moment it arrives so that I can pipe a live response into another process.
3. As a developer, I want `--json --stream` to emit one JSON line per SSE event so that scripts can consume every chunk, not only the final answer.
4. As a user, I want the TUI to render streamed text progressively so that I get immediate feedback on long generations.
5. As a user, I want the TUI header to show whether the stream is still live or has finished so that I know when the answer is complete.
6. As a user, I want the TUI header's elapsed time to tick during the stream so that I can see how long a generation takes so far.
7. As a user, I want the TUI to keep the newest text on screen while the stream is live so that I never have to chase the tail by hand.
8. As a user, I want scrolling up during a live stream to stop the auto-follow so that I can read earlier output without it jumping away.
9. As a user, I want jumping to the bottom during a live stream to re-enable auto-follow so that I can rejoin the tail deliberately.
10. As a user, I want token usage to appear in the TUI header when the provider reports it in a stream chunk.
11. As a developer, I want the request to carry `stream_options.include_usage` so that providers following the OpenAI schema report tokens for a stream.
12. As a developer, I want the total request timeout to not cap a streaming request so that long generations do not die at 30 seconds.
13. As a developer, I want the connect phase of a streaming request to still be bounded by the configured timeout so that an unreachable host fails fast.
14. As a developer, I want multi-line `data:` fields joined into a single event so that providers that wrap long payloads across lines still parse.
15. As a developer, I want comment lines, `event:`, `id:`, and `retry:` fields ignored so that keep-alive pings and SSE bookkeeping do not surface as content.
16. As a developer, I want the `[DONE]` sentinel to terminate the stream cleanly so that a normal end is not treated as a mid-stream error.
17. As a developer, I want a chunk boundary to fall anywhere inside a line so that the parser does not require a provider to flush on line boundaries.
18. As a developer, I want a non-2xx status to fail a streaming request with the body snippet, exactly as the one-shot path does.
19. As a developer, I want an event that is not valid JSON to be skipped rather than aborting the stream so that a stray line does not destroy the whole response.
20. As a developer, I want a connection that drops mid-stream to fail with a clear error so that scripts can tell a truncated answer from a complete one.
21. As a developer, I want `--text --stream` to fail when the stream produced no content so that an empty answer is distinguishable from success.
22. As a user, I want the streamed content to render with the same body styling and navigation keys as the one-shot TUI so that the command feels like one tool.
23. As a developer, I want `--stream` combined with `--json --text` to be rejected, because a JSON line stream and a text stream are not the same output.
24. As a developer, I want the one-shot payload to remain free of `stream` and `stream_options` keys so that existing integrations and tests keep passing unchanged.

## Implementation Decisions

- New `--stream` boolean flag on the request command's args. It is additive and default-off; `--json`/`--text` mutual exclusion stays as-is, and `--json`/`--text` each remain independent of `--stream`.
- The payload builder gains a stream parameter: when streaming, the body gains `"stream": true` and `"stream_options": {"include_usage": true}` alongside `model`, `messages`, and the sampling parameters. When not streaming, no such keys are emitted.
- New pure SSE layer next to the existing request logic, no new crate:
  - A parser fed incrementally with byte chunks, keeping a residual buffer so a chunk may end mid-line. It yields the accumulated `data:` payload of each event, treating a blank line as the event delimiter, joining multi-line `data:` fields with a newline, dropping comment lines and every non-`data` field, and stopping at the `[DONE]` sentinel.
  - Extraction of `choices[0].delta.content` from an event, returning nothing for a missing choice, a missing delta, or a non-string content.
  - Extraction of `usage` from an event, so the last event carrying one wins.
- One streaming send function takes the resolved settings, the payload, and a per-event callback, and returns the final status, accumulated usage, and accumulated content. The callback is invoked for every parsed event; callers decide what to do — the CLI paths print, the TUI path pushes to a channel.
- Streaming sends use a client with no total timeout and the configured timeout as the connect timeout, because a generation legitimately runs longer than a request timeout. The one-shot path keeps its total timeout.
- Streaming transport uses reqwest's byte-stream API, which requires enabling the `stream` feature on the existing reqwest dependency. No new crate is added.
- The one-shot send path, `parse_response`, and `extract_assistant_content` are left untouched so that existing behavior and tests are unaffected.
- Streaming HTTP errors reuse the one-shot error shape: non-2xx reads the body and bails with `HTTP <code>: <snippet>`, truncated the same way. A dropped connection mid-stream bails with a distinct message naming the interruption.
- TUI streaming state: the viewer gains a stream status that is off for one-shot requests, live while events arrive, and done at the end. The header renders `stream: live` or `stream: done` only when the status is not off. The elapsed time is recomputed on every frame redraw while the draw loop is running, so it continues to advance briefly after the last event is consumed and before the process exits.
- TUI tail-following: while live, each append scrolls to the newest text. Scrolling up, paging up, or jumping to the top disables following; jumping to the bottom re-enables it. Once the stream is done, following stops regardless.
- TUI streaming loop: the stream is driven by a background task that pushes events over an unbounded-capacity channel, and the draw loop drains the channel each frame, applies each event to the state, and redraws. Input handling and quit semantics are unchanged from the one-shot viewer.
- Appending text merges into the current last body line instead of opening a new one per event, so a chunk that contains a newline grows the document by exactly the number of lines it introduces.
- Empty streamed body still renders the existing `(empty response)` placeholder for the TUI, and `--text --stream` reports an error for an empty result rather than printing nothing.

## Testing Decisions

A good test here exercises the external contract of each layer: the payload builder returns the right body, the SSE parser yields the right events for adversarial framing, the extraction functions handle missing or wrong-typed fields, the TUI state moves correctly, and the binary speaks the right bytes over a real socket. Nothing tests internals such as buffer offsets or channel contents.

- Pure SSE parsing, extraction, and payload tests live with the existing request unit tests. Adversarial framing to cover: a chunk ending mid-line, a line split across several chunks, multi-line `data:` joined, comment and `event:`/`id:`/`retry:` lines ignored, `[DONE]` terminating, trailing blank lines, CRLF line endings, empty `data:` events skipped, and an event with `delta.content` that is `null` rather than a string.
- TUI state tests live with the existing request viewer state tests, mirroring how the report viewer's state is unit tested: append-merges-into-last-line, tail-follow-while-live, scroll-up-stops-following, bottom-resumes-following, follow-stops-when-done, and the stream status markers.
- End-to-end streaming tests live in the existing request integration test file, using the same local HTTP server harness rather than adding a mocking crate. The harness needs a streaming variant that writes SSE frames with a short pause between them, which proves the client consumes incrementally rather than waiting for a complete body. Tests to add: `--text --stream` prints the concatenated content, `--json --stream` emits one NDJSON line per event, the streamed payload carries `stream: true` and `stream_options.include_usage`, a streamed non-2xx fails with the body snippet, and a non-streaming payload still carries neither key.
- No TUI automation; PTY smoke is used, as for the report TUI.

## Out of Scope

- Tool/function calling, image inputs, and embedding or fine-tuning endpoints — already out of scope for the command.
- SSE features beyond `data:` payload collection: no `event:`/`id:` replay, no reconnect with `Last-Event-ID`, no `retry:` interval honoring.
- Reasoning or thought-channel deltas; only `choices[0].delta.content` is rendered.
- Streaming the local usage data sources; this only streams a provider response.
- Aborting a live stream mid-flight from the TUI with a cancellation token. Quitting the viewer exits the process, which is enough for now.
- Resampling, temperature drift detection, or any post-hoc analysis of the chunks.

## Further Notes

- `stream_options.include_usage` is included whenever `--stream` is set, because it is the documented way to get token counts out of a stream. A provider that rejects unknown request fields will surface it as an HTTP error with the provider's body snippet; the command cannot detect that proactively.
- The TUI renders deltas only. Providers that stream tool calls or logprobs through the same SSE channel are silently ignored.
- `--json --stream` emits NDJSON, not a single JSON object; a consumer that expects the one-shot response shape must drop `--stream`.
- The one-shot path keeps its 30-second default total timeout, so only streaming callers are exempt from it.
