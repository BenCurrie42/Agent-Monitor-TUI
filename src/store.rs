use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};

use crate::data::{
    parse_line, AssistantBlock, Event, EventRecord, Project, Session, UserContent,
};

/// Fallback "live" window for sessions we never observed a claude process for.
/// Only used when neither `process_open` nor `process_ever_open` apply.
pub const LIVE_THRESHOLD_SECS: i64 = 300;

pub fn is_session_live(s: &Session) -> bool {
    // 1. Definitive: user typed `/exit` (or `/quit`).
    if s.exit_observed {
        return false;
    }
    // 2. A claude process is here AND we believe it's driving this specific
    // session (i.e., this session is among the N most-recently-modified in a
    // project with N claude processes).
    if s.process_open {
        return true;
    }
    // 3. Claude is running in this project but is driving a different session.
    // Don't let the timestamp fallback pretend this stale sibling is live just
    // because its file mtime is recent.
    if s.project_has_claude {
        return false;
    }
    // 4. We observed a process here before, and it's gone now.
    if s.process_ever_open {
        return false;
    }
    // 5. No process info for this project — fall back to timestamp heuristic.
    let Some(t) = s.last_event.or(s.last_mtime) else {
        return false;
    };
    Utc::now().signed_duration_since(t).num_seconds() < LIVE_THRESHOLD_SECS
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

const HEAD_BYTES: u64 = 64 * 1024;
const TAIL_BYTES: u64 = 16 * 1024;

pub struct Store {
    pub projects_dir: PathBuf,
    /// Project slug -> Project.
    pub projects: BTreeMap<String, Project>,
    /// Session id -> Session.
    pub sessions: HashMap<String, Session>,
}

impl Store {
    pub fn new(projects_dir: PathBuf) -> Self {
        Self {
            projects_dir,
            projects: BTreeMap::new(),
            sessions: HashMap::new(),
        }
    }

    /// Project slugs sorted by most-recent session activity (desc). Projects
    /// with no sessions sort last.
    pub fn project_order_by_recency(&self) -> Vec<String> {
        let mut slugs: Vec<String> = self.projects.keys().cloned().collect();
        slugs.sort_by(|a, b| {
            let ta = self.most_recent_activity(a);
            let tb = self.most_recent_activity(b);
            tb.cmp(&ta).then_with(|| a.cmp(b))
        });
        slugs
    }

    fn most_recent_activity(&self, slug: &str) -> Option<DateTime<Utc>> {
        let proj = self.projects.get(slug)?;
        proj.sessions
            .iter()
            .filter_map(|sid| self.sessions.get(sid))
            .filter_map(|s| s.last_event.or(s.last_mtime))
            .max()
    }

    /// The single most-recently-active session globally (or None if empty).
    pub fn most_recent_session(&self) -> Option<&Session> {
        self.sessions
            .values()
            .filter(|s| s.last_event.is_some() || s.last_mtime.is_some())
            .max_by_key(|s| s.last_event.or(s.last_mtime))
    }

    pub fn initial_scan(&mut self) -> Result<()> {
        let entries = fs::read_dir(&self.projects_dir)
            .with_context(|| format!("reading {}", self.projects_dir.display()))?;
        for ent in entries.flatten() {
            let path = ent.path();
            if !path.is_dir() {
                continue;
            }
            let Some(slug) = path.file_name().and_then(|s| s.to_str()).map(String::from) else {
                continue;
            };
            self.scan_project_dir(&slug, &path);
        }
        Ok(())
    }

    fn scan_project_dir(&mut self, slug: &str, path: &Path) {
        let mut project = Project::new(slug.to_string(), path.to_path_buf());
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for ent in entries.flatten() {
            let p = ent.path();
            if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()).map(String::from) else {
                continue;
            };
            let mut session = Session::new(stem.clone(), slug.to_string(), p.clone());
            // mtime
            if let Ok(meta) = ent.metadata() {
                if let Ok(modified) = meta.modified() {
                    if let Ok(d) = modified.duration_since(std::time::UNIX_EPOCH) {
                        session.last_mtime = Utc.timestamp_opt(d.as_secs() as i64, 0).single();
                    }
                }
            }
            metadata_scan_session(&mut session);
            project.sessions.push(stem.clone());
            self.sessions.insert(stem, session);
        }
        // Sort sessions by last activity desc (mtime if no last_event yet).
        let me = &self.sessions;
        project.sessions.sort_by(|a, b| {
            let ta = me
                .get(a)
                .and_then(|s| s.last_event.or(s.last_mtime))
                .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap());
            let tb = me
                .get(b)
                .and_then(|s| s.last_event.or(s.last_mtime))
                .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap());
            tb.cmp(&ta)
        });
        self.projects.insert(slug.to_string(), project);
    }

    pub fn ensure_loaded(&mut self, session_id: &str) -> Result<()> {
        if let Some(s) = self.sessions.get(session_id) {
            if s.loaded {
                return Ok(());
            }
        } else {
            return Ok(());
        }
        let s = self.sessions.get_mut(session_id).unwrap();
        full_load_session(s)?;
        Ok(())
    }

    /// Delete all closed (non-live) sessions from disk and remove them from the store.
    /// Returns the number of sessions deleted.
    pub fn delete_closed_sessions(&mut self) -> usize {
        let closed_ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| !is_session_live(s))
            .map(|(id, _)| id.clone())
            .collect();
        let count = closed_ids.len();
        for sid in &closed_ids {
            if let Some(s) = self.sessions.get(sid) {
                let _ = std::fs::remove_file(&s.file);
            }
            self.sessions.remove(sid);
        }
        for project in self.projects.values_mut() {
            project.sessions.retain(|sid| !closed_ids.contains(sid));
        }
        self.projects.retain(|_, p| !p.sessions.is_empty());
        count
    }

    pub fn apply_fs_event(&mut self, ev: FsEvent, _debug: bool) {
        match ev {
            FsEvent::Modified(path) => self.on_modified(&path),
            FsEvent::Created(path) => self.on_created(&path),
            FsEvent::Removed(path) => self.on_removed(&path),
        }
    }

    fn on_modified(&mut self, path: &Path) {
        let Some((slug, sid)) = jsonl_ids_for(path) else {
            return;
        };
        // Refresh mtime
        if let Ok(meta) = fs::metadata(path) {
            if let Ok(modified) = meta.modified() {
                if let Ok(d) = modified.duration_since(std::time::UNIX_EPOCH) {
                    if let Some(s) = self.sessions.get_mut(&sid) {
                        s.last_mtime = Utc.timestamp_opt(d.as_secs() as i64, 0).single();
                    }
                }
            }
        }
        if !self.sessions.contains_key(&sid) {
            // New session that appeared via modify (e.g., from rapid creation+append).
            self.on_created(path);
            return;
        }
        let Some(s) = self.sessions.get_mut(&sid) else {
            return;
        };
        if !s.loaded {
            // Refresh metadata only.
            metadata_scan_session(s);
            self.re_sort_project(&slug);
            return;
        }
        let _ = tail_load_session(s);
        self.re_sort_project(&slug);
    }

    fn on_created(&mut self, path: &Path) {
        if path.is_dir() {
            let Some(slug) = path.file_name().and_then(|s| s.to_str()).map(String::from) else {
                return;
            };
            self.scan_project_dir(&slug, path);
            return;
        }
        let Some((slug, sid)) = jsonl_ids_for(path) else {
            return;
        };
        if self.sessions.contains_key(&sid) {
            return;
        }
        let mut session = Session::new(sid.clone(), slug.clone(), path.to_path_buf());
        if let Ok(meta) = fs::metadata(path) {
            if let Ok(modified) = meta.modified() {
                if let Ok(d) = modified.duration_since(std::time::UNIX_EPOCH) {
                    session.last_mtime = Utc.timestamp_opt(d.as_secs() as i64, 0).single();
                }
            }
        }
        metadata_scan_session(&mut session);
        self.sessions.insert(sid.clone(), session);
        let proj = self
            .projects
            .entry(slug.clone())
            .or_insert_with(|| Project::new(slug.clone(), path.parent().unwrap().to_path_buf()));
        if !proj.sessions.contains(&sid) {
            proj.sessions.push(sid);
        }
        self.re_sort_project(&slug);
    }

    fn on_removed(&mut self, path: &Path) {
        let Some((slug, sid)) = jsonl_ids_for(path) else {
            return;
        };
        self.sessions.remove(&sid);
        if let Some(p) = self.projects.get_mut(&slug) {
            p.sessions.retain(|s| s != &sid);
        }
    }

    fn re_sort_project(&mut self, slug: &str) {
        let sessions_clone: HashMap<String, (Option<DateTime<Utc>>, Option<DateTime<Utc>>)> = self
            .sessions
            .iter()
            .map(|(k, v)| (k.clone(), (v.last_event, v.last_mtime)))
            .collect();
        if let Some(p) = self.projects.get_mut(slug) {
            p.sessions.sort_by(|a, b| {
                let ta = sessions_clone
                    .get(a)
                    .and_then(|(e, m)| e.or(*m))
                    .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap());
                let tb = sessions_clone
                    .get(b)
                    .and_then(|(e, m)| e.or(*m))
                    .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap());
                tb.cmp(&ta)
            });
        }
    }

    /// Update `process_open` for all sessions.
    ///
    /// `active_dirs` is the list of working-directory paths of running claude
    /// processes — one entry per process, with duplicates if multiple claude
    /// processes share a CWD. `claude` does not hold its JSONL file open, so
    /// lsof can't tell us *which* session in a multi-session project is the
    /// one being driven. Instead, for each project with N running claude
    /// processes, we mark the N most-recently-active sessions as
    /// `process_open = true`. Sessions whose JSONL hasn't been touched
    /// recently won't be misclassified as live just because some unrelated
    /// claude is running in the same directory.
    pub fn apply_open_files(&mut self, active_dirs: &[std::path::PathBuf]) {
        let now = Utc::now();

        let mut counts: HashMap<String, usize> = HashMap::new();
        for p in active_dirs {
            if let Some(s) = p.to_str() {
                *counts.entry(cwd_to_slug(s)).or_insert(0) += 1;
            }
        }

        let mut active_sessions: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for (slug, n) in &counts {
            let mut ids: Vec<(String, Option<DateTime<Utc>>)> = self
                .sessions
                .iter()
                .filter(|(_, s)| s.project_slug == *slug)
                .map(|(id, s)| (id.clone(), s.last_event.or(s.last_mtime)))
                .collect();
            ids.sort_by(|a, b| b.1.cmp(&a.1));
            for (id, _) in ids.into_iter().take(*n) {
                active_sessions.insert(id);
            }
        }

        for (id, s) in self.sessions.iter_mut() {
            let was_open = s.process_open;
            let now_open = active_sessions.contains(id);
            s.process_open = now_open;
            s.project_has_claude = counts.contains_key(&s.project_slug);
            if now_open {
                s.process_ever_open = true;
                s.process_closed_at = None;
            } else if was_open {
                s.process_closed_at = Some(now);
            }
        }
    }

    /// Re-read the original JSON line for an event by its byte offset.
    pub fn raw_line_for(&self, session_id: &str, offset: u64, len: u64) -> Option<String> {
        let s = self.sessions.get(session_id)?;
        let mut f = File::open(&s.file).ok()?;
        f.seek(SeekFrom::Start(offset)).ok()?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf).ok()?;
        String::from_utf8(buf).ok()
    }
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

