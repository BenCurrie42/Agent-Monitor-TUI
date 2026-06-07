# agent-monitor — Project & Agent Ruleset

This document is binding for all AI agents working in this repo. The PRD loop and
its workers read it on every task.

## 1. Project Overview

AgentMonitorTUI is a passive, lazydocker-style terminal UI for monitoring Claude
Code sessions. It reads `~/.claude/projects` JSONL files, watches them for live
updates, and provides a keyboard-driven interface to inspect conversations, track
token usage, and navigate sessions. Architecture and module map live in
`CLAUDE.md`.

- **Language / Framework:** Rust 2021 / `ratatui` + `crossterm` (TUI)
- **Package manager / runner:** `cargo`
- **Testing:** `cargo test` (inline `#[cfg(test)]` unit tests)
- **Environment wrapper:** `nix develop --command bash -c "<cmd>"` (a dev shell is
  defined in `flake.nix` with rustc/cargo/clippy/rustfmt). Plain `cargo` also
  works if the Rust toolchain is on `PATH`.

## 2. Core Principles

- **Iterative development:** small, logical, testable increments.
- **Design-first:** every unit of work is a PRD in `plans/prds/`, approved before
  implementation.
- **TDD-native:** a failing test exists before application code changes.

## 3. Mandatory Quality Gates

Code is not "done" until ALL pass:

1. **Tests green:** `cargo test`
2. **Build:** `cargo build --release` succeeds.
3. **Lint:** `cargo clippy --all-targets -- -D warnings` clean.
4. **Format:** `cargo fmt --check` clean.

Coverage: there is no enforced coverage percentage. Any new logic (parsing,
state, liveness, filtering) must ship with unit tests that exercise the
acceptance criteria. Pure rendering code (`ui.rs`) is exempt from the test
requirement but must still build and pass clippy.

## 4. PRD Lifecycle

1. **Draft:** create `plans/prds/NN_short_name.md`. Implementation needs approval.
2. **Branch:** create a feature branch off latest `main`. **Never** push to `main`.
3. **Test-first:** write failing tests for the PRD's acceptance criteria.
4. **Implement:** make them pass.
5. **Verify:** run the full suite + build + lint + format (section 3).
6. **Finalize:** move the PRD to `plans/prds/completed/`.
7. **PR:** push the branch and open a PR — only after explicit operator approval.

## 5. Conventions

- **Commits:** natural-language summary, prefixed with the PRD number
  (e.g. "PRD-03: add session search"). No conventional-commit prefixes.
- **Module boundaries:** respect the separation in `CLAUDE.md` — data model and
  parsing in `data.rs`, FS/store logic in `store.rs`, UI state in `app.rs`,
  rendering in `ui.rs`. Don't put parsing logic in the UI layer or vice versa.
- **Theming:** colors come from `theme.rs` (`ThemeColors`); never hard-code color
  values in `ui.rs`.
- **Timestamps:** use `chrono` and keep the existing UTC-vs-local handling
  consistent with surrounding code.
- **Release sync:** after any version bump, `flake.nix` `version` and the three
  `fetchurl` hashes must be updated to match `Cargo.toml` (see the release
  checklist in `CLAUDE.md`). Diverging versions break Linux installs.

## 6. Agent Constraints

- **NEVER** bypass quality gates.
- **NEVER** commit or push without explicit operator approval.
- **NEVER** modify application source without an established failing test.
- **NEVER** run destructive or irreversible commands (force-push, history
  rewrite, deleting user `~/.claude` data, publishing a crate/release) without
  explicit approval.
- **Stop on blocked workflows:** if a tool needs interactive input or is blocked,
  do NOT hack around it. Stop, explain, and ask for guidance.

## 7. Definition of Done

Success for any PRD is strictly bound to the Quality Gates in section 3. Do not
signal completion until every gate is met.
