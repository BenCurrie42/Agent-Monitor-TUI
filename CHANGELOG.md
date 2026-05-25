# Changelog

All notable changes are documented here.

---

## [0.1.0] — 2026-05-25

### Added
- **Theme system** — 5 built-in color themes: Coffee (Espresso & Crema, default), Nordic Frost, Forest Moss, Cyberpunk Neon, and Default Dark. Runtime-switched without restart via lock-free `AtomicU8` in `src/theme.rs`. Each theme provides a semantic palette (`border`, `highlight`, `user_badge`, `assistant_badge`, `tool_badge`, `thinking`, `ctx_filled`, `ctx_empty`).
- **Settings view** — press `s` to open a full-screen theme picker; `j`/`k` to navigate, `Enter` to apply, `Esc`/`s`/`q` to close. Applied theme persists across open/close cycles.
- **Sidebar collapse** — press `b` to toggle the sidebar; focus automatically moves to the event stream when collapsed. Auto-collapses when terminal width < 100 columns.
- **Path simplification in event stream** — Read/Write/Edit tool calls now display paths relative to the session's working directory. Absolute paths outside the project root are shown in full. Paths longer than 50 chars are middle-truncated with `…`.
- **Session `cwd` field** — populated during the metadata scan from the first JSONL line that carries a `cwd` key; used for path simplification without requiring a full load.
- **New sessions open at bottom** — selecting a session for the first time now starts the stream cursor and viewport at the bottom (follow mode) instead of the top.

### Changed
- **Modern event-stream labels** — `[USER]`, `[ASST]`, `[TOOL]`, `[OUT]`, `[ERR]`, `[THK]`, `[SYS]`, `[TTL]` replace the old freeform prefix strings (`user`, `asst`, `tool`, `↩`, etc.).
- **All UI colors route through the active theme** — border, focus ring, badges, and CTX gauge bars now read from `theme::current()` instead of hardcoded `Color::*` constants.
- **Viewport anchor preserved during `v` toggle** — toggling meta-event visibility now keeps the event under the cursor in the same visual row rather than jumping to position 0.
- **Stream and sidebar caches** — `StreamCache` (keyed on session_id + events.len + show_meta) and `sidebar_rows_cache` are stored in `AppState`, eliminating redundant rebuilds on every render frame and key handler call.
- **Control-character sanitization** — all rendered text strips `\r`, `\t`, ANSI escapes, and other control chars before passing to ratatui, preventing diagonal-cascade terminal artifacts caused by embedded escape sequences in tool output.
- **Modal close repaint** — leaving Detail, Filter, Help, or DeleteConfirm mode now calls `terminal.clear()` to erase residual ratatui buffer cells from the closed overlay.
- **`tool_summary` fallback** — unknown tools now serialize the full input JSON instead of returning an empty string.
- **flake.nix** — updated Nix dependencies.

### New files
- `src/theme.rs` — 5-theme color system; `ThemeVariant` enum, per-theme `ThemeColors` struct, lock-free `set()`/`current()` API backed by `AtomicU8` + `OnceLock`.

## [0.0.6] — 2026-05-24

### Added
- **Context window pressure gauge** — fourth line in the session info header shows `CTX [████████░░░░░░░░░░░░] 38% (76k / 200k)`. Calculated as the full context sent in the most recent assistant turn (`input_tokens + cache_creation_input_tokens + cache_read_input_tokens`). Color shifts green → yellow → red at 50%/75% thresholds. Populated at startup from the metadata scan tail pass — no session select required. Model limits via new `model_context_window()` in `data.rs`: 1M for Opus 4.6/4.7 and Sonnet 4.6, 200k for all others.
- **Syntax highlighting for Read tool output** — file content in the event stream (both inline OUTPUT blocks and standalone tool-result entries) is syntax-highlighted by file extension using `syntect` v5 with the `base16-ocean.dark` theme. Lazy-initialized once via `OnceLock`. Falls back to plain text for unrecognized extensions.