#[derive(Debug, Clone)]
pub enum FsEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Removed(PathBuf),
}

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
            }
        }
    } else {
        // Whole file is in head; pick last_event from head pass.
        let mut last_ts: Option<DateTime<Utc>> = None;
        for line in head_str.lines() {
            if let Some(rec) = parse_line(line, 0) {
                if let Some(ts) = rec.timestamp {
                    last_ts = Some(ts);
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
    let file = File::open(&session.file)
        .with_context(|| format!("opening {}", session.file.display()))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut offset: u64 = 0;
    session.usage_totals = Default::default();
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
    let mut file = File::open(&session.file)
        .with_context(|| format!("opening {}", session.file.display()))?;
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
        Event::User(UserContent::Text(s))
            if session.first_user_line.is_none() && !s.trim().is_empty() =>
        {
            let cleaned = first_line(s, 80);
            if !cleaned.is_empty() {
                session.first_user_line = Some(cleaned);
            }
        }
        Event::Assistant { usage, .. } => {
            if let Some(u) = usage {
                let any_nonzero = u.input_tokens.unwrap_or(0) > 0
                    || u.output_tokens.unwrap_or(0) > 0
                    || u.cache_creation_input_tokens.unwrap_or(0) > 0
                    || u.cache_read_input_tokens.unwrap_or(0) > 0;
                if any_nonzero {
                    session.usage_totals.add(u, rec.model.as_deref());
                }
            }
        }
        _ => {}
    }
    // Suppress unused warning on AssistantBlock import path.
    let _ = std::any::type_name::<AssistantBlock>();
}
