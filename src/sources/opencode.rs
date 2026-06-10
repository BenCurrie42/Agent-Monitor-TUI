//! OpenCode data source.
//!
//! Reads `$XDG_DATA_HOME/opencode/storage` (falling back to
//! `~/.local/share/opencode/storage`). Layout:
//!
//! - `project/<projectID>.json`          — `{ id, worktree, vcs, time:{created,updated} }`
//! - `session/<projectID>/<sessionID>.json` — per-session metadata with `parentID?`
//! - `session/global/<sessionID>.json`   — sessions without a per-project worktree
//!
//! This PRD (03) delivers metadata scanning only. `load_session` and
//! `refresh_session` are stubs that will be filled in by PRD-04.
//!
//! The structs and public items here are wired into `main.rs` in PRD-07;
//! until then the dead_code lint is suppressed to keep the gates clean.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Deserialize;

use super::Source;
use crate::data::{
    index_tools, timestamp_from_millis, AssistantBlock, Event, EventRecord, Project, Session,
    SourceKind, ToolResult, Usage, UserContent,
};
use serde_json::Value;

// ── On-disk JSON shapes ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ProjectJson {
    id: String,
    worktree: Option<String>,
    // vcs and sandboxes are ignored; time is parsed but only the fields we use matter.
    #[allow(dead_code)]
    time: TimeJson,
}

#[derive(Deserialize)]
struct SessionJson {
    id: String,
    /// Stored but not used directly — the project slug comes from the directory
    /// path rather than this field (to avoid a second lookup on every parse).
    #[allow(dead_code)]
    #[serde(rename = "projectID")]
    project_id: String,
    directory: Option<String>,
    title: Option<String>,
    time: TimeJson,
    #[serde(rename = "parentID")]
    parent_id: Option<String>,
}

#[derive(Deserialize)]
struct TimeJson {
    created: i64,
    updated: i64,
}

// ── Message / part on-disk shapes (PRD-04) ───────────────────────────────────

/// A `message/<sessionID>/<messageID>.json` file. Only the fields we map are
/// deserialized; the rest are ignored.
#[derive(Deserialize)]
struct MessageJson {
    role: String,
    time: MsgTime,
    /// Assistant-only.
    #[serde(rename = "modelID")]
    model_id: Option<String>,
    /// Assistant-only.
    #[serde(rename = "providerID")]
    provider_id: Option<String>,
    /// Assistant-only authoritative USD cost (0/absent → fall back to estimate).
    cost: Option<f64>,
    /// Assistant-only token counts.
    tokens: Option<TokensJson>,
}

#[derive(Deserialize)]
struct MsgTime {
    created: i64,
}

#[derive(Deserialize)]
struct TokensJson {
    #[serde(default)]
    input: u64,
    #[serde(default)]
    output: u64,
    #[serde(default)]
    reasoning: u64,
    #[serde(default)]
    cache: CacheJson,
}

#[derive(Deserialize, Default)]
struct CacheJson {
    #[serde(default)]
    read: u64,
    #[serde(default)]
    write: u64,
}

