//! Claude Code data source.
//!
//! Reads `~/.claude/projects/<slug>/<session>.jsonl`: one JSONL file per
//! session under a per-project directory whose name is the project's CWD with
//! `/` and `.` replaced by `-`. Liveness comes from `lsof -c claude -d cwd`.
//!
//! All of this is Claude-specific and lives here so `Store` can stay generic.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};

use super::Source;
use crate::data::{
    parse_line, AssistantBlock, Event, EventRecord, Project, Session, SourceKind, UserContent,
};

const HEAD_BYTES: u64 = 64 * 1024;
const TAIL_BYTES: u64 = 16 * 1024;

/// The Claude Code source rooted at a projects directory (default
/// `~/.claude/projects`).
pub struct ClaudeSource {
    root: PathBuf,
}

impl ClaudeSource {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Scan one project directory into a `Project` + its sessions. Shared by
    /// `scan` (all dirs) and `scan_project` (one created dir).
    fn scan_dir(&self, slug: &str, path: &Path) -> (Project, Vec<Session>) {
        let project = Project::new(slug.to_string(), path.to_path_buf());
        let mut sessions = Vec::new();
        let Ok(entries) = fs::read_dir(path) else {
            return (project, sessions);
        };
        for ent in entries.flatten() {
            let p = ent.path();
            if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()).map(String::from) else {
                continue;
            };
            let mut session = Session::new(stem, slug.to_string(), p.clone());
            if let Ok(meta) = ent.metadata() {
                session.last_mtime = mtime_to_utc(&meta);
            }
            metadata_scan_session(&mut session);
            sessions.push(session);
        }
        (project, sessions)
    }
}

impl Source for ClaudeSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Claude
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn scan(&self) -> Vec<(Project, Vec<Session>)> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for ent in entries.flatten() {
            let path = ent.path();
            if !path.is_dir() {
                continue;
            }
            let Some(slug) = path.file_name().and_then(|s| s.to_str()).map(String::from) else {
                continue;
            };
            out.push(self.scan_dir(&slug, &path));
        }
        out
    }

    fn scan_project(&self, dir: &Path) -> Option<(Project, Vec<Session>)> {
        let slug = dir.file_name().and_then(|s| s.to_str())?.to_string();
        Some(self.scan_dir(&slug, dir))
    }

    fn discover_session(&self, file: &Path) -> Option<(Project, Session)> {
        let (slug, stem) = jsonl_ids_for(file)?;
        let parent = file.parent()?.to_path_buf();
        let project = Project::new(slug.clone(), parent);
        let mut session = Session::new(stem, slug, file.to_path_buf());
        if let Ok(meta) = fs::metadata(file) {
            session.last_mtime = mtime_to_utc(&meta);
        }
        metadata_scan_session(&mut session);
        Some((project, session))
    }

    fn load_session(&self, session: &mut Session) -> Result<()> {
        full_load_session(session)
    }

    fn refresh_session(&self, session: &mut Session) -> Result<()> {
        if session.loaded {
            tail_load_session(session)
        } else {
            // Not yet fully loaded: refresh the cheap metadata scan only.
            metadata_scan_session(session);
            Ok(())
        }
    }

    fn session_id_for_path(&self, path: &Path) -> Option<String> {
        jsonl_ids_for(path).map(|(_slug, stem)| stem)
    }

    fn active_dirs(&self) -> Vec<PathBuf> {
        claude_open_files()
    }

    fn slug_for_cwd(&self, cwd: &Path) -> Option<String> {
        cwd.to_str().map(cwd_to_slug)
    }
}

/// Convert a file's modified time to a UTC timestamp (second precision).
fn mtime_to_utc(meta: &fs::Metadata) -> Option<DateTime<Utc>> {
    let modified = meta.modified().ok()?;
    let d = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Utc.timestamp_opt(d.as_secs() as i64, 0).single()
}

/// True if a user-text event represents an `/exit` or `/quit` slash command.
/// Claude Code wraps slash commands as `<command-name>/exit</command-name>` in
/// the user-content stream, so we detect that wrapper directly.
fn is_exit_command(text: &str) -> bool {
    let t = text.trim();
    t == "<command-name>/exit</command-name>" || t == "<command-name>/quit</command-name>"
}

/// Re-encode an absolute path as the slug Claude Code would use for it:
/// replace every `/` and `.` with `-`. This is the same encoding Claude Code
/// applies when naming the project directory under `~/.claude/projects/`.
fn cwd_to_slug(path: &str) -> String {
    path.chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect()
}

