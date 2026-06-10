use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};

use crate::data::{Project, Session, SourceKind};
use crate::sources::Source;

/// Fallback "live" window for sessions we never observed a driving process for.
/// Only used when neither `process_open` nor `process_ever_open` apply.
pub const LIVE_THRESHOLD_SECS: i64 = 300;

pub fn is_session_live(s: &Session) -> bool {
    // 1. Definitive: user typed `/exit` (or `/quit`).
    if s.exit_observed {
        return false;
    }
    // 2. A driving process is here AND we believe it's driving this specific
    // session (i.e., this session is among the N most-recently-modified in a
    // project with N driving processes).
    if s.process_open {
        return true;
    }
    // 3. A process is running in this project but is driving a different session.
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

pub struct Store {
    /// Data sources, vec-shaped so a second source is purely additive (PRD-07).
    /// Currently only ever holds a single `ClaudeSource`.
    sources: Vec<Arc<dyn Source>>,
    /// Project slug -> Project.
    pub projects: BTreeMap<String, Project>,
    /// Session id -> Session.
    pub sessions: HashMap<String, Session>,
}

impl Store {
    pub fn new(sources: Vec<Arc<dyn Source>>) -> Self {
        Self {
            sources,
            projects: BTreeMap::new(),
            sessions: HashMap::new(),
        }
    }

    /// Look up the source backing a given kind (clones the cheap `Arc` so the
    /// caller can hold it while mutating `self.sessions`).
    fn source_for(&self, kind: SourceKind) -> Option<Arc<dyn Source>> {
        self.sources.iter().find(|s| s.kind() == kind).cloned()
    }

    /// Find the source that owns a raw FS path, plus the affected session id.
    fn owner_of_path(&self, path: &Path) -> Option<(Arc<dyn Source>, String)> {
        for src in &self.sources {
            if let Some(sid) = src.session_id_for_path(path) {
                return Some((src.clone(), sid));
            }
        }
        None
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
        let scanned: Vec<(Project, Vec<Session>)> =
            self.sources.iter().flat_map(|src| src.scan()).collect();
        for (project, sessions) in scanned {
            self.ingest(project, sessions);
        }
        Ok(())
    }

    /// Insert a project + its sessions into the maps, then sort the project's
    /// session list by most-recent activity (desc). The source leaves
    /// `project.sessions` empty; ordering is a store concern.
    fn ingest(&mut self, mut project: Project, sessions: Vec<Session>) {
        let slug = project.slug.clone();
        for s in sessions {
            if !project.sessions.contains(&s.id) {
                project.sessions.push(s.id.clone());
            }
            self.sessions.insert(s.id.clone(), s);
        }
        self.projects.insert(slug.clone(), project);
        self.re_sort_project(&slug);
    }

    pub fn ensure_loaded(&mut self, session_id: &str) -> Result<()> {
        let kind = match self.sessions.get(session_id) {
            Some(s) if !s.loaded => s.source,
            _ => return Ok(()),
        };
        let Some(src) = self.source_for(kind) else {
            return Ok(());
        };
        let s = self.sessions.get_mut(session_id).unwrap();
        src.load_session(s)
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
        let Some((src, sid)) = self.owner_of_path(path) else {
            return;
        };
        // Refresh mtime (source-agnostic: it is just the file's mtime).
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
        let slug = s.project_slug.clone();
        let _ = src.refresh_session(s);
        self.re_sort_project(&slug);
    }

    fn on_created(&mut self, path: &Path) {
        if path.is_dir() {
            for src in self.sources.clone() {
                if let Some((project, sessions)) = src.scan_project(path) {
                    self.ingest(project, sessions);
                    return;
                }
            }
            return;
        }
        let Some((src, sid)) = self.owner_of_path(path) else {
            return;
        };
        if self.sessions.contains_key(&sid) {
            return;
        }
        let Some((project, session)) = src.discover_session(path) else {
            return;
        };
        let slug = session.project_slug.clone();
        let sid = session.id.clone();
        self.sessions.insert(sid.clone(), session);
        let proj = self.projects.entry(slug.clone()).or_insert(project);
        if !proj.sessions.contains(&sid) {
            proj.sessions.push(sid);
        }
        self.re_sort_project(&slug);
    }

    fn on_removed(&mut self, path: &Path) {
        let Some((_src, sid)) = self.owner_of_path(path) else {
            return;
        };
        let slug = self.sessions.get(&sid).map(|s| s.project_slug.clone());
        self.sessions.remove(&sid);
        if let Some(slug) = slug {
            if let Some(p) = self.projects.get_mut(&slug) {
                p.sessions.retain(|s| s != &sid);
            }
        }
    }

    fn re_sort_project(&mut self, slug: &str) {
        type TimePair = (Option<DateTime<Utc>>, Option<DateTime<Utc>>);
        let sessions_clone: HashMap<String, TimePair> = self
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
    /// `active` is per-source: each entry is a source kind plus the working
    /// directories of that source's running processes (one entry per process,
    /// duplicates preserved). A source's processes don't hold their session
    /// file open, so we can't tell *which* session in a multi-session project
    /// is being driven. Instead, for each project with N running processes, we
    /// mark the N most-recently-active sessions as `process_open = true`.
    /// Sessions whose file hasn't been touched recently won't be misclassified
    /// as live just because some unrelated process is running in the same dir.
    ///
    /// The N-most-recent attribution is source-agnostic and lives here; only
    /// the CWD→slug encoding is source-specific and is delegated to the source.
    pub fn apply_open_files(&mut self, active: &[(SourceKind, Vec<PathBuf>)]) {
        let now = Utc::now();

        // Count running processes per (source, project slug).
        let mut counts: HashMap<(SourceKind, String), usize> = HashMap::new();
        for (kind, dirs) in active {
            let Some(src) = self.source_for(*kind) else {
                continue;
            };
            for dir in dirs {
                if let Some(slug) = src.slug_for_cwd(dir) {
                    *counts.entry((*kind, slug)).or_insert(0) += 1;
                }
            }
        }

        // For each (source, slug) with N processes, pick the N most-recent sessions.
        let mut active_sessions: HashSet<String> = HashSet::new();
        for ((kind, slug), n) in &counts {
            let mut ids: Vec<(String, Option<DateTime<Utc>>)> = self
                .sessions
                .iter()
                .filter(|(_, s)| s.source == *kind && s.project_slug == *slug)
                .map(|(id, s)| (id.clone(), s.last_event.or(s.last_mtime)))
                .collect();
            ids.sort_by_key(|b| Reverse(b.1));
            for (id, _) in ids.into_iter().take(*n) {
                active_sessions.insert(id);
            }
        }

        let active_slugs: HashSet<(SourceKind, String)> = counts.into_keys().collect();
        for (id, s) in self.sessions.iter_mut() {
            let was_open = s.process_open;
            let now_open = active_sessions.contains(id);
            s.process_open = now_open;
            s.project_has_claude = active_slugs.contains(&(s.source, s.project_slug.clone()));
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

#[derive(Debug, Clone)]
pub enum FsEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Removed(PathBuf),
}
