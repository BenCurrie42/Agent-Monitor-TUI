---
name: release
description: End-to-end release for Agent-Monitor-TUI — bumps version across Cargo.toml/Cargo.lock/flake.nix/CLAUDE.md, syncs README + CHANGELOG, commits, tags, pushes, watches the GitHub release workflow, then updates flake.nix fetchurl hashes from the published binaries. Use when the user says "release", "cut a version", "ship vX.Y.Z", "tag a release", or "update release docs" in this repo.
---

# Release — Agent-Monitor-TUI

Full release pipeline for **this repo specifically**. Unlike the generic `release-docs`
skill, this one knows the exact files, the Nix `fetchurl` hash dance, and the
tag → CI → hash-update sequence. Run the whole thing end to end.

Repo facts (hard-coded — verify before relying on them):
- GitHub: `BenCurrie42/Agent-Monitor-TUI`
- Binary name: `agentmonitor`; release assets: `agentmonitor-aarch64-apple-darwin`,
  `agentmonitor-x86_64-apple-darwin`, `agentmonitor-x86_64-unknown-linux-musl`
- Release workflow: `.github/workflows/release.yml`, triggered on `v*` tag push
- `flake.nix` distributes **pre-built release binaries via `fetchurl`** (NOT
  `buildRustPackage`). Its three `hash` values are SRI digests of the published
  assets and can only be computed *after* the workflow uploads them.

## Step 1 — Gather version + scope

Ask the user for the version (or bump type off the current `Cargo.toml` version) and
release date (default: today, from the environment's current date).

Determine current version from `Cargo.toml` `version = "X.Y.Z"`.

Understand what changed since the last release:
```bash
git describe --tags --abbrev=0          # last tag, e.g. v0.1.2
git log --oneline <lastTag>..HEAD
git diff <lastTag>..HEAD --stat
```
Read the actual diffs for non-trivial changes — don't trust commit messages alone.
Separate **user-facing/behavioral** changes (go in CHANGELOG Added/Changed/Fixed and
maybe README/CLAUDE.md) from **cleanup** (fmt/clippy/scaffolding — one Changed line).

## Step 2 — Bump version fields

Edit each (do NOT hand-edit `Cargo.lock` — `cargo build` regenerates it):

| File | What to change |
|---|---|
| `Cargo.toml` | `version = "X.Y.Z"` under `[package]` |
| `flake.nix` | `version = "X.Y.Z";` (the `let`-bound var near the top) |
| `CLAUDE.md` | the `## Version` line: `X.Y.Z — YYYY-MM-DD` |

Then regenerate the lockfile and confirm the build is clean:
```bash
cargo build --release 2>&1 | tail -5
grep -A1 'name = "agent-monitor-tui"' Cargo.lock   # should show new version
```

## Step 3 — CHANGELOG.md

Prepend a new entry directly after the `---` header line, above the previous version.
Format (only include sections that have entries):

```
## [X.Y.Z] — YYYY-MM-DD

### Added
- **Feature** — description with concrete detail (file paths, fn/type names, behavior).

### Changed
- **Thing** — what and why.

### Fixed
- **Bug** — what it was and the cause.

### New files
- `path` — one-line purpose.
```

Match the existing entries' density: name `src/*.rs` files, types, fns, and behaviors.

## Step 4 — README.md + CLAUDE.md (only if stale)

Patch surgically — don't rewrite sections.
- **README.md** "How it works" paragraph: theme count, feature list, flags, keys.
  (e.g. the theme count lives in the last paragraph: "pick from N color themes…")
- **CLAUDE.md**: module table (`src/theme.rs` "N-theme color system"), key types,
  data-flow, CLI flags, and the `## Version` note. Keep it accurate to the code.
- **AGENTS.md** references CLAUDE.md and rarely needs edits — touch only if the
  release changes quality gates / PRD lifecycle / conventions.

Skip README/CLAUDE.md entirely for internal-only refactors.

## Step 5 — Commit, tag, push

```bash
git add -A
git commit -F - <<'EOF'
Release vX.Y.Z: <one-line summary>

<short body: what changed, note flake hashes land in a follow-up commit>

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
git tag -a vX.Y.Z -m "Release vX.Y.Z: <summary>"
git push origin main
git push origin vX.Y.Z
```

Tagging + pushing is an outward action — only do it when the user has asked to tag/release
(they did if they invoked this skill with that intent). Otherwise stop after Step 4 and ask.

## Step 6 — Watch the release workflow

```bash
gh run list --repo BenCurrie42/Agent-Monitor-TUI --workflow release.yml --limit 1
gh run watch <run-id> --repo BenCurrie42/Agent-Monitor-TUI --exit-status
```
A run takes ~1.5–2 min and builds all three targets. If it fails, stop and report —
do NOT update flake hashes against a failed/partial release.

## Step 7 — Update flake.nix hashes from published binaries

Fetch the asset digests and convert each hex digest to Nix SRI:
```bash
gh release view vX.Y.Z --repo BenCurrie42/Agent-Monitor-TUI --json assets \
  --jq '.assets[] | "\(.name) \(.digest)"'
```
For each `sha256:<hex>` digest, convert:
```bash
echo "<hex>" | xxd -r -p | base64   # prefix the result with "sha256-"
```
Map by asset name → flake attr:
- `agentmonitor-aarch64-apple-darwin` → `aarch64-darwin`
- `agentmonitor-x86_64-apple-darwin` → `x86_64-darwin`
- `agentmonitor-x86_64-unknown-linux-musl` → `x86_64-linux`

Replace all three `hash = "sha256-…";` values in `flake.nix`, then commit + push:
```bash
git add flake.nix
git commit -m "chore: update flake.nix hashes for vX.Y.Z release binaries

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
git push origin main
```

## Step 8 — Final divergence check

Confirm the versions match (the checklist in CLAUDE.md mandates this — diverging
versions break Linux/Nix installs):
```bash
grep 'version = "' Cargo.toml | head -1
grep 'version = "' flake.nix
```
Both must show `X.Y.Z`. Report the run URL, the three new hashes, and the two commits.

## One-shot helper for Step 7 conversion

```bash
gh release view vX.Y.Z --repo BenCurrie42/Agent-Monitor-TUI \
  --json assets --jq '.assets[] | "\(.name) \(.digest)"' \
| while read name digest; do
    hex=${digest#sha256:}
    printf '%-45s sha256-%s\n' "$name" "$(echo "$hex" | xxd -r -p | base64)"
  done
```
