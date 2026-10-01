---
id: 0028
title: "distribution — make llmhelper installable and discoverable outside this checkout"
status: ready-for-agent
created: 2026-09-27
updated: 2026-09-27
triage: ready-for-agent
---

## Problem Statement

- `llmhelper` is a finished tool — 10 subcommands, 810 tests, 27 shipped specs,
7 ADRs — that nobody but this checkout's author can use. Four concrete
blockers, in descending order of how much they cost a new user:

- **It cannot be installed cleanly.** `Cargo.toml` has no `license`, no
  `repository`, no `readme`, no `categories`, no `rust-version`. `cargo
  install --path .` works, but `cargo install llmhelper` is impossible and a
  published `.crate` would be rejected by crates.io. There is no LICENSE file
  at all, which is a hard blocker for redistribution of any kind.
- **It needs a system library the README never mentions.** `reqwest` 0.11
  resolves `native-tls` → `openssl-sys`, so a build requires OpenSSL
  development headers. This is not a hypothetical: this machine could not
  compile the project until `openssl-devel` was installed, and the failure
  message points at `pkg-config`, not at llmhelper. A new user hits this
  before seeing a single line of llmhelper's own output.
- **It is undiscoverable.** No man page, no shell completions, no `build.rs`.
  A user who installs it must know that `--match` exists, that there are ten
  subcommands, and that `Tab` cycles a grouping in the `usage` TUI. None of
  that is reachable from `llmhelper --help` alone, and the README — 35 KB —
  is far too long to read before first use.
- **Nobody checks it still builds.** No CI, no `rust-toolchain.toml`. Every
  claim in the changelog about "790 tests green" rests on one machine, one
  moment. A published tool whose build breaks on a dependency bump is a
  support burden.

## Solution

Make the tool installable, discoverable, and verifiable by someone who has
never seen this repository, without changing what it does.

- **Package metadata + LICENSE** so `cargo install` and crates.io both work.
- **Document the build prerequisites** (OpenSSL headers) in the README, so the
  one hard prerequisite is stated before it is hit rather than after.
- **Generate man pages and shell completions** from the existing clap
  definitions, at run time, so they cannot drift from the CLI.
- **Add CI** that runs the test suite, clippy, and fmt on Linux and macOS.
- **Add a "Getting started / install" section** at the top of the README —
  the first screen a crates.io visitor sees.

Deliberately **not** done: no crates.io publish (that needs an owner decision
about name and visibility), no shell-installer script, no Homebrew formula, no
self-update mechanism. This spec makes the tool *installable*; it does not
decide *where it is installed from*.

## User Stories

1. As a user on a fresh machine, I want `cargo install llmhelper` to work, or
   to be told exactly what is missing, so I can run the tool.
2. As a user, I want the README to state the OpenSSL prerequisite before I hit
   a build error, so I am not confused by a `pkg-config` message about a
   crate I have never heard of.
3. As a user, I want `man llmhelper` and `llmhelper <TAB>` to work, so I can
  discover the ten subcommands and their flags without reading 35 KB.
4. As a user, I want the help output to be a usable map of the tool, so
   `--help` alone tells me what exists.
5. As the author, I want CI to run the tests, clippy, and fmt on every push, so
   "it builds" is a fact I did not personally verify this morning.
6. As the author, I want the generated man pages and completions to be derived
   from the clap definitions, so they can never describe a flag that no longer
   exists.

## Behaviour

### LICENSE and package metadata

- `Cargo.toml` gains `license`, `repository`, `readme`, `categories`,
  `keywords` (extended), `rust-version`, and `include`.
- A `LICENSE` file at the repository root states the license.
- The license choice is a decision the author must make; this spec's
  implementation uses MIT, which is the most common choice for a tool of this
  shape and the one compatible with every dependency in the tree (all
  MIT/Apache-2.0/BSD/ISC/Unicode-DFS).

### Build prerequisites, documented

- README gains an **Install** section stating: Rust toolchain, and OpenSSL
  development headers (Debian/Ubuntu `libssl-dev`, Fedora/RHEL
  `openssl-devel`, macOS/Homebrew `openssl` + `pkg-config`), with the exact
  `cargo install --path .` command.
- The reason is named, not hand-waved: `reqwest` 0.11 uses `native-tls`.

### Man pages and completions

- A `man` subcommand (`src/dist.rs`, hidden via `#[command(hide = true)]`)
  renders, on demand, from the in-memory `Cli` tree of the running binary:
  - a man page per subcommand (`llmhelper.1`, `llmhelper-usage.1`, …) via
    `clap_mangen`;
  - completion scripts for bash, zsh, fish, and PowerShell via
    `clap_complete`.
- Output goes to a directory (`target/dist/` by default; `--out-dir`,
  `--stdout`, and `--completions <shell>` for one script on stdout, for
  packaging). A `build.rs` could not host this generator: `src/cli.rs` reaches
  into `crate::filter`, `crate::config`, and the rest of the library, so a
  build script would have to depend on the crate it builds — a circular
  dependency cargo will not run (ADR 0008).
