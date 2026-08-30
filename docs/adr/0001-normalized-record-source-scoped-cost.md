# Normalized usage record; cost is source-scoped

Claude Code and OpenCode disagree on shape: Claude emits per-message token blocks in JSONL with no cost field; OpenCode exposes pre-aggregated per-session token columns *and* a cost column in SQLite. We model both behind a single normalized `Record` (session id, source, project, model, agent, timestamps, five-part token breakdown, count, optional cost). Claude's cost is always `None`; OpenCode's is `Some`.

Cost is **never** summed or averaged across Sources — it is displayed only per Source. This is deliberate: across sources there is no common currency or price basis, so cross-source cost math would be misleading. A future reader may expect a grand "total cost" cell; resist it. If a single-source grand total is ever wanted, it must be computed within one Source only.
