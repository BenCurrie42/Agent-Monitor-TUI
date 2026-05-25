use std::collections::HashSet;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::data::{AssistantBlock, Event, EventRecord, Session, UserContent};
use crate::store::{is_session_live, FsEvent, Store};

/// Sentinel "slug" used as the key for the Closed sessions dropdown in
/// `AppState.expanded`. Not a real project slug.
pub const CLOSED_KEY: &str = "__closed__";
pub const SUB_AGENT_KEY: &str = "__subagents__";

#[derive(Debug, Clone)]
pub enum AppEvent {
    Key(KeyEvent),
    Resize,
    Fs(FsEvent),
    /// One entry per running `claude` process, with duplicates if multiple
    /// processes share a CWD. Consumed by `Store::apply_open_files`.
    OpenFiles(Vec<PathBuf>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Stream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Filter,
    Detail,
    Help,
    DeleteConfirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailView {
    Pretty,
    Raw,
}

#[derive(Debug, Clone, Copy)]
pub struct StreamItem {
    pub event_idx: usize,
    /// Sub-index within the event. For Assistant: block index. For User
    /// ToolResults: result index. None means "the whole event".
    pub sub_idx: Option<usize>,
}

pub struct AppState {
    pub focus: Focus,
    pub mode: Mode,
    pub follow: bool,
    /// Show low-value meta events (system, attachment, ai-title, etc.).
    pub show_meta: bool,
    pub detail_view: DetailView,
    /// Expanded project slugs in the sidebar.
    pub expanded: HashSet<String>,
    /// Selected (project_slug, session_id) — None if none selected yet.
    pub selected_project: Option<String>,
    pub selected_session: Option<String>,
    /// Cursor in the sidebar (flat index into the rendered tree).
    pub sidebar_cursor: usize,
    /// Cursor into the selected session's filtered stream items.
    pub stream_cursor: usize,
    /// First visible item index in the event stream. Persists across renders so
    /// the cursor can move within the visible window without the window itself
    /// scrolling until the cursor reaches an edge.
    pub stream_viewport: usize,
    /// Filter buffer (lowercased on apply).
    pub filter: String,
    pub filter_input: String,
    /// Vertical scroll offset for the detail modal (in lines).
    pub detail_scroll: u16,
    /// Sidebar manually collapsed by the user (also auto-collapsed when width < 100).
    pub sidebar_collapsed: bool,
}

impl AppState {
    pub fn new(follow: bool) -> Self {
        Self {
            focus: Focus::Sidebar,
            mode: Mode::Normal,
            follow,
            show_meta: false,
            detail_view: DetailView::Pretty,
            expanded: HashSet::new(),
            selected_project: None,
            selected_session: None,
            sidebar_cursor: 0,
            stream_cursor: 0,
            stream_viewport: 0,
            filter: String::new(),
            filter_input: String::new(),
            detail_scroll: 0,
            sidebar_collapsed: false,
        }
    }

    /// Land on the most-recently-active session globally: expand its project,
    /// place the sidebar cursor on it, focus the event stream. Auto-expands the
    /// Closed section if the most-recent session is not currently live, so the
    /// user can still see and reach it.
    pub fn select_first(&mut self, store: &mut Store) {
        let pick: Option<(String, String, bool)> = store.most_recent_session().map(|s| {
            (
                s.project_slug.clone(),
                s.id.clone(),
                crate::store::is_session_live(s),
            )
        });
        if let Some((proj_slug, sid, live)) = pick {
            self.expanded.insert(proj_slug.clone());
            if !live {
                self.expanded.insert(CLOSED_KEY.to_string());
            }
            self.selected_project = Some(proj_slug);
            self.selected_session = Some(sid.clone());
            self.focus = Focus::Stream;
            let _ = store.ensure_loaded(&sid);
            // Position the sidebar cursor on this session's row.
            let rows = sidebar_rows(store, &self.expanded);
            self.sidebar_cursor = rows
                .iter()
                .position(|r| matches!(r, SidebarRow::Session { session_id, .. } if session_id == &sid))
                .unwrap_or(0);
            return;
        }
        // No activity anywhere — fall back to expanding the first project.
        if let Some(slug) = store.project_order_by_recency().into_iter().next() {
            self.expanded.insert(slug.clone());
            self.selected_project = Some(slug);
            self.sidebar_cursor = 0;
        }
    }

    pub fn preselect_session(&mut self, store: &mut Store, sid: &str) {
        if let Some(s) = store.sessions.get(sid) {
            self.expanded.insert(s.project_slug.clone());
            self.selected_project = Some(s.project_slug.clone());
            self.selected_session = Some(sid.to_string());
            let _ = store.ensure_loaded(sid);
            // Best-effort cursor placement: find its tree row later in UI.
            self.sidebar_cursor = 0;
        }
    }

    /// Returns true if the app should quit.
    pub fn handle_key(&mut self, k: KeyEvent, store: &mut Store) -> bool {
        // Ctrl-C always quits.
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            return true;
        }
        match self.mode {
            Mode::Filter => self.handle_key_filter(k),
            Mode::Detail => self.handle_key_detail(k),
            Mode::Help => self.handle_key_help(k),
            Mode::DeleteConfirm => self.handle_key_delete_confirm(k, store),
            Mode::Normal => self.handle_key_normal(k, store),
        }
    }

    fn handle_key_normal(&mut self, k: KeyEvent, store: &mut Store) -> bool {
        match k.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('?') => {
                self.mode = Mode::Help;
            }
            KeyCode::Char('D') => {
                let rows = sidebar_rows(store, &self.expanded);
                let idx = self.sidebar_cursor.min(rows.len().saturating_sub(1));
                let on_delete_target = matches!(
                    rows.get(idx),
                    Some(SidebarRow::ClosedHeader { .. }) | Some(SidebarRow::DeleteClosedRow)
                );
                if on_delete_target {
                    self.mode = Mode::DeleteConfirm;
                }
            }
            KeyCode::Tab => {
                self.focus = if self.focus == Focus::Sidebar {
                    Focus::Stream
                } else {
                    Focus::Sidebar
                };
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Filter;
                self.filter_input = self.filter.clone();
            }
            KeyCode::Char('b') => {
                self.sidebar_collapsed = !self.sidebar_collapsed;
                if self.sidebar_collapsed {
                    self.focus = Focus::Stream;
                }
            }
            KeyCode::Char('f') => self.follow = !self.follow,
            KeyCode::Char('v') => {
                self.show_meta = !self.show_meta;
                self.stream_cursor = 0;
                self.stream_viewport = 0;
            }
            KeyCode::Enter => match self.focus {
                Focus::Sidebar => {
                    if self.selected_session.is_some() {
                        self.focus = Focus::Stream;
                    }
                }
                Focus::Stream => {
                    if self.selected_session.is_some() {
                        self.mode = Mode::Detail;
                        self.detail_view = DetailView::Pretty;
                        self.detail_scroll = 0;
                    }
                }
            },
            KeyCode::Char('g') => match self.focus {
                Focus::Sidebar => {
                    self.sidebar_cursor = 0;
                    self.refresh_selection_from_cursor(store);
                }
                Focus::Stream => {
                    self.follow = false;
                    self.stream_cursor = 0;
                }
            },
            KeyCode::Char('G') => match self.focus {
                Focus::Sidebar => {
                    self.sidebar_cursor = usize::MAX; // clamped below
                    self.refresh_selection_from_cursor(store);
                }
                Focus::Stream => {
                    // G means "jump to bottom" — equivalent to re-enabling follow.
                    self.follow = true;
                    self.stream_cursor = usize::MAX;
                }
            },
            KeyCode::Down | KeyCode::Char('j') => match self.focus {
                Focus::Sidebar => {
                    self.sidebar_cursor = self.sidebar_cursor.saturating_add(1);
                    self.refresh_selection_from_cursor(store);
                }
                Focus::Stream => {
                    let bottom = self.bottom_index(store);
                    if self.follow {
                        self.stream_cursor = bottom;
                        self.follow = false;
                    } else {
                        self.stream_cursor = self.stream_cursor.saturating_add(1).min(bottom);
                    }
                }
            },
            KeyCode::Up | KeyCode::Char('k') => match self.focus {
                Focus::Sidebar => {
                    self.sidebar_cursor = self.sidebar_cursor.saturating_sub(1);
                    self.refresh_selection_from_cursor(store);
                }
                Focus::Stream => {
                    if self.follow {
                        self.stream_cursor = self.bottom_index(store).saturating_sub(1);
                        self.follow = false;
                    } else {
                        self.stream_cursor = self.stream_cursor.saturating_sub(1);
                    }
                }
            },
            KeyCode::Right | KeyCode::Char('l') => {
                if self.focus == Focus::Sidebar {
                    self.handle_sidebar_l(store);
                }
            }
            KeyCode::Left | KeyCode::Char('h') => match self.focus {
                Focus::Sidebar => self.handle_sidebar_h(store),
                Focus::Stream => self.focus = Focus::Sidebar,
            },
            KeyCode::Char('n') => self.stream_cursor = self.stream_cursor.saturating_add(1),
            KeyCode::Char('N') => self.stream_cursor = self.stream_cursor.saturating_sub(1),
            KeyCode::Esc => self.filter.clear(),
            _ => {}
        }
        false
    }