- Man pages and completions are **generated at run time, not at build time** or
  checked in. This is the whole point: a checked-in page drifts the first time
  a flag is added, and the drift is invisible. Generating from the live clap
  definition at the call site makes that drift unrepresentable.

### Help output as a map

- Each subcommand's `about` line is one sentence that says what the command
  answers, not what it does mechanically. `usage` → "Summarise token
  consumption and Cost over a time window."
- Where a TUI command has key bindings that `--help` cannot show, its `after_help`
  names them. This is the one piece of discoverability that costs no
  generation machinery: clap already renders `after_help`.

### CI
- `.github/workflows/ci.yml` runs on push and pull request:
  - `cargo fmt --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test --all-targets`
  on `ubuntu-latest` and `macos-latest`.
- A separate `install` job runs `cargo install --path . --root /tmp/llmhelper-install`
  on `ubuntu-latest` and smoke-tests the installed binary (`--version`,
  `man --completions bash --stdout`, `--help`) — the exact command a user
  runs, proving the release profile compiles.
- A `rust-toolchain.toml` pins the channel, so a local toolchain and CI agree.

## Out of Scope

- **Publishing to crates.io.** The packaging metadata this spec adds is what
  *enables* publishing; actually running `cargo publish` needs an owner
  decision (name, visibility, yank policy) and is a separate, deliberate act.
- **A shell installer (`curl | sh`).** Every such script is a supply-chain
  liability; `cargo install` is the supported path and this project already
  depends on a Rust toolchain to build.
- **Homebrew formula / Linux package.** Same reason: distribution channel
  decisions belong to an owner, and each one is its own maintenance burden.
- **Self-update (`llmhelper upgrade`).** A tool that rewrites its own binary
  has a security story of its own. `cargo install --force` is the answer.
- **Switching `reqwest` to rustls to remove the OpenSSL prerequisite.** This is
  genuinely tempting — it would delete the one hard native dependency, and
  `native-tls` is deprecated in newer reqwest. It is excluded because it
  changes the TLS backend of a command that sends API keys, which is a
  security-relevant change needing its own testing against real providers.
  Documenting the prerequisite is the honest smaller fix; the migration is
  filed as follow-up work.
- **Vendoring a config-file example.** README documents the config keys.

## Architectural Decisions

- **Man pages and completions are generated at run time, not committed.**
  The alternative — checking them in — means every flag change requires
  regenerating a binary-ish artifact, and a stale man page is worse than none
  because a user trusts it. A `man` subcommand renders them from the live
  `Cli` tree of the running binary; a `build.rs` cannot host the generator,
  because `src/cli.rs` reaches into `crate::filter`, `crate::config`, and the
  rest of the library, so a build script would have to depend on the crate it
  builds — a circular dependency cargo will not run (ADR 0008). The cost is a
  few hundred KB of runtime generator code a user never invokes; the benefit
  is that the help output is *provably* the help output.
- **Help text is written as answers, not as mechanics.** Every subcommand
  answers a question ("which project ate the budget?", "what did we say about
  X?"). The `about` line is that question's short form. This costs nothing
  and is the difference between a tool that feels designed and one that feels
  generated.
- **The OpenSSL prerequisite is documented, not engineered away.** Switching
  to rustls is the better long-term answer, but it is a change to how API keys
  are transmitted, and it deserves its own spec and its own testing against
  real providers. This spec ships the honest documentation and files the
  migration.
- **CI runs `clippy -D warnings`, not `clippy`.** The repo's stated standard
  (in every prior changelog entry) is zero warnings; CI that merely reports
  them lets them accumulate.

## Testing Decisions

- A test that runs `llmhelper man` and asserts the expected man-page and
  completion filenames appear and are non-empty. This is the runtime seam (not
  a build script) catching the "I renamed a subcommand and the generator
  silently produced nothing" failure.
- `cargo install --path .` and `cargo package --list` are verified in CI — the
  former is the exact command a user runs (it proves the release profile
  compiles), the latter is what crates.io would reject on.
- An assertion that every subcommand has a non-empty `about` prevents a
  subcommand from shipping with a blank help line.
- No test asserts the *content* of generated man pages (that is clap's job);
  tests assert their existence and that the CLI definition they derive from
  is complete.

## Further Notes

- `cargo install` builds with `--release`; the TUI is the only interactive
  part, so build time matters more than usual for a tool people install.
- The `include` list keeps the `.crate` small: the 108 KB changelog and
  34 KB README are the two large tracked docs, and the ADRs/specs are worth
  shipping because the README links into them.
- crates.io has a 10 MB package limit; the current tree is far under it, but
  `target/` must never be packaged (`.gitignore` already excludes it, and
  `include` makes that explicit).
