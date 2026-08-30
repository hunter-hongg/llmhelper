# Source trait + registry for pluggable agents

Each agent tool (Claude Code, OpenCode, and future ones like Codex/Gemini CLI/Aider) is a `Source` trait with `fn load(&self) -> Vec<Record>`. They are enumerated through a registry rather than a hardcoded dispatch.

Driven by the likelihood of adding more agents; a new source is then a single new struct, with no changes to the loader path or UI. The rejected alternative was two concrete loader functions (`load_claude`, `load_opencode`) called directly — simpler now, but every new source would touch dispatch code. The trade-off accepted is a small amount of trait/registry boilerplate up front in exchange for a clean extension point.