    fn handle_key_filter(&mut self, k: KeyEvent) -> bool {
        match k.code {
            KeyCode::Esc => {
                self.filter_input.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                self.filter = self.filter_input.trim().to_lowercase();
                self.mode = Mode::Normal;
                self.stream_cursor = 0;
                self.stream_viewport = 0;
            }
            KeyCode::Backspace => {
                self.filter_input.pop();
            }
            KeyCode::Char(c) => self.filter_input.push(c),
            _ => {}
        }
        false
    }

    fn handle_key_detail(&mut self, k: KeyEvent) -> bool {
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Char('R') | KeyCode::Char('r') => {
                self.detail_view = match self.detail_view {
                    DetailView::Pretty => DetailView::Raw,
                    DetailView::Raw => DetailView::Pretty,
                };
                self.detail_scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.detail_scroll = self.detail_scroll.saturating_add(1);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.detail_scroll = self.detail_scroll.saturating_sub(1);
            }
            KeyCode::PageDown | KeyCode::Char('d') => {
                self.detail_scroll = self.detail_scroll.saturating_add(10);
            }
            KeyCode::PageUp | KeyCode::Char('u') => {
                self.detail_scroll = self.detail_scroll.saturating_sub(10);
            }
            KeyCode::Char('g') => self.detail_scroll = 0,
            KeyCode::Char('G') => self.detail_scroll = u16::MAX, // clamped by render
            _ => {}
        }
        false
    }

    fn handle_key_help(&mut self, k: KeyEvent) -> bool {
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                self.mode = Mode::Normal;
            }
            _ => {}
        }
        false
    }

    fn handle_key_delete_confirm(&mut self, k: KeyEvent, store: &mut Store) -> bool {
        match k.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                store.delete_closed_sessions();
                self.sidebar_cursor = 0;
                self.selected_session = None;
                self.selected_project = None;
                self.mode = Mode::Normal;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            _ => {}
        }
        false
    }

    /// Last visible (filtered) stream item index in the selected session, or 0 if none.
    fn bottom_index(&self, store: &Store) -> usize {
        let Some(sid) = &self.selected_session else {
            return 0;
        };
        filtered_count(store, sid, &self.filter, self.show_meta).saturating_sub(1)
    }

    /// Called by ui::render after laying out rows, so we know what's actually selected.
    /// Also pins `sidebar_cursor` to the row of `selected_session` if that session
    /// still exists — so background activity (FS events, lsof ticks) that reorders
    /// rows doesn't drag the cursor onto a different session.
    pub fn resolve_selection(&mut self, store: &mut Store) {
        let rows = sidebar_rows(store, &self.expanded);
        if rows.is_empty() {
            self.selected_session = None;
            return;
        }
        if let Some(sid) = &self.selected_session {
            if let Some(idx) = rows.iter().position(
                |r| matches!(r, SidebarRow::Session { session_id, .. } if session_id == sid),
            ) {
                self.sidebar_cursor = idx;
            }
        }
        let max = rows.len().saturating_sub(1);
        if self.sidebar_cursor > max {
            self.sidebar_cursor = max;
        }
        match &rows[self.sidebar_cursor] {
            SidebarRow::Project { slug, .. } => {
                self.selected_project = Some(slug.clone());
                self.selected_session = None;
            }
            SidebarRow::Session {
                project_slug,
                session_id,
                ..
            } => {
                self.selected_project = Some(project_slug.clone());
                let changed = self.selected_session.as_deref() != Some(session_id.as_str());
                self.selected_session = Some(session_id.clone());
                if changed {
                    let _ = store.ensure_loaded(session_id);
                    self.stream_cursor = 0;
                    self.stream_viewport = 0;
                }
            }
            SidebarRow::ClosedHeader { .. }
            | SidebarRow::SubAgentHeader { .. }
            | SidebarRow::DeleteClosedRow => {
                // Header/action rows: don't change session selection.
            }
        }
    }

    /// `l` in the sidebar: open one level deeper.
    /// - On a Session row, switch focus to the events stream (same as Enter).
    /// - On an expandable row that's collapsed, expand it.
    /// - On an expandable row that's already expanded, descend onto its first child.
    fn handle_sidebar_l(&mut self, store: &mut Store) {
        let rows = sidebar_rows(store, &self.expanded);
        if rows.is_empty() || self.sidebar_cursor >= rows.len() {
            return;
        }
        let idx = self.sidebar_cursor;
        let max = rows.len() - 1;
        match &rows[idx] {
            SidebarRow::Session { .. } => {
                if self.selected_session.is_some() {
                    self.focus = Focus::Stream;
                }
            }
            SidebarRow::Project { slug, .. } => {
                if self.expanded.contains(slug) {
                    self.sidebar_cursor = (idx + 1).min(max);
                    self.refresh_selection_from_cursor(store);
                } else {
                    self.expanded.insert(slug.clone());
                }
            }
            SidebarRow::ClosedHeader { .. } => {
                if self.expanded.contains(CLOSED_KEY) {
                    self.sidebar_cursor = (idx + 1).min(max);
                    self.refresh_selection_from_cursor(store);
                } else {
                    self.expanded.insert(CLOSED_KEY.to_string());
                }
            }
            SidebarRow::SubAgentHeader { .. } => {
                if self.expanded.contains(SUB_AGENT_KEY) {
                    self.sidebar_cursor = (idx + 1).min(max);
                    self.refresh_selection_from_cursor(store);
                } else {
                    self.expanded.insert(SUB_AGENT_KEY.to_string());
                }
            }
            SidebarRow::DeleteClosedRow => {}
        }
    }

    /// `h` in the sidebar: step back up one level.
    /// - On a Session row, move the cursor to its parent (Project or SubAgent header).
    /// - On an expanded Project/header row, collapse it.
    /// - On a collapsed closed-section Project, jump up to the Closed header.
    /// - On the DeleteClosedRow, jump up to the Closed header.
    fn handle_sidebar_h(&mut self, store: &mut Store) {
        let rows = sidebar_rows(store, &self.expanded);
        if rows.is_empty() || self.sidebar_cursor >= rows.len() {
            return;
        }
        let idx = self.sidebar_cursor;
        match &rows[idx] {
            SidebarRow::Session { project_slug, .. } => {
                let target_slug = project_slug.clone();
                let parent = (0..idx).rev().find(|i| {
                    matches!(&rows[*i], SidebarRow::Project { slug, .. } if slug == &target_slug)
                        || matches!(&rows[*i], SidebarRow::SubAgentHeader { .. })
                });
                if let Some(p) = parent {
                    self.sidebar_cursor = p;
                    self.refresh_selection_from_cursor(store);
                }
            }
            SidebarRow::Project { slug, closed } => {
                if self.expanded.contains(slug) {
                    self.expanded.remove(slug);
                } else if *closed {
                    let parent = (0..idx)
                        .rev()
                        .find(|i| matches!(&rows[*i], SidebarRow::ClosedHeader { .. }));
                    if let Some(p) = parent {
                        self.sidebar_cursor = p;
                        self.refresh_selection_from_cursor(store);
                    }
                }
            }
            SidebarRow::ClosedHeader { .. } => {
                self.expanded.remove(CLOSED_KEY);
            }
            SidebarRow::SubAgentHeader { .. } => {
                self.expanded.remove(SUB_AGENT_KEY);
            }
            SidebarRow::DeleteClosedRow => {
                let parent = (0..idx)
                    .rev()
                    .find(|i| matches!(&rows[*i], SidebarRow::ClosedHeader { .. }));
                if let Some(p) = parent {
                    self.sidebar_cursor = p;
                    self.refresh_selection_from_cursor(store);
                }
            }
        }
    }

    /// After explicit cursor movement (j/k/g/G), sync `selected_session` and
    /// `selected_project` to whatever row the cursor now points at. Without
    /// this, `resolve_selection`'s pin would snap the cursor back to the old
    /// selection on the next render.
    fn refresh_selection_from_cursor(&mut self, store: &mut Store) {
        let rows = sidebar_rows(store, &self.expanded);
        if rows.is_empty() {
            self.selected_session = None;
            self.selected_project = None;
            return;
        }
        let max = rows.len() - 1;
        if self.sidebar_cursor > max {
            self.sidebar_cursor = max;
        }
        match &rows[self.sidebar_cursor] {
            SidebarRow::Session {
                project_slug,
                session_id,
                ..
            } => {
                self.selected_project = Some(project_slug.clone());
                let changed = self.selected_session.as_deref() != Some(session_id.as_str());
                self.selected_session = Some(session_id.clone());
                if changed {
                    let _ = store.ensure_loaded(session_id);
                    self.stream_cursor = 0;
                    self.stream_viewport = 0;
                }
            }
            SidebarRow::Project { slug, .. } => {
                self.selected_project = Some(slug.clone());
                self.selected_session = None;
            }
            SidebarRow::ClosedHeader { .. }
            | SidebarRow::SubAgentHeader { .. }
            | SidebarRow::DeleteClosedRow => {
                // Headers don't carry a selection; clear so pin doesn't fight the user.
                self.selected_session = None;
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum SidebarRow {
    Project {
        slug: String,
        closed: bool,
    },
    Session {
        project_slug: String,
        session_id: String,
        closed: bool,
    },
    ClosedHeader {
        project_count: usize,
        session_count: usize,
        expanded: bool,
    },
    SubAgentHeader {
        session_count: usize,
        expanded: bool,
    },
    DeleteClosedRow,
}

/// Whether an event should appear in the stream given the current visibility
/// preferences. Hides low-value meta types by default.
pub fn is_visible(rec: &EventRecord, show_meta: bool) -> bool {
    match &rec.event {
        Event::User(UserContent::Text(_)) | Event::Assistant { .. } | Event::Unknown(_) => true,
        Event::User(UserContent::ToolResults(_))
        | Event::System { .. }
        | Event::Attachment(_)
        | Event::AiTitle(_)
        | Event::LastPrompt(_)
        | Event::PermissionMode(_)
        | Event::FileHistorySnapshot => show_meta,
    }
}

/// Expand a session into stream items honoring visibility + filter.
/// Each StreamItem is a single navigable/openable row.
pub fn stream_items(session: &Session, filter: &str, show_meta: bool) -> Vec<StreamItem> {
    let mut items = Vec::new();
    for (i, rec) in session.events.iter().enumerate() {
        if !is_visible(rec, show_meta) {
            continue;
        }
        match &rec.event {
            Event::Assistant { blocks, .. } if !blocks.is_empty() => {
                for bi in 0..blocks.len() {
                    items.push(StreamItem {
                        event_idx: i,
                        sub_idx: Some(bi),
                    });
                }
            }
            Event::User(UserContent::ToolResults(rs)) if !rs.is_empty() => {
                for ri in 0..rs.len() {
                    items.push(StreamItem {
                        event_idx: i,
                        sub_idx: Some(ri),
                    });
                }
            }
            _ => items.push(StreamItem {
                event_idx: i,
                sub_idx: None,
            }),
        }
    }
    if !filter.is_empty() {
        items.retain(|it| item_matches(session, it, filter));
    }
    items
}

/// Substring match (case-insensitive) of `needle_lower` against the textual
/// content of a StreamItem. Used by both filter rendering and cursor logic.
pub fn item_matches(session: &Session, item: &StreamItem, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    let Some(rec) = session.events.get(item.event_idx) else {
        return false;
    };
    let mut buf = String::new();
    fn push(buf: &mut String, s: &str) {
        if !buf.is_empty() {
            buf.push(' ');
        }
        buf.push_str(s);
    }
    if rec.is_sidechain {
        push(&mut buf, "sidechain");
    }
    match (&rec.event, item.sub_idx) {
        (Event::Assistant { blocks, .. }, Some(b)) => {
            push(&mut buf, "assistant");
            if let Some(blk) = blocks.get(b) {
                match blk {
                    AssistantBlock::Thinking { text } => {
                        push(&mut buf, "thinking");
                        push(&mut buf, text);
                    }
                    AssistantBlock::Text { text } => push(&mut buf, text),
                    AssistantBlock::ToolUse { name, input, .. } => {
                        push(&mut buf, name);
                        if let Ok(s) = serde_json::to_string(input) {
                            push(&mut buf, &s);
                        }
                    }
                }
            }
        }
        (Event::User(UserContent::ToolResults(rs)), Some(r)) => {
            push(&mut buf, "result");
            if let Some(tr) = rs.get(r) {
                push(&mut buf, &tr.content);
                if tr.is_error {
                    push(&mut buf, "error");
                }
            }
        }
        (Event::User(UserContent::Text(s)), _) => {
            push(&mut buf, "user");
            push(&mut buf, s);
        }
        (Event::System { subtype, body }, _) => {
            push(&mut buf, "system");
            push(&mut buf, subtype);
            if let Ok(s) = serde_json::to_string(body) {
                push(&mut buf, &s);
            }
        }
        (Event::AiTitle(t), _) => {
            push(&mut buf, "title");
            push(&mut buf, t);
        }
        (Event::LastPrompt(t), _) => {
            push(&mut buf, "last-prompt");
            push(&mut buf, t);
        }
        (Event::PermissionMode(m), _) => {
            push(&mut buf, "permission-mode");
            push(&mut buf, m);
        }
        (Event::Attachment(_), _) => push(&mut buf, "attachment"),
        (Event::FileHistorySnapshot, _) => push(&mut buf, "file-history-snapshot"),
        (Event::Unknown(t), _) => push(&mut buf, t),
        (Event::Assistant { .. }, None) => push(&mut buf, "assistant"),
        (Event::User(UserContent::ToolResults(_)), None) => push(&mut buf, "result"),
    }
    buf.to_lowercase().contains(needle_lower)
}

pub fn filtered_count(store: &Store, session_id: &str, filter: &str, show_meta: bool) -> usize {
    let Some(s) = store.sessions.get(session_id) else {
        return 0;
    };
    stream_items(s, filter, show_meta).len()
}

/// A session is considered a sub-agent if it was explicitly launched as a
/// background sub-agent from another session. Detection currently relies on
/// the absence of any human-typed first_user_line combined with is_background;
/// this is a best-effort heuristic until JSONL includes a parentSessionId field.
fn is_sub_agent(s: &crate::data::Session) -> bool {
    s.is_background && s.first_user_line.is_none() && s.title.is_none()
}

pub fn sidebar_rows(store: &Store, expanded: &HashSet<String>) -> Vec<SidebarRow> {
    let mut rows = Vec::new();

    // Section 1: Live (non-sub-agent, live sessions).
    for slug in store.project_order_by_recency() {
        let Some(project) = store.projects.get(&slug) else {
            continue;
        };
        let live: Vec<&String> = project
            .sessions
            .iter()
            .filter(|sid| {
                store
                    .sessions
                    .get(*sid)
                    .map(|s| is_session_live(s) && !is_sub_agent(s))
                    .unwrap_or(false)
            })
            .collect();
        if live.is_empty() {
            continue;
        }
        rows.push(SidebarRow::Project {
            slug: slug.clone(),
            closed: false,
        });
        if expanded.contains(&slug) {
            for sid in live {
                rows.push(SidebarRow::Session {
                    project_slug: slug.clone(),
                    session_id: sid.clone(),
                    closed: false,
                });
            }
        }
    }

    // Section 2: Sub-agents (background sessions without a user-visible title/first-line).
    let sub_agent_ids: Vec<String> = {
        let mut ids: Vec<String> = store
            .sessions
            .iter()
            .filter(|(_, s)| is_sub_agent(s))
            .map(|(id, _)| id.clone())
            .collect();
        // Sort by last activity descending.
        ids.sort_by(|a, b| {
            let ta = store
                .sessions
                .get(a)
                .and_then(|s| s.last_event.or(s.last_mtime));
            let tb = store
                .sessions
                .get(b)
                .and_then(|s| s.last_event.or(s.last_mtime));
            tb.cmp(&ta)
        });
        ids
    };
    if !sub_agent_ids.is_empty() {
        let sub_expanded = expanded.contains(SUB_AGENT_KEY);
        rows.push(SidebarRow::SubAgentHeader {
            session_count: sub_agent_ids.len(),
            expanded: sub_expanded,
        });
        if sub_expanded {
            for sid in &sub_agent_ids {
                let proj_slug = store
                    .sessions
                    .get(sid)
                    .map(|s| s.project_slug.clone())
                    .unwrap_or_default();
                rows.push(SidebarRow::Session {
                    project_slug: proj_slug,
                    session_id: sid.clone(),
                    closed: false,
                });
            }
        }
    }

    // Section 3: Closed (non-sub-agent, non-live sessions).
    let mut visible_closed_projs = 0;
    let mut closed_sess_count = 0;
    for slug in store.projects.keys() {
        let has_closed = store
            .projects
            .get(slug)
            .map(|p| {
                p.sessions.iter().any(|sid| {
                    store
                        .sessions
                        .get(sid)
                        .map(|s| !is_session_live(s) && !is_sub_agent(s))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if has_closed {
            visible_closed_projs += 1;
            if let Some(p) = store.projects.get(slug) {
                closed_sess_count += p
                    .sessions
                    .iter()
                    .filter(|sid| {
                        store
                            .sessions
                            .get(*sid)
                            .map(|s| !is_session_live(s) && !is_sub_agent(s))
                            .unwrap_or(false)
                    })
                    .count();
            }
        }
    }
    if closed_sess_count > 0 {
        let header_expanded = expanded.contains(CLOSED_KEY);
        rows.push(SidebarRow::ClosedHeader {
            project_count: visible_closed_projs,
            session_count: closed_sess_count,
            expanded: header_expanded,
        });
        if header_expanded {
            rows.push(SidebarRow::DeleteClosedRow);
            for slug in store.project_order_by_recency() {
                let Some(project) = store.projects.get(&slug) else {
                    continue;
                };
                let closed_sessions: Vec<&String> = project
                    .sessions
                    .iter()
                    .filter(|sid| {
                        store
                            .sessions
                            .get(*sid)
                            .map(|s| !is_session_live(s) && !is_sub_agent(s))
                            .unwrap_or(false)
                    })
                    .collect();
                if closed_sessions.is_empty() {
                    continue;
                }
                rows.push(SidebarRow::Project {
                    slug: slug.clone(),
                    closed: true,
                });
                if expanded.contains(&slug) {
                    for sid in closed_sessions {
                        rows.push(SidebarRow::Session {
                            project_slug: slug.clone(),
                            session_id: sid.clone(),
                            closed: true,
                        });
                    }
                }
            }
        }
    }

    rows
}
