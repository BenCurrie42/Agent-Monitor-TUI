# agent-monitor

A lazydocker-style TUI for watching Claude Code sessions as they happen.

Reads the JSONL files Claude writes to `~/.claude/projects`, tails them in real time, and gives you a keyboard-driven way to browse sessions, inspect individual events, and see how many tokens you're burning.

![status: early](https://img.shields.io/badge/status-early-orange)

## Install

**From crates.io** (requires Rust):
```bash
cargo install agent-monitor-tui
```

**Pre-built binary** (no Rust required):
Download the binary for your platform from [GitHub Releases](https://github.com/BenCurrie42/Agent-Monitor-TUI/releases), `chmod +x`, and move it to somewhere on your `PATH`.

**From source:**
```bash
cargo install --path .
```

**Nix dev shell:**
```bash
nix develop
```

## Usage

```
agentmonitor [--projects-dir <PATH>] [--session <UUID>] [--no-follow] [--debug] [--dump]
```

| Flag | Description |
|---|---|
| `--projects-dir` | Use a different projects directory (default: `~/.claude/projects`) |
| `--session` | Jump straight to a session by UUID or unique prefix |
| `--no-follow` | Don't auto-scroll to the latest event on launch |
| `--debug` | Print filesystem watch events to stderr |
| `--dump` | Print a summary of all sessions and exit (no TUI) |

## Keys

```
j / k          navigate up/down
h / l          collapse/expand project
Tab            switch focus between sidebar and event stream
Enter          open detail view for selected event
/              filter events by text
f              toggle follow (auto-scroll)
v              toggle meta event visibility
g / G          jump to top / bottom
D              delete all closed sessions (confirmation prompt)
?              show help overlay
Esc            clear filter
q              quit

In detail view:
  j / k        scroll
  d / u        page down / up
  R            toggle raw JSON
  Esc          close
```

## How it works

Claude Code writes every conversation event to a JSONL file under `~/.claude/projects/<slug>/<session-id>.jsonl`. This tool watches that directory for changes, parses the JSONL on the fly, and streams events into the UI as they arrive.

Sessions are split into **live** (activity in the last 5 minutes, or JSONL file currently held open by a `claude` process), **sub-agents** (background sessions with no user-visible title), and **closed** sections. Token usage and estimated cost (Opus / Sonnet / Haiku) are tracked per session.

## License

MIT