/// A `part/<messageID>/<partID>.json` file.
#[derive(Deserialize)]
struct PartJson {
    #[serde(rename = "type")]
    part_type: String,
    text: Option<String>,
    time: Option<PartTime>,
    // tool fields:
    #[serde(rename = "callID")]
    call_id: Option<String>,
    tool: Option<String>,
    state: Option<Value>,
    // file fields:
    filename: Option<String>,
    url: Option<String>,
    // patch fields:
    files: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct PartTime {
    start: Option<i64>,
}

// ── Source impl ──────────────────────────────────────────────────────────────

/// The OpenCode source rooted at its storage directory (default
/// `~/.local/share/opencode/storage`).
pub struct OpencodeSource {
    root: PathBuf,
}

impl OpencodeSource {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Parse a project JSON file and return a `Project` (with `display_path`
    /// set from `worktree`). Returns `None` if the file can't be read/parsed.
    fn parse_project_file(&self, path: &Path) -> Option<Project> {
        let data = fs::read_to_string(path).ok()?;
        let pj: ProjectJson = serde_json::from_str(&data).ok()?;
        let worktree = pj.worktree.unwrap_or_default();
        // project dir path is the storage/project directory (sibling of session/).
        let proj_dir = path.parent().unwrap_or(&self.root).to_path_buf();
        Some(Project::with_display_path(
            pj.id.clone(),
            proj_dir,
            worktree,
            SourceKind::Opencode,
        ))
    }

    /// Parse a session JSON file and return a `Session`. `project_display_path`
    /// is used as the fallback cwd for the global bucket.
    fn parse_session_file(
        &self,
        path: &Path,
        project_slug: &str,
        project_display_path: &str,
    ) -> Option<Session> {
        let data = fs::read_to_string(path).ok()?;
        let sj: SessionJson = serde_json::from_str(&data).ok()?;
        let mut session = Session::new(sj.id.clone(), project_slug.to_string(), path.to_path_buf());
        session.source = SourceKind::Opencode;
        session.title = sj.title;
        session.started = timestamp_from_millis(sj.time.created);
        session.last_event = timestamp_from_millis(sj.time.updated);
        session.cwd = Some(
            sj.directory
                .unwrap_or_else(|| project_display_path.to_string()),
        );
        session.parent_id = sj.parent_id;
        Some(session)
    }

    /// Scan the `session/<project_id>/` directory for a given project.
    fn scan_sessions_for_project(
        &self,
        project_slug: &str,
        project_display_path: &str,
    ) -> Vec<Session> {
        let session_dir = self.root.join("session").join(project_slug);
        let Ok(entries) = fs::read_dir(&session_dir) else {
            return Vec::new();
        };
        let mut sessions = Vec::new();
        for ent in entries.flatten() {
            let p = ent.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if let Some(session) = self.parse_session_file(&p, project_slug, project_display_path) {
                sessions.push(session);
            }
        }
        sessions
    }

    /// Read + sort the parts for one message. Order by `time.start` when present,
    /// else by part-id (filename). Returns `(part_id, PartJson)` pairs.
    fn load_parts(&self, message_id: &str) -> Vec<(String, PartJson)> {
        let part_dir = self.root.join("part").join(message_id);
        let mut parts: Vec<(String, PartJson)> = Vec::new();
        if let Ok(entries) = fs::read_dir(&part_dir) {
            for ent in entries.flatten() {
                let p = ent.path();
                if p.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                let Some(pid) = p.file_stem().and_then(|s| s.to_str()).map(String::from) else {
                    continue;
                };
                let Ok(data) = fs::read_to_string(&p) else {
                    continue;
                };
                let Ok(pj) = serde_json::from_str::<PartJson>(&data) else {
                    continue;
                };
                parts.push((pid, pj));
            }
        }
        // Sort by time.start (missing sorts last) then by part id for stability.
        parts.sort_by(|(aid, a), (bid, b)| {
            let at = a.time.as_ref().and_then(|t| t.start);
            let bt = b.time.as_ref().and_then(|t| t.start);
            match (at, bt) {
                (Some(x), Some(y)) => x.cmp(&y).then_with(|| aid.cmp(bid)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => aid.cmp(bid),
            }
        });
        parts
    }

    /// Emit the events for one assistant message: an `Event::Assistant` holding
    /// the text/reasoning/tool/patch blocks, then one synthesized
    /// `Event::User(ToolResults)` per `tool` part so the existing tool-result
    /// cross-link UI (which keys results off a user event) lights up unchanged.
    fn emit_assistant(
        &self,
        session: &mut Session,
        mj: &MessageJson,
        parts: &[(String, PartJson)],
        ts: Option<chrono::DateTime<chrono::Utc>>,
        model: Option<String>,
    ) {
        let mut blocks: Vec<AssistantBlock> = Vec::new();
        let mut results: Vec<ToolResult> = Vec::new();

        for (_pid, pj) in parts {
            match pj.part_type.as_str() {
                "text" => {
                    if let Some(t) = &pj.text {
                        blocks.push(AssistantBlock::Text { text: t.clone() });
                    }
                }
                "reasoning" => {
                    if let Some(t) = &pj.text {
                        blocks.push(AssistantBlock::Thinking { text: t.clone() });
                    }
                }
                "tool" => {
                    let call_id = pj.call_id.clone().unwrap_or_default();
                    let name = pj.tool.clone().unwrap_or_else(|| "tool".to_string());
                    let state = pj.state.clone().unwrap_or(Value::Null);
                    let input = state.get("input").cloned().unwrap_or(Value::Null);
                    blocks.push(AssistantBlock::ToolUse {
                        id: call_id.clone(),
                        name,
                        input,
                    });
                    // OpenCode folds the result into the same part — synthesize a
                    // ToolResult keyed by callID.
                    let output = state.get("output");
                    let content = match output {
                        Some(Value::String(s)) => s.clone(),
                        Some(v) => v.to_string(),
                        None => String::new(),
                    };
                    let status = state.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    let is_error = status == "error" || state.get("error").is_some();
                    results.push(ToolResult {
                        tool_use_id: Some(call_id),
                        content,
                        is_error,
                    });
                }
                "patch" => {
                    // Render file edits as a text block rather than dropping them.
                    let files = pj.files.as_ref().map(|f| f.join(", ")).unwrap_or_default();
                    blocks.push(AssistantBlock::Text {
                        text: format!("[patch] {files}"),
                    });
                }
                "file" => {
                    // Attachment — represent as an attachment event below.
                }
                _ => {} // step-start / step-finish / unknown → no visible block
            }
        }

        // File attachments → standalone Attachment events (after the assistant turn).
        let attachments: Vec<Value> = parts
            .iter()
            .filter(|(_, pj)| pj.part_type == "file")
            .map(|(_, pj)| {
                serde_json::json!({
                    "filename": pj.filename,
                    "url": pj.url,
                })
            })
            .collect();

        // Build the Usage struct. Reasoning tokens FOLD INTO output_tokens:
        // providers bill reasoning as output, which keeps the ModelPrice math
        // correct and gives PRD-05 a stable input.
        let usage = mj.tokens.as_ref().map(|t| Usage {
            input_tokens: Some(t.input),
            output_tokens: Some(t.output.saturating_add(t.reasoning)),
            cache_creation_input_tokens: Some(t.cache.write),
            cache_read_input_tokens: Some(t.cache.read),
        });

        // Accumulate usage + cost.
        if let Some(u) = &usage {
            let any_nonzero = u.input_tokens.unwrap_or(0) > 0
                || u.output_tokens.unwrap_or(0) > 0
                || u.cache_creation_input_tokens.unwrap_or(0) > 0
                || u.cache_read_input_tokens.unwrap_or(0) > 0;
            if any_nonzero {
                let cost = mj.cost.unwrap_or(0.0);
                if cost != 0.0 {
                    // Authoritative source cost — preferred over a computed estimate.
                    session.usage_totals.add_tokens(u);
                    session.usage_totals.add_authoritative_cost(cost);
                } else {
                    // Fallback: token-only accumulation; PRD-05 owns model pricing.
                    session.usage_totals.add(u, model.as_deref());
                }
                // last_input_tokens = current context size = input + cache read + write.
                let ctx = u.input_tokens.unwrap_or(0)
                    + u.cache_read_input_tokens.unwrap_or(0)
                    + u.cache_creation_input_tokens.unwrap_or(0);
                if ctx > 0 {
                    session.last_input_tokens = Some(ctx);
                }
            }
        }

        // Push the assistant event (even if blocks are empty, to keep usage tied
        // to a turn) — but only if there is something to show or usage.
        if !blocks.is_empty() || usage.is_some() {
            let rec = EventRecord {
                uuid: None,
                parent_uuid: None,
                is_sidechain: false,
                session_kind: None,
                timestamp: ts,
                event: Event::Assistant { blocks, usage },
                model,
                file_offset: 0,
                file_len: 0,
            };
            let idx = session.events.len();
            index_tools(session, idx, &rec);
            session.events.push(rec);
            if let Some(t) = ts {
                session.last_event = Some(t);
            }
        }

        // Synthesized tool results as a user event so the cross-link UI works.
        if !results.is_empty() {
            let rec = EventRecord {
                uuid: None,
                parent_uuid: None,
                is_sidechain: false,
                session_kind: None,
                timestamp: ts,
                event: Event::User(UserContent::ToolResults(results)),
                model: None,
                file_offset: 0,
                file_len: 0,
            };
            let idx = session.events.len();
            index_tools(session, idx, &rec);
            session.events.push(rec);
        }

        // Attachment events.
        for att in attachments {
            session.events.push(EventRecord {
                uuid: None,
                parent_uuid: None,
                is_sidechain: false,
                session_kind: None,
                timestamp: ts,
                event: Event::Attachment(att),
                model: None,
                file_offset: 0,
                file_len: 0,
            });
        }
    }

    /// Emit events for one user message. The user's text lives in its parts;
    /// concatenate `text` parts into a single `Event::User(Text)`. `file` parts
    /// become attachment events.
    fn emit_user(
        &self,
        session: &mut Session,
        parts: &[(String, PartJson)],
        ts: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        let mut text = String::new();
        for (_pid, pj) in parts {
            match pj.part_type.as_str() {
                "text" => {
                    if let Some(t) = &pj.text {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(t);
                    }
                }
                "file" => {
                    session.events.push(EventRecord {
                        uuid: None,
                        parent_uuid: None,
                        is_sidechain: false,
                        session_kind: None,
                        timestamp: ts,
                        event: Event::Attachment(serde_json::json!({
                            "filename": pj.filename,
                            "url": pj.url,
                        })),
                        model: None,
                        file_offset: 0,
                        file_len: 0,
                    });
                }
                _ => {}
            }
        }
        if !text.is_empty() {
            let rec = EventRecord {
                uuid: None,
                parent_uuid: None,
                is_sidechain: false,
                session_kind: None,
                timestamp: ts,
                event: Event::User(UserContent::Text(text)),
                model: None,
                file_offset: 0,
                file_len: 0,
            };
            session.events.push(rec);
            if let Some(t) = ts {
                session.last_event = Some(t);
            }
        }
    }
}

impl Source for OpencodeSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Opencode
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn scan(&self) -> Vec<(Project, Vec<Session>)> {
        let mut out: Vec<(Project, Vec<Session>)> = Vec::new();

        // 1. Enumerate project/*.json files.
        let project_dir = self.root.join("project");
        if let Ok(entries) = fs::read_dir(&project_dir) {
            for ent in entries.flatten() {
                let p = ent.path();
                if p.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                let Some(project) = self.parse_project_file(&p) else {
                    continue;
                };
                let sessions = self.scan_sessions_for_project(
                    &project.slug.clone(),
                    &project.display_path.clone(),
                );
                out.push((project, sessions));
            }
        }

        // 2. Handle the global bucket: sessions without a per-project worktree.
        let global_dir = self.root.join("session").join("global");
        if global_dir.is_dir() {
            // Build a synthetic project for the global bucket.
            let global_project = Project::with_display_path(
                "global".to_string(),
                global_dir.clone(),
                String::new(), // display_path filled per-session from directory field
                SourceKind::Opencode,
            );
            let sessions = self.scan_sessions_for_project("global", "");
            out.push((global_project, sessions));
        }

        out
    }

    fn scan_project(&self, dir: &Path) -> Option<(Project, Vec<Session>)> {
        // `dir` is expected to be a session/<projectID> directory (or session/global).
        // Walk up to find a matching project/<projectID>.json.
        let project_id = dir.file_name().and_then(|s| s.to_str())?;
        if project_id == "global" {
            let global_project = Project::with_display_path(
                "global".to_string(),
                dir.to_path_buf(),
                String::new(),
                SourceKind::Opencode,
            );
            let sessions = self.scan_sessions_for_project("global", "");
            return Some((global_project, sessions));
        }
        let proj_file = self.root.join("project").join(format!("{project_id}.json"));
        let project = self.parse_project_file(&proj_file)?;
        let sessions = self.scan_sessions_for_project(project_id, &project.display_path.clone());
        Some((project, sessions))
    }

    fn discover_session(&self, file: &Path) -> Option<(Project, Session)> {
        // file must be session/<projectID>/<sessionID>.json
        if file.extension().and_then(|s| s.to_str()) != Some("json") {
            return None;
        }
        let project_id = file.parent()?.file_name().and_then(|s| s.to_str())?;
        let project = if project_id == "global" {
            Project::with_display_path(
                "global".to_string(),
                file.parent()?.to_path_buf(),
                String::new(),
                SourceKind::Opencode,
            )
        } else {
            let proj_file = self.root.join("project").join(format!("{project_id}.json"));
            self.parse_project_file(&proj_file)?
        };
        let session = self.parse_session_file(file, project_id, &project.display_path.clone())?;
        Some((project, session))
    }

    /// Full-load an OpenCode session: read every `message/<sid>/*.json` (ordered
    /// by `time.created`), and for each its `part/<mid>/*.json` (ordered by
    /// `time.start` then filename), mapping them onto the shared `Event` model.
    fn load_session(&self, session: &mut Session) -> Result<()> {
        let sid = &session.id;
        session.events.clear();
        session.usage_totals = Default::default();
        session.last_input_tokens = None;
        session.tool_use_index.clear();
        session.tool_result_index.clear();

        // 1. List + parse messages, then sort by time.created.
        let msg_dir = self.root.join("message").join(sid);
        let mut messages: Vec<(PathBuf, String, MessageJson)> = Vec::new();
        if let Ok(entries) = fs::read_dir(&msg_dir) {
            for ent in entries.flatten() {
                let p = ent.path();
                if p.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                let Some(mid) = p.file_stem().and_then(|s| s.to_str()).map(String::from) else {
                    continue;
                };
                let Ok(data) = fs::read_to_string(&p) else {
                    continue;
                };
                let Ok(mj) = serde_json::from_str::<MessageJson>(&data) else {
                    continue;
                };
                messages.push((p, mid, mj));
            }
        }
        messages.sort_by_key(|(_, _, mj)| mj.time.created);

        // 2. For each message, read its parts and emit events.
        for (_path, mid, mj) in &messages {
            let parts = self.load_parts(mid);
            let ts = timestamp_from_millis(mj.time.created);
            let is_assistant = mj.role == "assistant";
            let model = match (&mj.provider_id, &mj.model_id) {
                (Some(p), Some(m)) => Some(format!("{p}/{m}")),
                _ => None,
            };

            if is_assistant {
                self.emit_assistant(session, mj, &parts, ts, model);
            } else {
                self.emit_user(session, &parts, ts);
            }
        }

        session.loaded = true;
        Ok(())
    }

    /// PRD-04 stub: incremental refresh is not yet implemented for OpenCode.
    fn refresh_session(&self, session: &mut Session) -> Result<()> {
        if !session.loaded {
            // Re-read the session JSON to refresh metadata.
            if let Some(updated) = self.parse_session_file(
                &session.file.clone(),
                &session.project_slug.clone(),
                session.cwd.as_deref().unwrap_or(""),
            ) {
                session.title = updated.title;
                session.started = updated.started;
                session.last_event = updated.last_event;
                session.cwd = updated.cwd;
                session.parent_id = updated.parent_id;
            }
        }
        // TODO(PRD-04): tail-load events for loaded sessions.
        Ok(())
    }

    fn session_id_for_path(&self, path: &Path) -> Option<String> {
        // Must be a .json file under session/<anything>/<sid>.json
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            return None;
        }
        // Check it lives inside our root/session/ subtree.
        let session_root = self.root.join("session");
        if !path.starts_with(&session_root) {
            return None;
        }
        // The session id is the stem of the filename.
        path.file_stem().and_then(|s| s.to_str()).map(String::from)
    }

