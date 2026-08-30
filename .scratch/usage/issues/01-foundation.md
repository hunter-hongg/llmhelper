# 01 — Foundation: Cargo deps + Clap subcommand skeleton

**What to build:** The project compiles and runs a Clap binary that dispatches on a `#[command(subcommand)]` enum, where `usage` is the first variant. The crate builds with stable Rust (`edition = "2021"`, since the current `edition = "2024"` requires nightly). The dependency set needed for the whole feature (clap, tokio, ratatui, serde/serde_json, csv, rusqlite, chrono, toml, anyhow) is declared and resolvable. Running `llmhelper usage` with no real data path yet is a graceful no-op / friendly error, not a build failure.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] `cargo build` succeeds on stable Rust (edition set to 2021; the stub `main.rs` replaced with a Clap entrypoint)
- [ ] A `Cli` struct with `#[command(subcommand)]` enum exists; `usage` is the first variant with its own args struct
- [ ] `llmhelper --help` lists `usage`; `llmhelper usage` runs without panicking
- [ ] All dependencies from the spec (clap, tokio, ratatui, serde, serde_json, csv, rusqlite, chrono, toml, anyhow) resolve under `cargo build`
- [ ] No source-control of `.scratch/` is required (it is local working state)
