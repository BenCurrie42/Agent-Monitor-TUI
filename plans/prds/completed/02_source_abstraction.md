# PRD-02: Introduce a data-source abstraction (Claude refactor, no behavior change)

## Goal

Decouple `Store` from the Claude-Code-specific on-disk layout so a second source
(OpenCode, PRDs 03–07) can be added without forking the codebase. Today `Store`
hard-codes Claude assumptions throughout: one `*.jsonl` file per session under
`~/.claude/projects/<slug>/`, the `decode_slug`/`cwd_to_slug` path encoding,
byte-offset tail loading, and `lsof -c claude` liveness.

This PRD is a **pure refactor**: extract that knowledge behind a `Source`
abstraction and port the existing Claude behavior onto it. There must be **zero
user-visible change** — the TUI behaves byte-for-byte as it does today when only
Claude sessions are present.

This is the enabling step. It ships no OpenCode support; it only makes the next
five PRDs additive instead of invasive.

## Design direction

Introduce a `Source` trait (object-safe, stored as `Box<dyn Source>` in `Store`)
that owns everything source-specific. Suggested surface:

```rust
pub trait Source {
    /// Stable identifier for this source ("claude", "opencode").
    fn kind(&self) -> SourceKind;
    /// Root directory this source reads from (for the watcher + display).
    fn root(&self) -> &Path;
    /// Discover all projects + session metadata (cheap; no full parse).
    fn scan(&self) -> Vec<(Project, Vec<Session>)>;
    /// Full-load one session's events/usage in place.
    fn load_session(&self, session: &mut Session) -> Result<()>;
    /// Incrementally load anything new for an already-loaded session.
    fn refresh_session(&self, session: &mut Session) -> Result<()>;
    /// Map a raw FS path (from the watcher) to the affected session id, if any.
    fn session_id_for_path(&self, path: &Path) -> Option<String>;
    /// Working dirs of this source's running processes (for liveness).
    fn active_dirs(&self) -> Vec<PathBuf>;
}
```

- Add a `SourceKind` enum (`Claude`, `Opencode`) and a `source: SourceKind`
  field on `Session` and `Project` (default `Claude`) so later PRDs can badge
  and route per item. Defaulting to `Claude` keeps existing construction sites
  compiling.
- Move the current free functions (`metadata_scan_session`, `full_load_session`,
  `tail_load_session`, `cwd_to_slug`, `jsonl_ids_for`, the `lsof` helper from
  `main.rs`) into a `ClaudeSource` impl in a new `src/sources/claude.rs` (or
  `src/source.rs` with submodules). `Store` keeps the generic pieces
  (`projects`/`sessions` maps, ordering, `apply_fs_event` dispatch, the N-most-
  recent process-attribution logic in `apply_open_files`).
- `Store::new` takes a `Vec<Box<dyn Source>>` (decided: convert now, not later —
  see Notes). It may hold a single `ClaudeSource` this PRD, but the field and all
  scan/dispatch sites are vec-shaped so PRD-07 only adds a source, not a refactor.
  The byte-offset/tail-load fields on `Session` stay; they are Claude's
  implementation detail behind `refresh_session`.

## Acceptance criteria

- [ ] A failing test exists first (TDD): a test that drives a `ClaudeSource`
      over a fixture dir and asserts the same `Project`/`Session`/`EventRecord`
      results the current `Store` produces today.
- [ ] All existing `data.rs`/`store.rs` tests still pass unchanged (or are moved
      verbatim alongside the code they cover — assertions must not weaken).
- [ ] `Store` contains no string literal `.jsonl`, no `decode_slug`/`cwd_to_slug`
      call, and no `claude` process name — all live behind `ClaudeSource`.
- [ ] `Session`/`Project` gain a `source: SourceKind` field; every existing
      construction path sets it to `SourceKind::Claude`.
- [ ] `is_session_live`'s five-tier cascade is preserved for Claude exactly
      (move it if needed, but the logic and thresholds are unchanged).
- [ ] The watcher path → session mapping goes through
      `Source::session_id_for_path` (replacing the inline `jsonl_ids_for`).
- [ ] `--dump` output is identical to pre-refactor for a given fixture.
- [ ] `cargo build --release`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
      and `cargo fmt --check` are all clean.

## Out of scope

- Any OpenCode reading (PRDs 03+).
- Loading a *second* source at runtime — `Store` becomes vec-shaped now, but this
  PRD still only wires the single `ClaudeSource`. Auto-detecting and loading
  OpenCode alongside Claude lands in PRD-07.
- Changing the `Session`/`EventRecord`/`Event` data model shape beyond adding
  `source`. (OpenCode-driven model additions, e.g. richer tool-result linkage,
  come with PRD-04.)
- Any UI change, including source badges (PRD-07).

## Notes

Key files: `src/store.rs`, `src/data.rs`, `src/main.rs` (the `lsof` thread +
`claude_open_files`), `src/watcher.rs`, and a new `src/sources/` module.

Watch out for: `apply_open_files` encodes the "N processes → N most-recent
sessions" attribution that is genuinely source-agnostic — keep it in `Store` and
feed it from `Source::active_dirs()`. Only the *process name* (`claude`) and the
CWD→slug matching are Claude-specific.

Single vs. multi source (decided): `Store` holds `Vec<Box<dyn Source>>` from this
PRD, even though only `ClaudeSource` is populated until PRD-07. Retrofitting a vec
later would mean touching every scan/refresh/dispatch site a second time, so eat
that cost once, here. PRD-07 then only *adds a source to the vec* — no structural
change.

The point of this PRD is a clean seam, not new features. If a change isn't
required to create that seam, it belongs in a later PRD.
</content>
</invoke>