    /// OpenCode liveness via process detection is out of scope for PRD-03
    /// (deferred to PRD-06). Return empty — the timestamp fallback handles
    /// recency ordering.
    fn active_dirs(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    /// OpenCode doesn't use the Claude slug encoding; return None so the store
    /// won't try to map process CWDs to OpenCode projects via this method.
    fn slug_for_cwd(&self, _cwd: &Path) -> Option<String> {
        None
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{AssistantBlock, Event, UserContent};
    use std::collections::HashSet;

    /// Self-cleaning temp directory.
    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new(label: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("am-oc-src-{}-{}", std::process::id(), label));
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

    // ── Fixture helpers ──────────────────────────────────────────────────────

    const PROJECT_ID: &str = "4f84f334812237ae46e099b31498555ba155b38c";
    const SESSION_ID: &str = "ses_166aa8d54ffewzdqP2F6oNFkqh";
    const SUBAGENT_SESSION_ID: &str = "ses_166aa8112ffe0qSbt0UJBRjNm2";

    const PROJECT_JSON: &str = r#"{
        "id": "4f84f334812237ae46e099b31498555ba155b38c",
        "worktree": "/Users/x/my-project",
        "vcs": "git",
        "time": { "created": 1780688944376, "updated": 1781016881702 }
    }"#;

    const SESSION_JSON: &str = r#"{
        "id": "ses_166aa8d54ffewzdqP2F6oNFkqh",
        "version": "1.1.14",
        "projectID": "4f84f334812237ae46e099b31498555ba155b38c",
        "directory": "/Users/x/my-project",
        "title": "New session - 2026-06-05T19:49:34.507Z",
        "time": { "created": 1780688974507, "updated": 1780689031541 },
        "summary": { "additions": 0, "deletions": 0, "files": 0 }
    }"#;

