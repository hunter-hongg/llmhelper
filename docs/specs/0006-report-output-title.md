---
id: 0006
title: "report --output and --title for shareable Markdown"
status: ready-for-agent
created: 2026-09-06
triage: ready-for-agent
---

## Problem Statement

`llmhelper report` currently prints a Markdown document to stdout with a fixed title "# llmhelper report". Users who want to save the report must shell-redirect manually, and the title cannot be changed to reflect the specific window, project or audience. Sharing in chat/PR loses context about what the report is for.

## Solution

Add two optional CLI flags to `report`:
- `--output <path>` writes the rendered Markdown to the given file instead of stdout (stdout remains the default)
- `--title <string>` replaces the document title heading with the supplied string; when absent the current "# llmhelper report" is used

The rendering pipeline stays pure; file writing is a thin I/O layer around the existing `render_report` function. `ReportMeta` gains an optional title field that flows through to the renderer.

## User Stories

1. As a user, I want `--output weekly.md` so that I can save a report to a file without shell redirection
2. As a user, I want `stdout` to remain the default when `--output` is not given, so existing scripts keep working
3. As a user, I want `--title "Team weekly LLM usage"` so that pasted Markdown carries a meaningful heading
4. As a user, I want the title to be optional and fall back to "# llmhelper report" so existing invocations are unaffected
5. As a user, I want the file to be overwritten if it exists, with a warning on error, so failures are visible
6. As a user, I want the same filter flags as before to still work with `--output`/`--title`
7. As a script author, I want deterministic Markdown content apart from timestamp, so diffs of saved reports stay meaningful
8. As a script author, I want the file write to fail clearly with a descriptive error naming the destination, so I can fix the destination instead of debugging a silent partial file
9. As a team lead, I want a custom title to reflect the reporting window and audience, so shared reports are self-describing
10. As a developer, I want the renderer to stay pure and testable, so adding title/output does not increase test complexity
11. As a developer, I want `ReportMeta` to carry the title, so header construction is centralized
12. As a user, I want the generated timestamp and window to remain in the header regardless of title/output
13. As a user, I want error messages for invalid paths to be clear, so I can fix the destination
14. As a user, I want `--output` to work with all existing filters (`--last`, `--since`, `--project`, `--model`, `--source`, `--group-by`, `--top`)

## Implementation Decisions

- Extend `ReportArgs` in `src/cli.rs` with:
  - `title: Option<String>` `#[arg(long = "title")]`
  - `output: Option<PathBuf>` `#[arg(long = "output")]`
- Extend `ReportMeta` in `src/report.rs` with `title: Option<String>`; builder `report_meta` populates it from `args.title`
- Modify `render_report` to use `meta.title.as_deref().unwrap_or("llmhelper report")` for the first heading `# ...`
- Modify `run_report` in `src/main.rs` to:
  - Render Markdown via existing pure function
  - If `args.output` is Some, write bytes to path using `std::fs::write`, propagate errors
  - Else print to stdout via `println!`
- Keep `render_report` pure: no I/O, only string generation
- Validation: `ReportArgs::validate` remains unchanged; output path validation deferred to write error
- No new dependencies; uses std lib only
- Title markdown escaping: apply same `md_cell` logic to title to avoid pipe breakage

## Testing Decisions

- Unit tests for `render_report`: with `ReportMeta.title = Some("Custom")` asserts first line is `# Custom`
- Unit test: with title None asserts default "# llmhelper report"
- Unit test: title containing `|` is escaped correctly
- Integration test: `llmhelper report --last 7d --output /tmp/out.md` creates file and content contains expected headings
- Integration test: `--output` with unwritable path returns error and exits non-zero
- Existing report unit tests remain green; no behavior change when flags absent

## Out of Scope

- Interactive prompt for title or output
- Format other than Markdown
- Automatic opening of file
- Diff or pagination inside report
- Compression or encoding options

## Further Notes

The change preserves backward compatibility: omitting both flags produces identical output to today. File writing is the only new I/O and stays outside the pure renderer, maintaining testability.