/// Split a Claude JSONL path into `(project_slug, session_id)`.
fn jsonl_ids_for(path: &Path) -> Option<(String, String)> {
    if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
        return None;
    }
    let stem = path.file_stem().and_then(|s| s.to_str())?.to_string();
    let slug = path
        .parent()?
        .file_name()
        .and_then(|s| s.to_str())?
        .to_string();
    Some((slug, stem))
}

fn extract_cwd(line: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct CwdOnly {
        cwd: Option<String>,
    }
    serde_json::from_str::<CwdOnly>(line)
        .ok()
        .and_then(|r| r.cwd)
}

fn index_tools(session: &mut Session, event_idx: usize, rec: &EventRecord) {
    match &rec.event {
        Event::Assistant { blocks, .. } => {
            for (bi, b) in blocks.iter().enumerate() {
                if let AssistantBlock::ToolUse { id, .. } = b {
                    if !id.is_empty() {
                        session.tool_use_index.insert(id.clone(), (event_idx, bi));
                    }
                }
            }
        }
        Event::User(UserContent::ToolResults(rs)) => {
            for (ri, r) in rs.iter().enumerate() {
                if let Some(id) = &r.tool_use_id {
                    session
                        .tool_result_index
                        .insert(id.clone(), (event_idx, ri));
                }
            }
        }
        _ => {}
    }
}

fn metadata_scan_session(session: &mut Session) {
    let path = &session.file;
    let Ok(file) = File::open(path) else { return };
    let meta = match file.metadata() {
        Ok(m) => m,
        Err(_) => return,
    };
    let size = meta.len();

    // Head pass: parse the first ~HEAD_BYTES, extract title/first user line/started.
    let head_bytes = std::cmp::min(size, HEAD_BYTES);
    let mut head_buf = vec![0u8; head_bytes as usize];
    {
        let mut f = match File::open(path) {
            Ok(f) => f,
            Err(_) => return,
        };
        if f.read_exact(&mut head_buf).is_err() {
            // Best-effort, partial is fine.
            head_buf.truncate(head_bytes as usize);
        }
    }
    // Drop trailing partial line.
    let head_str = String::from_utf8_lossy(&head_buf);
    let mut found_user = false;
    for line in head_str.lines() {
        if session.cwd.is_none() {
            if let Some(cwd) = extract_cwd(line) {
                session.cwd = Some(cwd);
            }
        }
        if let Some(rec) = parse_line(line, 0) {
            if session.started.is_none() {
                session.started = rec.timestamp;
            }
            if rec.session_kind.as_deref() == Some("bg") {
                session.is_background = true;
            }
            match &rec.event {
                Event::AiTitle(t) if !t.trim().is_empty() => {
                    session.title = Some(t.clone());
                }
                Event::AgentName(n) if !n.trim().is_empty() => {
                    session.agent_name = Some(n.clone());
                }
                Event::User(UserContent::Text(s)) if !found_user && !s.trim().is_empty() => {
                    let cleaned = first_line(s, 80);
                    if !cleaned.is_empty() {
                        session.first_user_line = Some(cleaned);
                        found_user = true;
                    }
                }
                _ => {}
            }
        }
    }

    // Tail pass: parse the last ~TAIL_BYTES, extract last_event timestamp.
    if size > head_bytes {
        let tail_start = size.saturating_sub(TAIL_BYTES);
        let mut f = match File::open(path) {
            Ok(f) => f,
            Err(_) => return,
        };
        if f.seek(SeekFrom::Start(tail_start)).is_err() {
            return;
        }
        let mut tail_buf = Vec::with_capacity(TAIL_BYTES as usize);
        if f.read_to_end(&mut tail_buf).is_err() {
            return;
        }
        let tail_str = String::from_utf8_lossy(&tail_buf);
        let mut iter = tail_str.lines();
        // Skip first (likely truncated) line if we're not at start.
        if tail_start > 0 {
            iter.next();
        }
        for line in iter {
            if let Some(rec) = parse_line(line, 0) {
                if let Some(ts) = rec.timestamp {
                    session.last_event = Some(ts);
                }
                if let Event::Assistant { usage: Some(u), .. } = &rec.event {
                    let ctx = u.input_tokens.unwrap_or(0)
                        + u.cache_creation_input_tokens.unwrap_or(0)
                        + u.cache_read_input_tokens.unwrap_or(0);
                    if ctx > 0 {
                        session.last_input_tokens = Some(ctx);
                    }
                }
            }
        }
    } else {
        // Whole file is in head; pick last_event and last_input_tokens from head pass.
        let mut last_ts: Option<DateTime<Utc>> = None;
        for line in head_str.lines() {
            if let Some(rec) = parse_line(line, 0) {
                if let Some(ts) = rec.timestamp {
                    last_ts = Some(ts);
                }
                if let Event::Assistant { usage: Some(u), .. } = &rec.event {
                    let ctx = u.input_tokens.unwrap_or(0)
                        + u.cache_creation_input_tokens.unwrap_or(0)
                        + u.cache_read_input_tokens.unwrap_or(0);
                    if ctx > 0 {
                        session.last_input_tokens = Some(ctx);
                    }
                }
            }
        }
        session.last_event = last_ts;
    }
}