    const SUBAGENT_SESSION_JSON: &str = r#"{
        "id": "ses_166aa8112ffe0qSbt0UJBRjNm2",
        "version": "1.1.14",
        "projectID": "4f84f334812237ae46e099b31498555ba155b38c",
        "directory": "/Users/x/my-project",
        "parentID": "ses_166aa8d54ffewzdqP2F6oNFkqh",
        "title": "Find PRD bug info (@explore subagent)",
        "time": { "created": 1780690000000, "updated": 1780690500000 },
        "summary": { "additions": 2, "deletions": 0, "files": 1 }
    }"#;

    const GLOBAL_SESSION_JSON: &str = r#"{
        "id": "ses_global001",
        "version": "1.1.14",
        "projectID": "global",
        "directory": "/Users/x/scratch",
        "title": "Global scratch session",
        "time": { "created": 1778645096179, "updated": 1778645220622 },
        "summary": { "additions": 0, "deletions": 0, "files": 0 }
    }"#;

    /// Build a minimal but complete storage fixture tree.
    fn write_fixture(root: &Path) {
        let proj_dir = root.join("project");
        let sess_dir = root.join("session").join(PROJECT_ID);
        let global_dir = root.join("session").join("global");
        fs::create_dir_all(&proj_dir).unwrap();
        fs::create_dir_all(&sess_dir).unwrap();
        fs::create_dir_all(&global_dir).unwrap();

        fs::write(proj_dir.join(format!("{PROJECT_ID}.json")), PROJECT_JSON).unwrap();
        fs::write(sess_dir.join(format!("{SESSION_ID}.json")), SESSION_JSON).unwrap();
        fs::write(
            sess_dir.join(format!("{SUBAGENT_SESSION_ID}.json")),
            SUBAGENT_SESSION_JSON,
        )
        .unwrap();
        fs::write(global_dir.join("ses_global001.json"), GLOBAL_SESSION_JSON).unwrap();
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    #[test]
    fn session_metadata_parsed_correctly() {
        let tmp = TmpDir::new("meta");
        write_fixture(tmp.path());

        let src = OpencodeSource::new(tmp.path().to_path_buf());
        let scanned = src.scan();

        // Should have 2 entries: the real project + the global bucket.
        assert_eq!(scanned.len(), 2, "expected real project + global");

        // Find the non-global project.
        let (project, sessions) = scanned
            .iter()
            .find(|(p, _)| p.slug == PROJECT_ID)
            .expect("project not found");

        assert_eq!(project.slug, PROJECT_ID);
        assert_eq!(project.display_path, "/Users/x/my-project");
        assert_eq!(project.source, SourceKind::Opencode);
        assert!(project.sessions.is_empty(), "store owns the session list");

        // Two sessions in the project dir.
        assert_eq!(sessions.len(), 2);

        let regular = sessions
            .iter()
            .find(|s| s.id == SESSION_ID)
            .expect("regular session");
        assert_eq!(regular.id, SESSION_ID);
        assert_eq!(regular.project_slug, PROJECT_ID);
        assert_eq!(regular.source, SourceKind::Opencode);
        assert_eq!(
            regular.title.as_deref(),
            Some("New session - 2026-06-05T19:49:34.507Z")
        );
        assert_eq!(regular.cwd.as_deref(), Some("/Users/x/my-project"));
        // started from time.created (ms)
        let started = regular.started.expect("started");
        assert_eq!(started.timestamp_millis(), 1_780_688_974_507);
        assert_eq!(started.format("%Y").to_string(), "2026");
        // last_event from time.updated (ms)
        let last = regular.last_event.expect("last_event");
        assert_eq!(last.timestamp_millis(), 1_780_689_031_541);
        // Not a sub-agent.
        assert!(regular.parent_id.is_none());
    }

    #[test]
    fn subagent_session_has_parent_id() {
        let tmp = TmpDir::new("subagent");
        write_fixture(tmp.path());

        let src = OpencodeSource::new(tmp.path().to_path_buf());
        let scanned = src.scan();

        let (_, sessions) = scanned
            .iter()
            .find(|(p, _)| p.slug == PROJECT_ID)
            .expect("project not found");

        let sub = sessions
            .iter()
            .find(|s| s.id == SUBAGENT_SESSION_ID)
            .expect("sub-agent session not found");
        assert_eq!(
            sub.parent_id.as_deref(),
            Some("ses_166aa8d54ffewzdqP2F6oNFkqh")
        );
        assert_eq!(
            sub.title.as_deref(),
            Some("Find PRD bug info (@explore subagent)")
        );
        // is_sub_agent() should return true for this session.
        assert!(crate::app::is_sub_agent_for_test(sub));
    }

    #[test]
    fn global_bucket_sessions_use_directory_as_display_path() {
        let tmp = TmpDir::new("global");
        write_fixture(tmp.path());

        let src = OpencodeSource::new(tmp.path().to_path_buf());
        let scanned = src.scan();

        let (global_proj, global_sessions) = scanned
            .iter()
            .find(|(p, _)| p.slug == "global")
            .expect("global project not found");
        // Global project has no worktree so display_path is empty.
        assert_eq!(global_proj.display_path, "");
        assert_eq!(global_sessions.len(), 1);
        let gsess = &global_sessions[0];
        assert_eq!(gsess.id, "ses_global001");
        assert_eq!(gsess.cwd.as_deref(), Some("/Users/x/scratch"));
        assert_eq!(gsess.source, SourceKind::Opencode);
    }

    #[test]
    fn session_id_for_path_extracts_stem() {
        let tmp = TmpDir::new("idpath");
        write_fixture(tmp.path());
        let src = OpencodeSource::new(tmp.path().to_path_buf());

        let sess_file = tmp
            .path()
            .join("session")
            .join(PROJECT_ID)
            .join(format!("{SESSION_ID}.json"));

        assert_eq!(
            src.session_id_for_path(&sess_file).as_deref(),
            Some(SESSION_ID)
        );
        // Non-json path returns None.
        assert_eq!(src.session_id_for_path(tmp.path()), None);
        // A project JSON (not under session/) returns None.
        let proj_file = tmp
            .path()
            .join("project")
            .join(format!("{PROJECT_ID}.json"));
        assert_eq!(src.session_id_for_path(&proj_file), None);
    }

    #[test]
    fn discover_session_parses_file_directly() {
        let tmp = TmpDir::new("discover");
        write_fixture(tmp.path());
        let src = OpencodeSource::new(tmp.path().to_path_buf());

        let sess_file = tmp
            .path()
            .join("session")
            .join(PROJECT_ID)
            .join(format!("{SESSION_ID}.json"));

        let (project, session) = src.discover_session(&sess_file).expect("should discover");
        assert_eq!(project.slug, PROJECT_ID);
        assert_eq!(session.id, SESSION_ID);
        assert_eq!(session.source, SourceKind::Opencode);
        assert_eq!(
            session.title.as_deref(),
            Some("New session - 2026-06-05T19:49:34.507Z")
        );
    }

    #[test]
    fn scan_empty_root_returns_empty() {
        let tmp = TmpDir::new("empty");
        // No project/ or session/ dirs.
        let src = OpencodeSource::new(tmp.path().to_path_buf());
        assert!(src.scan().is_empty());
    }

    #[test]
    fn source_kind_and_root() {
        let tmp = TmpDir::new("kind");
        let src = OpencodeSource::new(tmp.path().to_path_buf());
        assert_eq!(src.kind(), SourceKind::Opencode);
        assert_eq!(src.root(), tmp.path());
    }

    #[test]
    fn slug_for_cwd_returns_none() {
        let tmp = TmpDir::new("slug");
        let src = OpencodeSource::new(tmp.path().to_path_buf());
        assert_eq!(src.slug_for_cwd(Path::new("/any/path")), None);
    }

    #[test]
    fn active_dirs_returns_empty() {
        let tmp = TmpDir::new("active");
        let src = OpencodeSource::new(tmp.path().to_path_buf());
        assert!(src.active_dirs().is_empty());
    }

    /// Verify the epoch-ms to DateTime round-trip via the public helper.
    #[test]
    fn timestamp_from_millis_roundtrip() {
        // Values from the fixture.
        let created = crate::data::timestamp_from_millis(1_780_688_974_507).unwrap();
        let updated = crate::data::timestamp_from_millis(1_780_689_031_541).unwrap();
        assert!(created < updated);
        assert_eq!(created.format("%Y").to_string(), "2026");
    }

    /// The `is_sub_agent` logic is tested indirectly through the app module
    /// helper exposed for testing. Verify a session with `parent_id` is
    /// considered a sub-agent even when it has a title (OpenCode always gives
    /// sub-agents a title like "... (@explore subagent)").
    #[test]
    fn session_with_parent_id_is_sub_agent_even_with_title() {
        let mut s = Session::new("sid".into(), "proj".into(), PathBuf::from("/tmp/s.json"));
        s.source = SourceKind::Opencode;
        s.title = Some("Find stuff (@explore subagent)".into());
        // Without parent_id, not a sub-agent (title present, not background).
        assert!(!crate::app::is_sub_agent_for_test(&s));
        s.parent_id = Some("ses_parent".into());
        // Now it should be a sub-agent.
        assert!(crate::app::is_sub_agent_for_test(&s));
    }

    /// Verify that a HashSet<String> of IDs we'd pass to the store-level
    /// sub-agent filter finds the sub-agent by its `parent_id`.
    #[test]
    fn sub_agent_ids_collected_correctly() {
        let tmp = TmpDir::new("subagent_ids");
        write_fixture(tmp.path());
        let src = OpencodeSource::new(tmp.path().to_path_buf());
        let scanned = src.scan();
        let (_, sessions) = scanned.iter().find(|(p, _)| p.slug == PROJECT_ID).unwrap();
        let sub_ids: HashSet<&str> = sessions
            .iter()
            .filter(|s| s.parent_id.is_some())
            .map(|s| s.id.as_str())
            .collect();
        assert!(sub_ids.contains(SUBAGENT_SESSION_ID));
        assert!(!sub_ids.contains(SESSION_ID));
    }

    // ── PRD-04: load_session (message/part event stream + token usage) ────────

    /// Write a message JSON + its part JSONs for `load_session` testing.
    fn write_message(root: &Path, sid: &str, mid: &str, msg_json: &str, parts: &[(&str, &str)]) {
        let msg_dir = root.join("message").join(sid);
        fs::create_dir_all(&msg_dir).unwrap();
        fs::write(msg_dir.join(format!("{mid}.json")), msg_json).unwrap();
        let part_dir = root.join("part").join(mid);
        fs::create_dir_all(&part_dir).unwrap();
        for (pid, pjson) in parts {
            fs::write(part_dir.join(format!("{pid}.json")), pjson).unwrap();
        }
    }

    #[test]
    fn load_session_maps_parts_indexes_tools_and_tallies_usage() {
        let tmp = TmpDir::new("load");
        write_fixture(tmp.path());
        let root = tmp.path();
        let sid = SESSION_ID;

        // User message (created earlier) — text lives in its part.
        write_message(
            root,
            sid,
            "msg_user001",
            r#"{ "id":"msg_user001", "sessionID":"ses_x", "role":"user",
                "time":{"created":1781045149000},
                "model":{"providerID":"opencode-go","modelID":"deepseek-v4-flash"} }"#,
            &[(
                "prt_u1",
                r#"{ "type":"text", "text":"build the thing", "time":{"start":1781045149000,"end":1781045149000} }"#,
            )],
        );

        // Assistant message (created later) — reasoning + text + tool parts, with tokens & cost.
        write_message(
            root,
            sid,
            "msg_asst001",
            r#"{ "id":"msg_asst001", "sessionID":"ses_x", "role":"assistant",
                "time":{"created":1781045149262,"completed":1781045151439},
                "modelID":"deepseek-v4-flash", "providerID":"opencode-go",
                "cost":0.00185332,
                "tokens":{"input":12926,"output":127,"reasoning":29,"cache":{"read":40,"write":5}},
                "finish":"tool-calls" }"#,
            &[
                (
                    "prt_a1",
                    r#"{ "type":"reasoning", "text":"let me think", "time":{"start":1781045149300,"end":1781045149400} }"#,
                ),
                (
                    "prt_a2",
                    r#"{ "type":"text", "text":"on it", "time":{"start":1781045149500,"end":1781045149600} }"#,
                ),
                (
                    "prt_a3",
                    r#"{ "type":"tool", "callID":"call_17329035", "tool":"glob",
                        "state":{ "status":"completed", "input":{"pattern":"*.rs"},
                                  "output":"No files found",
                                  "time":{"start":1781045149700,"end":1781045149800} } }"#,
                ),
                ("prt_a0", r#"{ "type":"step-start" }"#),
            ],
        );

        let src = OpencodeSource::new(root.to_path_buf());
        let mut session = Session::new(sid.to_string(), PROJECT_ID.to_string(), {
            root.join("session")
                .join(PROJECT_ID)
                .join(format!("{sid}.json"))
        });
        session.source = SourceKind::Opencode;

        src.load_session(&mut session).unwrap();
        assert!(session.loaded);

        // Expected event sequence: User(text), Assistant{Thinking,Text,ToolUse}, User(ToolResults).
        // step-start produces no event.
        assert_eq!(
            session.events.len(),
            3,
            "user + assistant + synthesized result"
        );

        match &session.events[0].event {
            Event::User(UserContent::Text(s)) => assert_eq!(s, "build the thing"),
            other => panic!("expected user text, got {other:?}"),
        }

        match &session.events[1].event {
            Event::Assistant { blocks, usage } => {
                assert_eq!(blocks.len(), 3, "thinking + text + tool_use");
                assert!(matches!(blocks[0], AssistantBlock::Thinking { .. }));
                assert!(matches!(blocks[1], AssistantBlock::Text { .. }));
                match &blocks[2] {
                    AssistantBlock::ToolUse { id, name, input } => {
                        assert_eq!(id, "call_17329035");
                        assert_eq!(name, "glob");
                        assert_eq!(input.get("pattern").and_then(|v| v.as_str()), Some("*.rs"));
                    }
                    other => panic!("expected tool_use, got {other:?}"),
                }
                assert!(usage.is_some());
            }
            other => panic!("expected assistant, got {other:?}"),
        }
        // model is "<providerID>/<modelID>".
        assert_eq!(
            session.events[1].model.as_deref(),
            Some("opencode-go/deepseek-v4-flash")
        );
        // timestamp from message time.created (epoch ms).
        assert_eq!(
            session.events[1].timestamp.unwrap().timestamp_millis(),
            1_781_045_149_262
        );

        // Synthesized tool result event.
        match &session.events[2].event {
            Event::User(UserContent::ToolResults(rs)) => {
                assert_eq!(rs.len(), 1);
                assert_eq!(rs[0].tool_use_id.as_deref(), Some("call_17329035"));
                assert_eq!(rs[0].content, "No files found");
                assert!(!rs[0].is_error);
            }
            other => panic!("expected synthesized tool result, got {other:?}"),
        }

        // Cross-link indexes populated for the callID.
        assert!(session.tool_use_index.contains_key("call_17329035"));
        assert!(session.tool_result_index.contains_key("call_17329035"));

        // Token mapping: reasoning (29) folds into output (127) → 156.
        assert!(session.usage_totals.has_usage);
        assert_eq!(session.usage_totals.input, 12926);
        assert_eq!(session.usage_totals.output, 127 + 29);
        assert_eq!(session.usage_totals.cache_creation, 5);
        assert_eq!(session.usage_totals.cache_read, 40);
        // Authoritative cost preferred (non-zero message cost).
        assert!((session.usage_totals.cost_usd - 0.00185332).abs() < 1e-9);
        // last_input_tokens = input + cache.read + cache.write of most recent assistant.
        assert_eq!(session.last_input_tokens, Some(12926 + 40 + 5));
    }

    #[test]
    fn load_session_tool_error_status_sets_is_error() {
        let tmp = TmpDir::new("toolerr");
        write_fixture(tmp.path());
        let root = tmp.path();
        let sid = SESSION_ID;

        write_message(
            root,
            sid,
            "msg_e001",
            r#"{ "id":"msg_e001", "sessionID":"ses_x", "role":"assistant",
                "time":{"created":1781045149262},
                "modelID":"m", "providerID":"p", "cost":0,
                "tokens":{"input":1,"output":1,"reasoning":0,"cache":{"read":0,"write":0}} }"#,
            &[(
                "prt_e1",
                r#"{ "type":"tool", "callID":"call_err", "tool":"bash",
                    "state":{ "status":"error", "input":{}, "output":"boom",
                              "time":{"start":1,"end":2} } }"#,
            )],
        );

        let src = OpencodeSource::new(root.to_path_buf());
        let mut session = Session::new(sid.to_string(), PROJECT_ID.to_string(), {
            root.join("session")
                .join(PROJECT_ID)
                .join(format!("{sid}.json"))
        });
        session.source = SourceKind::Opencode;
        src.load_session(&mut session).unwrap();

        let (evt, res) = session.tool_result_index.get("call_err").cloned().unwrap();
        if let Event::User(UserContent::ToolResults(rs)) = &session.events[evt].event {
            assert!(rs[res].is_error);
            assert_eq!(rs[res].content, "boom");
        } else {
            panic!("expected tool result event");
        }
    }

    #[test]
    fn load_session_zero_cost_falls_back_to_token_only() {
        // When message cost is 0, no authoritative cost is added; cost stays 0
        // here because providerID/modelID won't match the Claude pricing table.
        let tmp = TmpDir::new("zerocost");
        write_fixture(tmp.path());
        let root = tmp.path();
        let sid = SESSION_ID;
        write_message(
            root,
            sid,
            "msg_z001",
            r#"{ "id":"msg_z001", "sessionID":"ses_x", "role":"assistant",
                "time":{"created":1781045149262},
                "modelID":"gpt-oss-120b", "providerID":"nvidia", "cost":0,
                "tokens":{"input":100,"output":10,"reasoning":0,"cache":{"read":0,"write":0}} }"#,
            &[(
                "prt_z1",
                r#"{ "type":"text", "text":"hi", "time":{"start":1,"end":2} }"#,
            )],
        );
        let src = OpencodeSource::new(root.to_path_buf());
        let mut session = Session::new(sid.to_string(), PROJECT_ID.to_string(), {
            root.join("session")
                .join(PROJECT_ID)
                .join(format!("{sid}.json"))
        });
        session.source = SourceKind::Opencode;
        src.load_session(&mut session).unwrap();
        assert_eq!(session.usage_totals.input, 100);
        assert_eq!(session.usage_totals.cost_usd, 0.0);
    }
}
