use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;

const MAX_FIELD_LEN: usize = 4096;

/// Which on-disk source a project/session originates from. Defaults to
/// `Claude`; later sources (OpenCode) set it explicitly so the UI can badge
/// and route per item. Keeping a default keeps existing construction sites
/// compiling unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SourceKind {
    #[default]
    Claude,
    #[allow(dead_code)] // populated starting with PRD-03 (OpenCode reader)
    Opencode,
}

#[derive(Debug, Clone)]
pub struct Project {
    #[allow(dead_code)] // keyed in the store by the same slug; kept for symmetry/future use
    pub slug: String,
    #[allow(dead_code)] // disk path; future: open-in-editor, etc.
    pub path: PathBuf,
    pub display_path: String,
    pub sessions: Vec<String>,
    /// Originating data source. Defaults to `Claude`. Read starting with PRD-07
    /// (per-source UI badging/routing).
    #[allow(dead_code)]
    pub source: SourceKind,
}

impl Project {
    pub fn new(slug: String, path: PathBuf) -> Self {
        let display_path = decode_slug(&slug);
        Self {
            slug,
            path,
            display_path,
            sessions: Vec::new(),
            source: SourceKind::Claude,
        }
    }

    /// Create a project with an explicit display path (for sources that don't
    /// use the Claude slug encoding, e.g. OpenCode whose project IDs are
    /// opaque hashes and whose display paths come from a `worktree` field).
    ///
    /// Wired into the main binary in PRD-07.
    #[allow(dead_code)]
    pub fn with_display_path(
        slug: String,
        path: PathBuf,
        display_path: String,
        source: SourceKind,
    ) -> Self {
        Self {
            slug,
            path,
            display_path,
            sessions: Vec::new(),
            source,
        }
    }
}