fn first_line(s: &str, max: usize) -> String {
    let cleaned = strip_command_envelope(s);
    let line = cleaned.lines().next().unwrap_or("").trim();
    if line.chars().count() <= max {
        line.to_string()
    } else {
        let truncated: String = line.chars().take(max).collect();
        format!("{}…", truncated)
    }
}

/// Strip leading XML-ish envelopes the Claude CLI prepends to user messages
/// for slash commands and local-command output (e.g.
/// `<command-name>...</command-name>`, `<local-command-stdout>...`) so session
/// labels show the actual user content rather than wrapper tags.
fn strip_command_envelope(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        let trimmed = rest.trim_start();
        if let Some(tag_close) = trimmed.strip_prefix('<') {
            // Look for the end of the opening tag.
            if let Some(end) = tag_close.find('>') {
                let tag_name = tag_close[..end]
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_start_matches('/');
                if is_command_envelope_tag(tag_name) {
                    // Skip past this opening tag.
                    let after_open = &tag_close[end + 1..];
                    let close_marker = format!("</{tag_name}>");
                    if let Some(close_pos) = after_open.find(&close_marker) {
                        rest = &after_open[close_pos + close_marker.len()..];
                        continue;
                    } else {
                        // No close tag — drop everything we've seen and emit nothing useful.
                        break;
                    }
                }
            }
        }
        out.push_str(rest);
        break;
    }
    out
}

fn is_command_envelope_tag(tag: &str) -> bool {
    matches!(
        tag,
        "command-name"
            | "command-message"
            | "command-args"
            | "local-command-stdout"
            | "local-command-stderr"
            | "local-command-caveat"
    )
}

fn full_load_session(session: &mut Session) -> Result<()> {
    let file =
        File::open(&session.file).with_context(|| format!("opening {}", session.file.display()))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut offset: u64 = 0;
    session.usage_totals = Default::default();
    session.last_input_tokens = None;
    session.tool_use_index.clear();
    session.tool_result_index.clear();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let len = line.len() as u64;
        if let Some(rec) = parse_line(&line, offset) {
            let event_idx = events.len();
            index_tools(session, event_idx, &rec);
            apply_event_side_effects(session, &rec);
            events.push(rec);
        }
        offset += len + 1; // +1 for \n consumed by lines()
    }
    session.events = events;
    session.byte_offset = size;
    session.loaded = true;
    Ok(())
}

fn tail_load_session(session: &mut Session) -> Result<()> {
    let mut file =
        File::open(&session.file).with_context(|| format!("opening {}", session.file.display()))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    if size <= session.byte_offset {
        return Ok(());
    }
    let to_read = std::cmp::min(size - session.byte_offset, 1024 * 1024);
    file.seek(SeekFrom::Start(session.byte_offset))?;
    let mut buf = vec![0u8; to_read as usize];
    file.read_exact(&mut buf)?;
    // Find last newline; ignore trailing partial line and rewind offset to keep it for next tick.
    let last_nl = buf.iter().rposition(|&b| b == b'\n');
    let consumed = match last_nl {
        Some(i) => i + 1,
        None => 0, // No complete line yet; do not advance.
    };
    let usable = &buf[..consumed];
    let chunk = String::from_utf8_lossy(usable);
    let mut local_offset = session.byte_offset;
    for line in chunk.split('\n') {
        if line.is_empty() {
            local_offset += 1;
            continue;
        }
        let len = line.len() as u64;
        if let Some(rec) = parse_line(line, local_offset) {
            let event_idx = session.events.len();
            index_tools(session, event_idx, &rec);
            apply_event_side_effects(session, &rec);
            session.events.push(rec);
        }
        local_offset += len + 1;
    }
    session.byte_offset += consumed as u64;
    Ok(())
}

