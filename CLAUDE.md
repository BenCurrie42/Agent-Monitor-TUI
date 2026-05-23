# AgentMonitorTUI — Claude Code Context

A passive, lazydocker-style TUI for monitoring Claude Code sessions. Reads `~/.claude/projects` JSONL files, watches them for live updates, and provides a keyboard-driven interface to inspect conversations, track token usage, and navigate sessions.

## Modules

| File | Role |
|---|---|
| `src/main.rs` | Entry point: CLI args (`clap`), terminal setup/teardown, `crossbeam-channel` event loop |
| `src/app.rs` | `AppState`, keyboard handling, `sidebar_rows`, `stream_items`, filter matching |
| `src/data.rs` | Data model, JSONL `parse_line`, `UsageTotals`, `ModelPrice`, slug decoding |
| `src/store.rs` | `Store`: project/session maps, initial scan, lazy full load, incremental tail load, FS event handling |
| `src/ui.rs` | All `ratatui` rendering: sidebar, header, event stream, detail modal, filter overlay, statusline |
| `src/watcher.rs` | `notify-debouncer-mini` watcher; maps FS events to `FsEvent` enum |
| `build.rs` | macOS SDK lib path discovery via `xcrun` (libiconv linker workaround for nix toolchains) |

## Key types

- **`AppState`** (`app.rs`) — all mutable UI state: focus, mode, follow, cursors, viewport, filter, expanded set
- **`Store`** (`store.rs`) — `BTreeMap<String, Project>` + `HashMap<String, Session>`
- **`EventRecord`** (`data.rs`) — parsed JSONL line: `Event` enum + timestamp, model, sidechain flag, byte offset
- **`FsEvent`** (`store.rs`) — `Created | Modified | Removed(PathBuf)` dispatched from the watcher thread

## Data flow

```
~/.claude/projects/**/*.jsonl
        │
        ├─[startup] Store::initial_scan → metadata_scan_session (64KB head + 16KB tail read)
        ├─[select]  Store::ensure_loaded → full_load_session (full parse)
        └─[watch]   WatcherHandle → FsEvent → Store::apply_fs_event → tail_load_session
                                                        │
                                              AppEvent::Fs → main loop → render
```

## Loading strategy

- **Metadata scan** — reads first 64KB (title, first user line, started) + last 16KB (last_event timestamp) without a full parse. Runs for all sessions at startup.
- **Full load** — parses entire file; populates `events`, `usage_totals`, `tool_use_index`, `tool_result_index`. Triggered on first session select.
- **Tail load** — seeks to `byte_offset`, reads only newly appended bytes. Triggered on FS modify events for already-loaded sessions.

## Liveness

Sessions with activity within `LIVE_THRESHOLD_SECS` (300s) are "live". The sidebar splits live and closed sessions; the closed section is collapsible. The header badge uses green (<30s) / yellow (<5min) / gray.

## CLI flags

| Flag | Default | Description |
|---|---|---|
| `--projects-dir` | `~/.claude/projects` | Claude projects directory |
| `--session` | — | Preselect session by UUID or ≥4-char prefix |
| `--no-follow` | false | Start with follow disabled |
| `--debug` | false | Print FS watch events to stderr |
| `--dump` | false | Headless summary mode, then exit |

## Version

0.0.1 — 2026-05-23
