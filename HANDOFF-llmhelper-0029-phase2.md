# Handoff — llmhelper: 0029 Project convention fix + final state

## Status

**Everything is committed and pushed.** Working tree is clean. All gates pass:
`cargo fmt --check` clean, `cargo clippy --all-targets -- -D warnings` clean,
`cargo test` = **810 passed, 0 failed**.

## What this session completed

### 1. Full path as Project (the key decision)

The initial implementation stored the **basename** of the cwd as the llmhelper
Source's Project (e.g. `/home/u/code/api` → `api`). A two-axis review found the
spec's rationale — "matching Kilo and OMP presentation" — was false. Verified
against the other Sources' code and their test fixtures:

- **Claude Code**: `decode_project_name` (src/source.rs:645) decodes `-home-foo-bar` → `/home/foo/bar` (full path)
- **OMP**: `session_cwd` (src/source.rs:1218/1275) = the `cwd` field (full path; fixture: `/home/hunter/projects/omp-test`)
- **Kilo/OpenCode**: `directory` column passed through as `project` (src/source.rs:474/991); fixtures `tests/fixtures/kilo/kilo.db` and `tests/fixtures/opencode/opencode.db` both hold `/home/hunter/repos/test`

All four store full paths. The basename would make `--project /some/dir` fail
to match (the project filter is a substring match, src/filter.rs:115) and
would split one directory into two `--group-by project` buckets. Fixed:
`project_from_cwd` (src/source.rs:1589) now returns the cwd path as-is
(normalized: trailing separators stripped, root/null/empty → `""`).

### 2. Consume-once model fallback

`pending_request_model.take()` ensures a request's model is consumed by the
first response that needs it — no `response` can inherit a stale request
model across an interleaved log. The asymmetry is deliberate and documented:
`request(A) request(B) response response` → `["B", ""]`. The first response
still takes the late request's model (mispriced); only the second is
recovered as `not measured`. Fixing the first needs a request id the log
doesn't carry. See the test `an_interleaved_log_never_invents_a_second_model`
(src/source.rs:2151) and ADR 0009.

### 3. Docs corrected

- `docs/specs/0029-llmhelper-source-identity.md`: Behaviour rule, Out-of-Scope
  + Architectural Decisions + Testing Decisions all updated to "full path as-is,
  matching every other Source"; pairing section renamed from "Unambiguous
  model pairing" to "Honest model pairing (consume-once)" with explicit
  interleaving caveats.
- `CONTEXT.md` Project entry: corrected from "directory's last path component"
  to "full path", and the false claims about other Sources storing names.
- `README.md` Sources note: "last path component" → "full path, like every other Source".
- `AGENT_CHANGELOG.md`: 0029 entry now states full path, 810 tests, cites ADR 0009
  (not a phantom spec Out-of-Scope entry) for the request-id gap.
- `docs/adr/0009-consume-once-model-pairing.md`: verified correct as written
  (its asymmetry description matches the test); no edits needed.

### 4. Test suite

11 new tests (10 unit in src/source.rs, 1 e2e in tests/request.rs). The e2e
asserts `groups[0].key.ends_with("myproj")` (full path) and that
`--project myproj` still matches via substring filter.

## Commits

Two commits landed on master:

- `fb3a2ed` — feat(source): full path as Project + consume-once Model
  (src/request.rs, src/source.rs, tests/request.rs, docs/specs/0029,
  docs/adr/0008, docs/adr/0009, AGENT_CHANGELOG.md, CONTEXT.md, README.md)
- `52f4f2a` — docs(0028): align spec with runtime `man` subcommand

The second commit was amended to `52f4f2a` with the correct author and a
clean message; the working-tree changes from the spec 0028 fixes were restored
from `52f4f2a` and re-applied.

## Open items (not done, for the next session)

1. Spec 0028 still has some stale prose: the Solution section (line 46) says
   "at build time" for man-page generation, and lines 30/64 reference the
   README as "33 KB" (it is 34 KB). These are minor spec-only discrepancies
   that don't affect the implementation. The CI section and Testing Decisions
   were already corrected in the committed spec text.
2. Spec 0028 §Problem Statement line 12 says "790 tests"; the real count at
   that commit is 799. This is a spec-vs-reality mismatch.

These are tracked in the handoff but were explicitly deferred by instruction
("don't change other docs").

## Suggested skills

- **code-review** — final pass on `docs/specs/0028-distribution.md` to resolve
  the remaining stale prose items above (Solution section line 46, README size
  references at lines 30/64, test count at line 12).
- **tdd** — the `cargo test` suite at 810 passing already pins the full-path
  Project and consume-once Model behavior; no new tests needed unless the
  spec 0028 fixes are revisited.
- **research** — confirm the OMP fallback path (`decoded_project` at
  src/source.rs:1276) by reading `tests/fixtures/omp/` to verify whether any
  OMP fixture log lacks a `cwd` field and falls back to the directory name.