fn apply_event_side_effects(session: &mut Session, rec: &EventRecord) {
    if let Some(ts) = rec.timestamp {
        session.last_event = Some(ts);
        if session.started.is_none() {
            session.started = Some(ts);
        }
    }
    if rec.is_sidechain {
        session.sidechain_event_count += 1;
    }
    if rec.session_kind.as_deref() == Some("bg") {
        session.is_background = true;
    }
    if let Event::User(UserContent::Text(s)) = &rec.event {
        if is_exit_command(s) {
            session.exit_observed = true;
        }
    }
    match &rec.event {
        Event::AiTitle(t) if !t.trim().is_empty() => session.title = Some(t.clone()),
        Event::AgentName(n) if !n.trim().is_empty() => session.agent_name = Some(n.clone()),
        Event::User(UserContent::Text(s))
            if session.first_user_line.is_none() && !s.trim().is_empty() =>
        {
            let cleaned = first_line(s, 80);
            if !cleaned.is_empty() {
                session.first_user_line = Some(cleaned);
            }
        }
        Event::Assistant { usage: Some(u), .. } => {
            let any_nonzero = u.input_tokens.unwrap_or(0) > 0
                || u.output_tokens.unwrap_or(0) > 0
                || u.cache_creation_input_tokens.unwrap_or(0) > 0
                || u.cache_read_input_tokens.unwrap_or(0) > 0;
            if any_nonzero {
                session.usage_totals.add(u, rec.model.as_deref());
                // Full context size = all input-side tokens (most are cache hits/writes).
                let ctx = u.input_tokens.unwrap_or(0)
                    + u.cache_creation_input_tokens.unwrap_or(0)
                    + u.cache_read_input_tokens.unwrap_or(0);
                if ctx > 0 {
                    session.last_input_tokens = Some(ctx);
                }
            }
        }
        _ => {}
    }
}