### Changed
- **Opus pricing** in `model_price()` updated to current rates: $5/$25 per MTok input/output (was $15/$75).
- **Session info panel** height extended from 5 → 6 lines to accommodate the CTX gauge row.

## [0.0.5] — 2026-05-23

### Fixed
- **Phantom characters** — Ratatui alternates two buffer slots and only flushes cell diffs; cells not explicitly rendered in a frame retain their symbol from 2 frames prior, and if both slots hold the same stale symbol no diff is generated and the terminal keeps showing old content. Fixed by rendering `Clear` over every panel area and over the full terminal at the start of each frame so uncovered cells are guaranteed to diff as spaces against any prior content.
- **Detail modal scroll locked after `G`** — `clamp_scroll` used logical line count, but `Paragraph::scroll` counts visual (wrapped) lines. Wide JSON content with few logical lines but many wrapped rows produced max-scroll = 0, disabling scrolling entirely. Fixed with `visual_line_count_str` / `visual_line_count_lines` helpers that divide each line's display width by the area width; the clamped position is also written back to `app.detail_scroll` so `k` after `G` starts from the actual clamped bottom rather than `u16::MAX`.
- **Terminal resize artifacts** — `terminal.clear()` now called on `AppEvent::Resize` to erase characters outside the new (smaller) render area that ratatui's buffer does not cover.

## [0.0.4] — 2026-05-23

### Added
- **Nix flake** (`flake.nix`) — `nix develop` drops into a dev shell with Rust toolchain and macOS SDK frameworks (CoreServices, libiconv); `nix build` produces the release binary; supports aarch64-darwin, x86_64-darwin, and Linux
- **GitHub Actions release workflow** (`.github/workflows/release.yml`) — builds pre-compiled binaries on tag push (`v*`) for aarch64-apple-darwin, x86_64-apple-darwin, and x86_64-unknown-linux-musl; uploads them to a GitHub Release with auto-generated notes
- **crates.io metadata** — `Cargo.toml` gains `repository`, `homepage`, `readme`, `keywords`, `categories`, and `exclude` fields; enables `cargo publish`

### New files
- `flake.nix` — Nix flake with `devShells.default` (dev shell) and `packages.default` (release binary build)
- `flake.lock` — pinned nixpkgs/flake-utils inputs
- `.github/workflows/release.yml` — CI release pipeline: build + upload pre-compiled binaries to GitHub Release

## [0.0.3] — 2026-05-23

### Added
- **`Session.project_has_claude`** — true while any `claude` process has this session's project as its CWD, even if that process is driving a different session; prevents stale siblings from being misclassified as live
- **`Session.process_ever_open`** — latches true the first time lsof sees a claude process for this session; transitions the session to Cold once the process exits
- **`Session.process_closed_at`** — records when `process_open` transitions `true → false`
- **`Session.exit_observed`** — set when an explicit `/exit` or `/quit` slash command is parsed from the event stream; immediately marks the session Cold regardless of process or timestamp state
- **`cwd_to_slug`** helper (`store.rs`) — re-encodes absolute paths as the project-slug format Claude Code uses for `~/.claude/projects/` directory names
- **`is_exit_command`** helper (`store.rs`) — detects `/exit` and `/quit` commands in the `<command-name>…</command-name>` wrapper format