/// Decode `~/.claude/projects/-Users-x--config-nix` → `/Users/x/.config/nix`.
/// The slug replaces both `/` and `.` with `-`, ambiguously. We do a best-effort
/// reverse by treating consecutive dashes as a path separator + dot. It is only
/// used for display, so a perfect roundtrip is not required.
pub fn decode_slug(slug: &str) -> String {
    let mut out = String::with_capacity(slug.len());
    let bytes = slug.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'-' {
            // Count consecutive dashes.
            let start = i;
            while i < bytes.len() && bytes[i] == b'-' {
                i += 1;
            }
            let run = i - start;
            // Leading run (at index 0) is purely path separators — preserve as `/`*run.
            // For interior runs of length n, treat as `/` plus `.` repeated (n-1) times
            // so that `--config` becomes `/.config`.
            if start == 0 {
                for _ in 0..run {
                    out.push('/');
                }
            } else {
                out.push('/');
                for _ in 1..run {
                    out.push('.');
                }
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Convert a Unix epoch millisecond timestamp (as used by OpenCode) to a
/// `DateTime<Utc>`. Returns `None` for out-of-range values.
///
/// Do NOT use `Utc.timestamp_opt` (seconds) for OpenCode times — they are
/// milliseconds and would produce dates far in the future.
///
/// Wired into the main binary via PRD-03/04 (OpenCode metadata scan).
#[allow(dead_code)]
pub fn timestamp_from_millis(ms: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_millis_opt(ms).single()
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub project_slug: String,
    pub file: PathBuf,
    pub title: Option<String>,
    pub first_user_line: Option<String>,
    /// Sub-agent display name from the most recent `agent-name` record. Used as
    /// a label fallback for sub-agents that have no AI title or first user line.
    pub agent_name: Option<String>,
    pub started: Option<DateTime<Utc>>,
    pub last_event: Option<DateTime<Utc>>,
    pub last_mtime: Option<DateTime<Utc>>,
    pub process_open: bool,
    /// True while at least one claude process has this session's project as its
    /// CWD — even if that claude is driving a *different* session in the same
    /// project. Lets `is_session_live` distinguish "claude is running here but
    /// not on me" (closed) from "we have no process info" (timestamp fallback).
    pub project_has_claude: bool,
    /// Set to true the first time lsof sees a claude process in this session's project.
    pub process_ever_open: bool,
    /// Set when process_open transitions true→false; used for the badge color.
    pub process_closed_at: Option<DateTime<Utc>>,
    /// Set to true once we observe an explicit `/exit` (or `/quit`) command in
    /// the event stream. Definitive evidence the session ended.
    pub exit_observed: bool,
    pub events: Vec<EventRecord>,
    pub usage_totals: UsageTotals,
    pub byte_offset: u64,
    pub loaded: bool,
    pub sidechain_event_count: usize,
    pub is_background: bool,
    /// Actual working directory from the first user event's `cwd` field.
    /// More reliable than decode_slug for projects whose names contain hyphens.
    pub cwd: Option<String>,
    /// tool_use id → (event_idx, block_idx) for the originating tool_use block.
    pub tool_use_index: HashMap<String, (usize, usize)>,
    /// tool_use id → (event_idx, result_idx) for the tool_result that replied to it.
    pub tool_result_index: HashMap<String, (usize, usize)>,
    /// input_tokens from the most recent assistant turn — represents current context size.
    pub last_input_tokens: Option<u64>,
    /// Originating data source. Defaults to `Claude`.
    pub source: SourceKind,
    /// Parent session id. Set for OpenCode sub-agent sessions (those with a
    /// `parentID` field in the session JSON). Used by the sidebar to group
    /// sub-agents independently of the Claude `is_background` heuristic.
    /// Read starting with PRD-03 (is_sub_agent predicate in app.rs).
    #[allow(dead_code)]
    pub parent_id: Option<String>,
}

impl Session {
    pub fn new(id: String, project_slug: String, file: PathBuf) -> Self {
        Self {
            id,
            project_slug,
            file,
            title: None,
            first_user_line: None,
            agent_name: None,
            started: None,
            last_event: None,
            last_mtime: None,
            process_open: false,
            project_has_claude: false,
            process_ever_open: false,
            process_closed_at: None,
            exit_observed: false,
            events: Vec::new(),
            usage_totals: UsageTotals::default(),
            byte_offset: 0,
            loaded: false,
            sidechain_event_count: 0,
            is_background: false,
            cwd: None,
            tool_use_index: HashMap::new(),
            tool_result_index: HashMap::new(),
            last_input_tokens: None,
            source: SourceKind::Claude,
            parent_id: None,
        }
    }

    pub fn display_label(&self) -> String {
        if let Some(t) = &self.title {
            return t.clone();
        }
        if let Some(p) = &self.first_user_line {
            return p.clone();
        }
        if let Some(n) = &self.agent_name {
            return n.clone();
        }
        short_id(&self.id)
    }
}

pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Populate a session's `tool_use_index` / `tool_result_index` from one event
/// record at position `event_idx`. Shared by every source (Claude JSONL and
/// OpenCode message/part) so the detail/expand cross-link UI behaves identically
/// regardless of origin.
pub fn index_tools(session: &mut Session, event_idx: usize, rec: &EventRecord) {
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

#[derive(Debug, Default, Clone, Copy)]
pub struct UsageTotals {
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    /// Estimated USD cost, summed when we know the model price.
    pub cost_usd: f64,
    /// Whether any usage at all was observed.
    pub has_usage: bool,
    /// Whether any assistant message had an unknown model (so cost may be incomplete).
    pub unknown_model: bool,
}

impl UsageTotals {
    /// Accumulate token counts only (no cost computation). Used when the source
    /// provides an authoritative per-message cost that should not be estimated
    /// from a pricing table — see [`UsageTotals::add_authoritative_cost`].
    pub fn add_tokens(&mut self, u: &Usage) {
        self.input = self.input.saturating_add(u.input_tokens.unwrap_or(0));
        self.output = self.output.saturating_add(u.output_tokens.unwrap_or(0));
        self.cache_creation = self
            .cache_creation
            .saturating_add(u.cache_creation_input_tokens.unwrap_or(0));
        self.cache_read = self
            .cache_read
            .saturating_add(u.cache_read_input_tokens.unwrap_or(0));
        self.has_usage = true;
    }

    /// Add an authoritative USD cost reported by the source itself (e.g.
    /// OpenCode records the real cost per assistant message). This is preferred
    /// over a pricing-table estimate; the computed path (`add`) is only the
    /// fallback when the source reports no cost.
    pub fn add_authoritative_cost(&mut self, cost: f64) {
        self.cost_usd += cost;
    }

    pub fn add(&mut self, u: &Usage, model: Option<&str>) {
        self.input = self.input.saturating_add(u.input_tokens.unwrap_or(0));
        self.output = self.output.saturating_add(u.output_tokens.unwrap_or(0));
        self.cache_creation = self
            .cache_creation
            .saturating_add(u.cache_creation_input_tokens.unwrap_or(0));
        self.cache_read = self
            .cache_read
            .saturating_add(u.cache_read_input_tokens.unwrap_or(0));
        self.has_usage = true;
        if let Some(price) = model.and_then(model_price) {
            self.cost_usd += (u.input_tokens.unwrap_or(0) as f64) / 1_000_000.0 * price.input
                + (u.output_tokens.unwrap_or(0) as f64) / 1_000_000.0 * price.output
                + (u.cache_creation_input_tokens.unwrap_or(0) as f64) / 1_000_000.0
                    * price.cache_write
                + (u.cache_read_input_tokens.unwrap_or(0) as f64) / 1_000_000.0 * price.cache_read;
        } else if model.is_some() {
            self.unknown_model = true;
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_read: f64,
}

/// Normalize a model identifier for matching.
///
/// Lowercases the string, then strips any `<providerID>/` prefix — taking only
/// the part after the **last** `/`.  This means:
///
/// - `"openai/gpt-4o"` → `"gpt-4o"`
/// - `"opencode-go/deepseek-v4-flash"` → `"deepseek-v4-flash"`
/// - `"claude-sonnet-4-6"` (no `/`) → `"claude-sonnet-4-6"`  (no-op)
fn normalize_model_id(model: &str) -> String {
    let lower = model.to_ascii_lowercase();
    match lower.rfind('/') {
        Some(pos) => lower[pos + 1..].to_string(),
        None => lower,
    }
}

/// USD per million tokens for known model families.
///
/// Approximate rates — update as providers change pricing.
/// Sources consulted at implementation time (2026-06-10):
///   Anthropic: https://www.anthropic.com/pricing
///   OpenAI:    https://openai.com/api/pricing/
///   DeepSeek:  https://platform.deepseek.com/docs/quickstart
///   Moonshot/Kimi: https://platform.moonshot.ai/docs/pricing/chat
///   Google Gemini: https://ai.google.dev/gemini-api/docs/pricing
///
/// Be conservative: a wrong price is worse than an unknown flag. For
/// self-hosted / local endpoints (nvidia routing, opencode-go routing ids)
/// we return `None` — OpenCode records `cost: 0` for those and the
/// authoritative cost path surfaces that correctly.
pub fn model_price(model: &str) -> Option<ModelPrice> {
    let lower = model.to_ascii_lowercase();

    // Early-return for providers known to host self-served / zero-cost endpoints
    // whose prices we cannot determine. The provider prefix is significant here:
    // "nvidia/..." is a routing namespace for NVIDIA-hosted models (often local
    // or self-hosted), not a guarantee of OpenAI API rates.  "opencode-go/..." is
    // the internal OpenCode routing layer — prices vary by actual backend.
    // We check the provider prefix BEFORE stripping it.
    if let Some(slash) = lower.find('/') {
        let provider = &lower[..slash];
        if provider == "nvidia" || provider == "opencode-go" {
            return None;
        }
    }

    // Normalize: lowercase + strip provider prefix.
    let m = normalize_model_id(model);

    // --- Anthropic (approximate, update as Anthropic changes pricing) ---
    if m.contains("opus") {
        return Some(ModelPrice {
            input: 5.00,
            output: 25.00,
            cache_write: 6.25,
            cache_read: 0.50,
        });
    }
    if m.contains("sonnet") {
        return Some(ModelPrice {
            input: 3.00,
            output: 15.00,
            cache_write: 3.75,
            cache_read: 0.30,
        });
    }
    if m.contains("haiku") {
        return Some(ModelPrice {
            input: 1.00,
            output: 5.00,
            cache_write: 1.25,
            cache_read: 0.10,
        });
    }

    // --- OpenAI (approximate, update as providers change pricing) ---
    // gpt-oss: internal/hosted OpenAI models served at roughly gpt-4-class rates.
    if m.contains("gpt-oss") {
        return Some(ModelPrice {
            input: 5.00,
            output: 15.00,
            cache_write: 0.00,
            cache_read: 0.00,
        });
    }
    // gpt-4o family (gpt-4o, gpt-4o-mini uses different tier — only match non-mini here).
    if m.contains("gpt-4o-mini") {
        return Some(ModelPrice {
            input: 0.15,
            output: 0.60,
            cache_write: 0.00,
            cache_read: 0.075,
        });
    }
    if m.contains("gpt-4o") {
        return Some(ModelPrice {
            input: 2.50,
            output: 10.00,
            cache_write: 0.00,
            cache_read: 1.25,
        });
    }
    // o1 reasoning models ($15/$60 per 1M as of implementation).
    if m.contains("o1") {
        return Some(ModelPrice {
            input: 15.00,
            output: 60.00,
            cache_write: 0.00,
            cache_read: 7.50,
        });
    }
    // o3 reasoning models ($10/$40 per 1M as of implementation).
    if m.contains("o3") {
        return Some(ModelPrice {
            input: 10.00,
            output: 40.00,
            cache_write: 0.00,
            cache_read: 2.50,
        });
    }

    // --- DeepSeek (approximate, update as providers change pricing) ---
    // deepseek-chat / deepseek-v3 / deepseek-v4* — $0.27/$1.10 per 1M.
    if m.contains("deepseek") {
        return Some(ModelPrice {
            input: 0.27,
            output: 1.10,
            cache_write: 0.00,
            cache_read: 0.07,
        });
    }

    // --- Moonshot / Kimi (approximate, update as providers change pricing) ---
    // kimi-k2 and related: $0.15/$2.50 per 1M as of implementation.
    if m.contains("kimi") {
        return Some(ModelPrice {
            input: 0.15,
            output: 2.50,
            cache_write: 0.00,
            cache_read: 0.00,
        });
    }

    // --- Google Gemini (approximate, update as providers change pricing) ---
    // gemini-2.5-pro: $1.25/$10.00 per 1M as of implementation.
    // gemini-2.5-flash: $0.075/$0.30.
    // gemini-1.5-pro: $1.25/$5.00.
    // Use a conservative "gemini-flash" check first (more specific).
    if m.contains("gemini") && m.contains("flash") {
        return Some(ModelPrice {
            input: 0.075,
            output: 0.30,
            cache_write: 0.00,
            cache_read: 0.019,
        });
    }
    if m.contains("gemini") {
        return Some(ModelPrice {
            input: 1.25,
            output: 10.00,
            cache_write: 0.00,
            cache_read: 0.31,
        });
    }

    // --- Llama / self-hosted (price unknown; return None — correct & honest) ---
    // These include nvidia-hosted, meta-hosted, and opencode-go local endpoints.
    // OpenCode records cost: 0 for these; the authoritative cost path handles it.

    None
}

/// Input token limit for a given model.
///
/// Anthropic Opus 4.6+/Sonnet 4.6+ have 1M context; everything else uses a
/// sensible published window or a 200k safe default for unknowns.
pub fn model_context_window(model: &str) -> u64 {
    // Normalize: lowercase + strip provider prefix.
    let m = normalize_model_id(model);

    // Anthropic 1M rule: Opus 4.6+ and Sonnet 4.6+ — keep EXACTLY as-is.
    if (m.contains("opus") && (m.contains("4-6") || m.contains("4-7")))
        || (m.contains("sonnet") && m.contains("4-6"))
    {
        return 1_000_000;
    }

    // Other Anthropic models (matched before provider-agnostic fallback).
    if m.contains("opus") || m.contains("sonnet") || m.contains("haiku") {
        return 200_000;
    }

    // OpenAI GPT-4o / gpt-oss: 128k.
    if m.contains("gpt-4o") || m.contains("gpt-oss") {
        return 128_000;
    }
    // OpenAI o1/o3 reasoning: 128k.
    if m.contains("o1") || m.contains("o3") {
        return 128_000;
    }
    // OpenAI gpt-4 (non-4o): 8k or 128k depending on variant — use 128k as safe upper.
    if m.contains("gpt-4") {
        return 128_000;
    }

    // DeepSeek: 64k (deepseek-chat/v3/v4 context window).
    if m.contains("deepseek") {
        return 64_000;
    }

    // Kimi K2: 128k.
    if m.contains("kimi") {
        return 128_000;
    }

    // Google Gemini 2.5 Pro/Flash: 1M context.
    // Gemini 1.5 Pro: also 1M (2M extended). Use 1M as the baseline.
    if m.contains("gemini") {
        return 1_000_000;
    }

    // Llama 3.x: 128k.
    if m.contains("llama") {
        return 128_000;
    }

    // Safe default for unknown models.
    200_000
}

#[derive(Debug, Clone)]
pub struct EventRecord {
    #[allow(dead_code)] // future: full parent→sidechain tree linkage
    pub uuid: Option<String>,
    #[allow(dead_code)]
    pub parent_uuid: Option<String>,
    pub is_sidechain: bool,
    pub session_kind: Option<String>,
    pub timestamp: Option<DateTime<Utc>>,
    pub event: Event,
    pub model: Option<String>,
    /// Byte range in the source file for re-reading on detail open.
    pub file_offset: u64,
    pub file_len: u64,
}

#[derive(Debug, Clone)]
pub enum Event {
    User(UserContent),
    Assistant {
        blocks: Vec<AssistantBlock>,
        usage: Option<Usage>,
    },
    System {
        subtype: String,
        body: Value,
    },
    Attachment(#[allow(dead_code)] Value),
    AiTitle(String),
    LastPrompt(String),
    PermissionMode(String),
    /// Sub-agent display name (`agent-name` record). Used to label sub-agents
    /// that have no AI title or user-typed first line.
    AgentName(String),
    /// Session interaction mode (`mode` record), e.g. "normal", "plan".
    Mode(String),
    FileHistorySnapshot,
    Unknown(String),
}

#[derive(Debug, Clone)]
pub enum UserContent {
    Text(String),
    ToolResults(Vec<ToolResult>),
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    #[allow(dead_code)] // future: link result to the originating tool_use block
    pub tool_use_id: Option<String>,
    pub content: String,
    pub is_error: bool,
}

#[derive(Debug, Clone)]
pub enum AssistantBlock {
    Thinking {
        text: String,
    },
    Text {
        text: String,
    },
    ToolUse {
        #[allow(dead_code)] // future: cross-link with tool_result by id
        id: String,
        name: String,
        input: Value,
    },
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RawRecord {
    #[serde(rename = "type")]
    r#type: Option<String>,
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default, rename = "parentUuid")]
    parent_uuid: Option<String>,
    #[serde(default, rename = "isSidechain")]
    is_sidechain: Option<bool>,
    #[serde(default, rename = "sessionKind")]
    session_kind: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    message: Option<Value>,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    content: Option<Value>,
    #[serde(default, rename = "aiTitle")]
    ai_title: Option<String>,
    #[serde(default, rename = "lastPrompt")]
    last_prompt: Option<String>,
    #[serde(default, rename = "permissionMode")]
    permission_mode: Option<String>,
    #[serde(default, rename = "agentName")]
    agent_name: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    attachment: Option<Value>,
}

/// Parse a single JSONL line into an `EventRecord`. Returns `None` on malformed lines.
pub fn parse_line(line: &str, file_offset: u64) -> Option<EventRecord> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() {
        return None;
    }
    let raw: RawRecord = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return None,
    };
    let r#type = raw.r#type.clone().unwrap_or_default();

    let timestamp = raw
        .timestamp
        .as_deref()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|dt| dt.with_timezone(&Utc));

    let mut model: Option<String> = None;

    let event = match r#type.as_str() {
        "user" => parse_user(raw.message.as_ref()),
        "assistant" => {
            let (e, m) = parse_assistant(raw.message.as_ref());
            model = m;
            e
        }
        "system" => Event::System {
            subtype: raw.subtype.unwrap_or_default(),
            body: raw.content.unwrap_or(Value::Null),
        },
        "attachment" => Event::Attachment(raw.attachment.unwrap_or(Value::Null)),
        "ai-title" => Event::AiTitle(raw.ai_title.unwrap_or_default()),
        "last-prompt" => Event::LastPrompt(raw.last_prompt.unwrap_or_default()),
        "permission-mode" => Event::PermissionMode(raw.permission_mode.unwrap_or_default()),
        "agent-name" => Event::AgentName(raw.agent_name.unwrap_or_default()),
        "mode" => Event::Mode(raw.mode.unwrap_or_default()),
        "file-history-snapshot" => Event::FileHistorySnapshot,
        "" => Event::Unknown(String::from("?")),
        other => Event::Unknown(other.to_string()),
    };

    Some(EventRecord {
        uuid: raw.uuid,
        parent_uuid: raw.parent_uuid,
        is_sidechain: raw.is_sidechain.unwrap_or(false),
        session_kind: raw.session_kind,
        timestamp,
        event,
        model,
        file_offset,
        file_len: line.len() as u64,
    })
}

fn parse_user(message: Option<&Value>) -> Event {
    let Some(msg) = message else {
        return Event::User(UserContent::Text(String::new()));
    };
    let content = match msg.get("content") {
        Some(c) => c,
        None => return Event::User(UserContent::Text(String::new())),
    };
    match content {
        Value::String(s) => Event::User(UserContent::Text(truncate(s, MAX_FIELD_LEN))),
        Value::Array(items) => {
            let mut results = Vec::new();
            let mut text_fallback = String::new();
            for item in items {
                let t = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if t == "tool_result" {
                    let tool_use_id = item
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let is_error = item
                        .get("is_error")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let content_str = stringify_content(item.get("content"));
                    results.push(ToolResult {
                        tool_use_id,
                        content: truncate(&content_str, MAX_FIELD_LEN),
                        is_error,
                    });
                } else if t == "text" {
                    if let Some(s) = item.get("text").and_then(|v| v.as_str()) {
                        if !text_fallback.is_empty() {
                            text_fallback.push('\n');
                        }
                        text_fallback.push_str(s);
                    }
                }
            }
            if !results.is_empty() {
                Event::User(UserContent::ToolResults(results))
            } else {
                Event::User(UserContent::Text(truncate(&text_fallback, MAX_FIELD_LEN)))
            }
        }
        _ => Event::User(UserContent::Text(String::new())),
    }
}

fn parse_assistant(message: Option<&Value>) -> (Event, Option<String>) {
    let Some(msg) = message else {
        return (
            Event::Assistant {
                blocks: Vec::new(),
                usage: None,
            },
            None,
        );
    };
    let model = msg
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let mut blocks = Vec::new();
    if let Some(arr) = msg.get("content").and_then(|v| v.as_array()) {
        for item in arr {
            let t = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match t {
                "thinking" => {
                    if let Some(s) = item.get("thinking").and_then(|v| v.as_str()) {
                        blocks.push(AssistantBlock::Thinking {
                            text: truncate(s, MAX_FIELD_LEN),
                        });
                    }
                }
                "text" => {
                    if let Some(s) = item.get("text").and_then(|v| v.as_str()) {
                        blocks.push(AssistantBlock::Text {
                            text: truncate(s, MAX_FIELD_LEN),
                        });
                    }
                }
                "tool_use" => {
                    let id = item
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = item.get("input").cloned().unwrap_or(Value::Null);
                    blocks.push(AssistantBlock::ToolUse { id, name, input });
                }
                _ => {}
            }
        }
    }
    let usage = msg
        .get("usage")
        .and_then(|v| serde_json::from_value::<Usage>(v.clone()).ok());
    (Event::Assistant { blocks, usage }, model)
}

fn stringify_content(v: Option<&Value>) -> String {
    let Some(v) = v else { return String::new() };
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            let mut out = String::new();
            for item in items {
                if let Some(s) = item.get("text").and_then(|v| v.as_str()) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(s);
                } else {
                    let s = serde_json::to_string(item).unwrap_or_default();
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&s);
                }
            }
            out
        }
        other => other.to_string(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Cut on char boundary
        let mut end = max;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        let mut out = String::with_capacity(end + 3);
        out.push_str(&s[..end]);
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_from_millis_known_value() {
        // 1780688974507 ms → 2026-06-05 19:49:34 UTC (OpenCode fixture value)
        let ts = timestamp_from_millis(1_780_688_974_507).expect("should parse");
        assert_eq!(ts.timestamp_millis(), 1_780_688_974_507);
        // Year must be 2026, not year ~58000 (which seconds would give).
        assert_eq!(ts.format("%Y").to_string(), "2026");
    }

    #[test]
    fn timestamp_from_millis_zero_is_epoch() {
        let ts = timestamp_from_millis(0).expect("epoch");
        assert_eq!(ts.timestamp(), 0);
    }

    #[test]
    fn decode_slug_basic() {
        assert_eq!(decode_slug("-Users-x-src"), "/Users/x/src");
    }

    #[test]
    fn decode_slug_with_dotfile() {
        assert_eq!(decode_slug("-Users-x--config-nix"), "/Users/x/.config/nix");
    }

    #[test]
    fn parse_user_text_line() {
        let line = r#"{"type":"user","uuid":"u1","timestamp":"2026-05-22T17:19:35.133Z","message":{"role":"user","content":"hello world"}}"#;
        let r = parse_line(line, 0).unwrap();
        match r.event {
            Event::User(UserContent::Text(s)) => assert_eq!(s, "hello world"),
            _ => panic!("expected user text"),
        }
        assert_eq!(r.uuid.as_deref(), Some("u1"));
        assert!(!r.is_sidechain);
        assert!(r.timestamp.is_some());
    }

    #[test]
    fn parse_user_tool_result_line() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu_1","content":"ok","is_error":false}]}}"#;
        let r = parse_line(line, 0).unwrap();
        match r.event {
            Event::User(UserContent::ToolResults(v)) => {
                assert_eq!(v.len(), 1);
                assert_eq!(v[0].tool_use_id.as_deref(), Some("tu_1"));
                assert_eq!(v[0].content, "ok");
                assert!(!v[0].is_error);
            }
            _ => panic!("expected tool_results"),
        }
    }

    #[test]
    fn parse_assistant_blocks_and_usage() {
        let line = r#"{"type":"assistant","message":{"model":"claude-sonnet-4-6","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"yo"},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}],"usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":5}}}"#;
        let r = parse_line(line, 0).unwrap();
        let Event::Assistant { blocks, usage } = r.event else {
            panic!("expected assistant");
        };
        assert_eq!(blocks.len(), 3);
        let u = usage.expect("usage");
        assert_eq!(u.input_tokens, Some(10));
        assert_eq!(u.output_tokens, Some(20));
        assert_eq!(u.cache_read_input_tokens, Some(5));
        assert_eq!(r.model.as_deref(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn parse_agent_name_line() {
        let line = r#"{"type":"agent-name","agentName":"sidebar-collapse-min-size-rounded","sessionId":"s1"}"#;
        let r = parse_line(line, 0).unwrap();
        match r.event {
            Event::AgentName(n) => assert_eq!(n, "sidebar-collapse-min-size-rounded"),
            _ => panic!("expected agent-name"),
        }
    }

    #[test]
    fn parse_mode_line() {
        let line = r#"{"type":"mode","mode":"normal","sessionId":"s1"}"#;
        let r = parse_line(line, 0).unwrap();
        match r.event {
            Event::Mode(m) => assert_eq!(m, "normal"),
            _ => panic!("expected mode"),
        }
    }

    #[test]
    fn agent_name_used_as_label_fallback() {
        let mut s = Session::new(
            "abcd1234efgh".into(),
            "slug".into(),
            PathBuf::from("/tmp/x"),
        );
        // No title, no first user line → falls back to short_id.
        assert_eq!(s.display_label(), "abcd1234");
        s.agent_name = Some("my-subagent".into());
        // agent_name takes precedence over the short id.
        assert_eq!(s.display_label(), "my-subagent");
        // ...but an explicit title still wins.
        s.title = Some("Real Title".into());
        assert_eq!(s.display_label(), "Real Title");
    }

    #[test]
    fn parse_unknown_type_does_not_panic() {
        let line = r#"{"type":"some-future-event","whatever":true}"#;
        let r = parse_line(line, 0).unwrap();
        match r.event {
            Event::Unknown(s) => assert_eq!(s, "some-future-event"),
            _ => panic!("expected unknown"),
        }
    }

    #[test]
    fn parse_empty_or_garbage_line_returns_none() {
        assert!(parse_line("", 0).is_none());
        assert!(parse_line("not json", 0).is_none());
    }

    #[test]
    fn usage_totals_compute_cost_sonnet() {
        let mut t = UsageTotals::default();
        let u = Usage {
            input_tokens: Some(1_000_000),
            output_tokens: Some(1_000_000),
            cache_creation_input_tokens: Some(0),
            cache_read_input_tokens: Some(0),
        };
        t.add(&u, Some("claude-sonnet-4-6"));
        // 3.00 + 15.00 = 18.00
        assert!((t.cost_usd - 18.0).abs() < 1e-6);
        assert!(t.has_usage);
    }

    #[test]
    fn usage_totals_unknown_model_flag() {
        let mut t = UsageTotals::default();
        let u = Usage {
            input_tokens: Some(100),
            output_tokens: Some(50),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        };
        t.add(&u, Some("some-future-model"));
        assert!(t.unknown_model);
        assert_eq!(t.cost_usd, 0.0);
    }

    // --- PRD-05: multi-provider pricing tests ---

    // Helper: run model_price and assert input/output rates match within tolerance.
    fn assert_price(model: &str, expected_in: f64, expected_out: f64) {
        let p = model_price(model).unwrap_or_else(|| panic!("expected price for {model}"));
        assert!(
            (p.input - expected_in).abs() < 1e-9,
            "input price mismatch for {model}: got {}, want {expected_in}",
            p.input
        );
        assert!(
            (p.output - expected_out).abs() < 1e-9,
            "output price mismatch for {model}: got {}, want {expected_out}",
            p.output
        );
    }

    // Anthropic regressions — these must be byte-for-byte unchanged.
    #[test]
    fn anthropic_opus_price_regression() {
        assert_price("claude-opus-4-6", 5.00, 25.00);
    }

    #[test]
    fn anthropic_sonnet_price_regression() {
        assert_price("claude-sonnet-4-6", 3.00, 15.00);
    }

    #[test]
    fn anthropic_haiku_price_regression() {
        assert_price("claude-haiku-3-5", 1.00, 5.00);
    }

    // Provider-prefixed Anthropic ids must still resolve (e.g. if OpenCode ever
    // emits "anthropic/claude-sonnet-4-6").
    #[test]
    fn anthropic_sonnet_with_provider_prefix() {
        assert_price("anthropic/claude-sonnet-4-6", 3.00, 15.00);
    }

    // OpenAI: gpt-4o family.
    #[test]
    fn openai_gpt4o_price() {
        assert_price("openai/gpt-4o", 2.50, 10.00);
    }

    // OpenAI: gpt-4o bare id.
    #[test]
    fn openai_gpt4o_bare() {
        assert_price("gpt-4o", 2.50, 10.00);
    }

    // OpenAI: gpt-oss (internal/hosted variant).
    #[test]
    fn openai_gpt_oss_price() {
        assert_price("openai/gpt-oss-120b", 5.00, 15.00);
    }

    // OpenAI: o1/o3 reasoning models.
    #[test]
    fn openai_o3_price() {
        // o3 — $10/$40 per 1M
        assert_price("openai/o3", 10.00, 40.00);
    }

    #[test]
    fn openai_o1_price() {
        assert_price("openai/o1", 15.00, 60.00);
    }

    // DeepSeek: deepseek-v3 / deepseek-v4-flash etc.
    // "opencode-go" is a routing namespace → None (self-hosted, price unknown).
    #[test]
    fn deepseek_opencode_go_provider_returns_none() {
        // opencode-go is an internal routing layer; prices vary by actual backend.
        assert!(model_price("opencode-go/deepseek-v4-flash").is_none());
    }

    #[test]
    fn deepseek_price_via_deepseek_provider() {
        // deepseek-prefixed provider should resolve to deepseek pricing.
        assert_price("deepseek/deepseek-chat", 0.27, 1.10);
    }

    #[test]
    fn deepseek_bare_id() {
        assert_price("deepseek-chat", 0.27, 1.10);
    }

    // Kimi / Moonshot.
    #[test]
    fn kimi_price() {
        assert_price("moonshotai/kimi-k2.6", 0.15, 2.50);
    }

    // Gemini.
    #[test]
    fn gemini_price() {
        assert_price("google/gemini-2.5-pro", 1.25, 10.00);
    }

    // Llama — we deliberately return None (self-hosted, price unknown).
    #[test]
    fn llama_returns_none() {
        assert!(model_price("nvidia/llama-3.1-70b").is_none());
        assert!(model_price("meta/llama-3").is_none());
    }

    // opencode-go bare routing ids with no known provider price → None.
    #[test]
    fn unknown_opencode_model_returns_none() {
        assert!(model_price("opencode-go/some-local-model").is_none());
    }

    // --- Context window tests ---

    // Anthropic 1M rule must still fire.
    #[test]
    fn context_window_opus_4_6_is_1m() {
        assert_eq!(model_context_window("claude-opus-4-6"), 1_000_000);
    }

    #[test]
    fn context_window_sonnet_4_6_is_1m() {
        assert_eq!(model_context_window("claude-sonnet-4-6"), 1_000_000);
    }

    // GPT-4o: 128k.
    #[test]
    fn context_window_gpt4o() {
        assert_eq!(model_context_window("openai/gpt-4o"), 128_000);
    }

    // Gemini 2.5 Pro: 1M context.
    #[test]
    fn context_window_gemini_25_pro() {
        assert_eq!(model_context_window("google/gemini-2.5-pro"), 1_000_000);
    }

    // DeepSeek: 64k.
    #[test]
    fn context_window_deepseek() {
        assert_eq!(model_context_window("deepseek-chat"), 64_000);
    }

    // Kimi K2: 128k.
    #[test]
    fn context_window_kimi() {
        assert_eq!(model_context_window("moonshotai/kimi-k2.6"), 128_000);
    }

    // Unknown model falls back to 200k safe default.
    #[test]
    fn context_window_unknown_defaults_200k() {
        assert_eq!(model_context_window("some-mystery-model"), 200_000);
    }

    // --- Authoritative cost path regression (PRD-04) ---
    // When add_authoritative_cost is used, the pricing table is never consulted
    // and unknown_model stays false.
    #[test]
    fn authoritative_cost_path_unaffected_by_pricing_table() {
        let mut t = UsageTotals::default();
        let u = Usage {
            input_tokens: Some(1_000_000),
            output_tokens: Some(1_000_000),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        };
        // add_tokens accumulates counts without touching cost or unknown_model.
        t.add_tokens(&u);
        t.add_authoritative_cost(7.42);
        assert!((t.cost_usd - 7.42).abs() < 1e-9, "cost should be 7.42");
        assert!(
            !t.unknown_model,
            "unknown_model must not be set by authoritative cost path"
        );
        assert!(t.has_usage);
    }
}