/// Returns one entry per running `claude` process — each is the process's CWD.
/// Duplicates are preserved so callers can count how many claude processes
/// share a project. The `-a` flag is critical: without it, lsof ORs the
/// `-c` and `-d` filters and returns every process on the system.
fn claude_open_files() -> Vec<PathBuf> {
    let Ok(out) = std::process::Command::new("lsof")
        .args(["-a", "-c", "claude", "-d", "cwd", "-F", "n"])
        .output()
    else {
        return Vec::new();
    };
    let Ok(stdout) = std::str::from_utf8(&out.stdout) else {
        return Vec::new();
    };
    stdout
        .lines()
        .filter_map(|l| l.strip_prefix('n'))
        .filter(|p| !p.is_empty() && *p != "/")
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A self-cleaning temp directory unique to this test binary + a label.
    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "am-claude-src-{}-{}",
                std::process::id(),
                label
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TmpDir(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const FIXTURE: &str = concat!(
        r#"{"type":"user","uuid":"u1","timestamp":"2026-05-22T17:19:35.133Z","cwd":"/Users/x/src/proj","message":{"role":"user","content":"build the thing"}}"#,
        "\n",
        r#"{"type":"assistant","uuid":"a1","timestamp":"2026-05-22T17:19:40.000Z","message":{"model":"claude-sonnet-4-6","content":[{"type":"text","text":"on it"},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}],"usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":50}}}"#,
        "\n",
        r#"{"type":"user","uuid":"u2","timestamp":"2026-05-22T17:19:45.000Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok","is_error":false}]}}"#,
        "\n"
    );

    fn write_fixture(root: &Path) -> (String, String) {
        let slug = "-Users-x-src-proj";
        let sid = "11111111-2222-3333-4444-555555555555";
        let proj_dir = root.join(slug);
        fs::create_dir_all(&proj_dir).unwrap();
        fs::write(proj_dir.join(format!("{sid}.jsonl")), FIXTURE).unwrap();
        (slug.to_string(), sid.to_string())
    }

    #[test]
    fn scan_produces_expected_project_and_session_metadata() {
        let tmp = TmpDir::new("scan");
        let (slug, sid) = write_fixture(tmp.path());

        let src = ClaudeSource::new(tmp.path().to_path_buf());
        assert_eq!(src.kind(), SourceKind::Claude);
        assert_eq!(src.root(), tmp.path());

        let scanned = src.scan();
        assert_eq!(scanned.len(), 1, "one project");
        let (project, sessions) = &scanned[0];
        assert_eq!(project.slug, slug);
        assert_eq!(project.display_path, "/Users/x/src/proj");
        assert_eq!(project.source, SourceKind::Claude);
        // The store owns the per-project session-id list; scan leaves it empty.
        assert!(project.sessions.is_empty());

        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.id, sid);
        assert_eq!(s.project_slug, slug);
        assert_eq!(s.source, SourceKind::Claude);
        // Metadata scan (no full parse) fills title-ish + cwd + timestamps.
        assert_eq!(s.first_user_line.as_deref(), Some("build the thing"));
        assert_eq!(s.cwd.as_deref(), Some("/Users/x/src/proj"));
        assert!(s.started.is_some());
        assert!(s.last_event.is_some());
        // Not loaded yet → no events.
        assert!(!s.loaded);
        assert!(s.events.is_empty());
    }

    #[test]
    fn load_session_parses_events_usage_and_indexes() {
        let tmp = TmpDir::new("load");
        let (_slug, _sid) = write_fixture(tmp.path());

        let src = ClaudeSource::new(tmp.path().to_path_buf());
        let (_project, mut sessions) = src.scan().pop().unwrap();
        let mut session = sessions.pop().unwrap();

        src.load_session(&mut session).unwrap();
        assert!(session.loaded);
        assert_eq!(session.events.len(), 3);
        // Usage came from the single assistant turn.
        assert!(session.usage_totals.has_usage);
        assert_eq!(session.usage_totals.input, 100);
        assert_eq!(session.usage_totals.output, 20);
        assert_eq!(session.usage_totals.cache_read, 50);
        // last_input_tokens = sum of input-side tokens of the last assistant turn.
        assert_eq!(session.last_input_tokens, Some(150));
        // Tool-use / tool-result cross index populated.
        assert!(session.tool_use_index.contains_key("toolu_1"));
        assert!(session.tool_result_index.contains_key("toolu_1"));
        // byte_offset advanced to full size so a later tail load is a no-op.
        assert_eq!(session.byte_offset, FIXTURE.len() as u64);
    }

    #[test]
    fn path_mapping_and_slug_encoding() {
        let tmp = TmpDir::new("paths");
        let (slug, sid) = write_fixture(tmp.path());
        let file = tmp.path().join(&slug).join(format!("{sid}.jsonl"));

        let src = ClaudeSource::new(tmp.path().to_path_buf());
        assert_eq!(
            src.session_id_for_path(&file).as_deref(),
            Some(sid.as_str())
        );
        // Non-jsonl path is not owned.
        assert_eq!(src.session_id_for_path(tmp.path()), None);
        // CWD → slug uses the same `/`+`.` → `-` encoding as Claude Code.
        assert_eq!(
            src.slug_for_cwd(Path::new("/Users/x/src/proj")).as_deref(),
            Some(slug.as_str())
        );

        // discover_session rebuilds the same project + metadata-scanned session.
        let (project, session) = src.discover_session(&file).unwrap();
        assert_eq!(project.slug, slug);
        assert_eq!(session.id, sid);
        assert_eq!(session.first_user_line.as_deref(), Some("build the thing"));
    }

    #[test]
    fn refresh_session_tail_loads_appended_lines() {
        let tmp = TmpDir::new("tail");
        let (slug, sid) = write_fixture(tmp.path());
        let file = tmp.path().join(&slug).join(format!("{sid}.jsonl"));

        let src = ClaudeSource::new(tmp.path().to_path_buf());
        let (_project, mut sessions) = src.scan().pop().unwrap();
        let mut session = sessions.pop().unwrap();
        src.load_session(&mut session).unwrap();
        assert_eq!(session.events.len(), 3);

        // Append a new line and tail-refresh.
        let extra = format!(
            "{}\n",
            r#"{"type":"user","uuid":"u3","timestamp":"2026-05-22T17:20:00.000Z","message":{"role":"user","content":"again"}}"#
        );
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap();
        use std::io::Write;
        f.write_all(extra.as_bytes()).unwrap();
        drop(f);

        src.refresh_session(&mut session).unwrap();
        assert_eq!(
            session.events.len(),
            4,
            "tail load picked up the appended line"
        );
    }
}
