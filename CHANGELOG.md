# Changelog

All notable changes are documented here.

---

## [0.0.1] — 2026-05-23

### Added
- **TUI layout** — three-pane layout: sidebar (32%), session header (6 lines), event stream + statusline (68%), rendered with `ratatui`
- **Sidebar** — projects and sessions sorted by most-recent activity; live sessions in top section, closed sessions in a collapsible `▶ Closed sessions` section with per-project expand/collapse
- **Liveness indicators** — green (●, <30s), yellow (●, <5min), gray (○, cold); applied to sidebar bullets and the session header status badge
- **Session header** — displays AI-generated title (or first user line), project path, session UUID prefix, liveness badge, relative last-activity time, and per-session token usage + estimated USD cost (Opus / Sonnet / Haiku pricing)
- **Event stream** — scrollable list of JSONL events; assistant turns expanded into per-block rows (thinking / text / tool_use), user tool_results expanded per result; sidechain events prefixed with `└─`
- **Follow mode** — auto-scrolls to the latest event; toggled with `f` or disabled automatically when navigating up
- **Meta visibility toggle** — `v` shows/hides low-value system events (system, attachment, ai-title, last-prompt, permission-mode, file-history-snapshot)
- **Detail modal** — `Enter` opens a scrollable pretty-print view of the selected event; `R` switches to raw JSON; tool_use blocks show their matching tool_result inline
- **Tool rendering** — purpose-built display for Bash (`$ command`), Read, Write (with content), Edit (diff `from`/`to` colored red/green), Glob, Grep, WebFetch, WebSearch, and Task/Agent (subagent prompt)
- **Filter mode** — `/` opens a centered overlay for substring search; matches against all textual event content including tool names, inputs, and results; `Esc` clears
- **Keyboard navigation** — `j`/`k` (up/down), `h`/`l` (collapse/expand), `Tab` (focus sidebar↔stream), `g`/`G` (top/bottom), `q` quit; detail modal: `j`/`k` scroll, `d`/`u` page, `R` raw toggle, `Esc` close
- **JSONL parser** (`src/data.rs`) — parses `user`, `assistant` (thinking/text/tool_use blocks), `system`, `attachment`, `ai-title`, `last-prompt`, `permission-mode`, `file-history-snapshot`, and unknown event types; fields truncated at 4096 chars
- **Token usage tracking** — aggregates `input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens` per session; estimates USD cost using Anthropic published pricing; flags sessions with unknown model names with `*`
- **Tool cross-linking** — `tool_use_index` and `tool_result_index` maps link tool calls to their results by `tool_use_id` for inline display in the detail modal
- **Slug decoding** — converts `~/.claude/projects/-Users-x--config-nix` style slugs to human-readable paths (`/Users/x/.config/nix`)
- **Store** (`src/store.rs`) — lazy-load strategy: metadata scan (64KB head + 16KB tail) at startup for all sessions; full parse deferred until session is selected; incremental tail load on FS modify events
- **File watcher** (`src/watcher.rs`) — `notify-debouncer-mini` with 200ms debounce on `~/.claude/projects`; maps debounced events to `Created`/`Modified`/`Removed` by path existence
- **Headless dump mode** — `--dump` prints project/session summary with token totals and exits without launching the TUI
- **CLI flags** — `--projects-dir`, `--session` (UUID or ≥4-char prefix), `--no-follow`, `--debug`
- **macOS build fix** (`build.rs`) — auto-discovers SDK lib path via `xcrun` to resolve `libiconv` linker errors with nix-managed Rust toolchains
- **Unit tests** — `data.rs` covers slug decoding, user/assistant/tool-result line parsing, unknown event handling, usage cost calculation, and unknown-model flagging

### New files
- `src/main.rs` — entry point, CLI arg parsing (`clap`), terminal setup/teardown, `crossbeam-channel` event loop
- `src/app.rs` — `AppState`, keyboard handling, `sidebar_rows`, `stream_items`, filter matching, `SidebarRow`/`StreamItem` types
- `src/data.rs` — `Project`, `Session`, `EventRecord`, `Event`, `UsageTotals`, `ModelPrice`, JSONL `parse_line`
- `src/store.rs` — `Store`, `FsEvent`, metadata scan, full/tail load, tool index maintenance
- `src/ui.rs` — all `ratatui` rendering: sidebar, header, stream, detail modal, statusline, filter overlay
- `src/watcher.rs` — `WatcherHandle`, `spawn_watcher` via `notify-debouncer-mini`
- `build.rs` — macOS SDK lib path discovery for nix toolchain linker fix
- `Cargo.toml` — package manifest; deps: ratatui, crossterm, notify, notify-debouncer-mini, serde, serde_json, clap, anyhow, chrono, dirs, crossbeam-channel
