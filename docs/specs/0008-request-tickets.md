## Tickets for request command

### Ticket 1: Config and CLI args
**Title:** Add request config section and CLI args
**Blocks:** 2,3,4
**Description:** Extend Config struct to include `[request]` section (base_url, api_key, default_model, timeout_seconds). Add `Command::Request` to cli.rs with RequestArgs: --base-url, --api-key, --model, --messages, --prompt, --json, --text, --temperature, --top-p, --max-tokens, --stop.

### Ticket 2: Request client implementation
**Title:** Implement OpenAI compatible request client
**Blocks:** 4,5
**Description:** Create src/request.rs with RequestClient using reqwest, build_payload pure function, RequestResponse struct, error mapping. Unit tests for payload builder.

### Ticket 3: CLI output modes
**Title:** Implement CLI JSON and text output
**Blocks:** 6
**Description:** Wire request command in main.rs to call client, handle --json/--text output. Exit codes and error messages.

### Ticket 4: TUI viewer for request
**Title:** Add TUI viewer for streaming request response
**Blocks:** 6
**Description:** Create src/tui/request_app.rs and request_render.rs following report patterns. Display response, headers, timing, token usage. Scroll navigation.

### Ticket 5: Integration tests
**Title:** Add integration tests with mock server
**Blocks:** 6
**Description:** Add tests using mockito to verify request roundtrip, error handling, streaming.

### Ticket 6: Docs and README
**Title:** Update README and docs for request command
**Blocks:** none
**Description:** Add request usage examples to README.md and docs/specs summary.