### Changed
- **lsof strategy** — `claude_open_files()` now queries process CWDs (`lsof -a -c claude -d cwd`) instead of open file paths; returns `Vec<PathBuf>` (one entry per running process) rather than `HashSet`; the `-a` flag ANDs the `-c`/`-d` filters to avoid returning every process on the system
- **lsof polling interval** — reduced from 5 s to 1 s for faster liveness response
- **`apply_open_files`** — rewritten to accept CWD paths; counts running claude processes per project slug, then marks the N most-recently-active sessions in a project as `process_open = true` (N = number of claude processes in that project)
- **`is_session_live` logic** — five-tier cascade: `exit_observed` (definitive dead) → `process_open` (definitive live) → `project_has_claude` (live process, different session → dead) → `process_ever_open` (process exited → dead) → timestamp fallback (no process info)
- **`liveness()` UI helper** — `exit_observed → Cold`; `process_open` shows Live (<30 s activity) or Recent (quiet but alive); `process_ever_open → Cold`; falls through to timestamp heuristic only if no process was ever seen
- **Sidebar `l` key** — redesigned as tree-style "step in": on a Session row, shifts focus to the events stream; on a collapsed Project/header, expands it; on an already-expanded Project/header, moves cursor onto its first child
- **Sidebar `h` key** — redesigned as tree-style "step out": on a Session row, moves cursor to its parent header; on an expanded Project/header, collapses it; `h` in the events panel moves focus back to the sidebar
- **`Enter` key** — context-aware: in the Sidebar shifts focus to the events stream; in the events stream opens the detail modal
- **`g`/`G`/`j`/`k` sidebar navigation** — calls `refresh_selection_from_cursor` immediately so selection tracks the cursor without waiting for the next render
- **`resolve_selection`** — pins `sidebar_cursor` to the row of the currently-selected session, preventing background FS events and lsof ticks from dragging the cursor off the user's selection
- **Help overlay** — updated `h`/`l` description to "Step out / in (l on a session = focus events)" and `Enter` to "Sidebar: focus events · Events: open detail"
- **`AppEvent::OpenFiles`** — payload changed from `HashSet<PathBuf>` to `Vec<PathBuf>` to carry one entry per process with duplicates

### Removed
- **`ExpandTarget` enum** (`app.rs`) — replaced by `handle_sidebar_l` / `handle_sidebar_h` methods with richer per-row semantics
- **`target_at_cursor`** method — replaced by the same refactor

## [0.0.2] — 2026-05-23

### Added
- **Sub-agents section** — collapsible "Sub-agents" sidebar section groups background sessions detected via `sessionKind: "bg"`; excluded from live/closed sections
- **Delete closed sessions** — `D` key (cursor on Closed header or `[D] Delete all closed` row) opens a confirmation modal; confirmed deletion removes JSONL files from disk and evicts sessions from the store
- **Help modal** — `?` opens a keyboard-reference overlay listing all keybindings; `?` / `Esc` / `q` to close
- **Process-open liveness** — `lsof -F n -c claude` polled every 5 s in a background thread; sessions whose JSONL file is held open by a `claude` process are marked `process_open: true` and treated as live regardless of timestamp
- **`sessionKind` field** — parsed from JSONL `RawRecord`; propagated to `EventRecord.session_kind` and used to set `Session.is_background` during metadata and full-load scans
- **`Session.process_open`** and **`Session.is_background`** — new boolean fields; checked in `is_session_live` and the `liveness` UI helper
- **`OpenFiles` app event** — `AppEvent::OpenFiles(HashSet<PathBuf>)` dispatched from the lsof-polling thread; handled by `Store::apply_open_files` in the main event loop
- **`Store::delete_closed_sessions`** — removes all non-live sessions from disk and in-memory store; returns count deleted
- **`Store::apply_open_files`** — updates `process_open` for all sessions from a given path set

### Changed
- **Layout** — reorganized to top-bar (session info, 5 lines) + bottom split (sidebar 30% / events 70%); previously sidebar-left + header-top + stream-right
- **Session info panel** — redesigned from a single usage line: line 1 = title + liveness badge + last-active time; line 2 = cost / tokens / model (with `claude-` prefix stripped); line 3 = project / session ID / started time
- **Event stream labels** — replaced emoji prefix + colon format with fixed-width 6-char text labels (`asst  `, `tool  `, `think `, `user  `, `sys   `, `title `, `↩     `)
- **`ToolResults` events** — now hidden by default (treated as meta) instead of always visible
- **Statusline** — simplified to a single compact hint line; removed MODE badge and brand label
- **Sidebar title** — "Live sessions" → "Sessions"
- **Closed header** — "Closed sessions" → "Closed"; count format compacted from `(N in M project(s))` to `(N in M)`
- **Stream cursor** — clamped to the bottom-of-stream index when navigating down

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
