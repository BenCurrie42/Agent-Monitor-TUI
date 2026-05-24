use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

const MAX_FIELD_LEN: usize = 4096;

#[derive(Debug, Clone)]
pub struct Project {
    #[allow(dead_code)] // keyed in the store by the same slug; kept for symmetry/future use
    pub slug: String,
    #[allow(dead_code)] // disk path; future: open-in-editor, etc.
    pub path: PathBuf,
    pub display_path: String,
    pub sessions: Vec<String>,
}

impl Project {
    pub fn new(slug: String, path: PathBuf) -> Self {
        let display_path = decode_slug(&slug);
        Self {
            slug,
            path,
            display_path,
            sessions: Vec::new(),
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

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub project_slug: String,
    pub file: PathBuf,
    pub title: Option<String>,
    pub first_user_line: Option<String>,
    pub started: Option<DateTime<Utc>>,
    pub last_event: Option<DateTime<Utc>>,
    pub last_mtime: Option<DateTime<Utc>>,
    pub process_open: bool,
    pub events: Vec<EventRecord>,
    pub usage_totals: UsageTotals,
    pub byte_offset: u64,
    pub loaded: bool,
    pub sidechain_event_count: usize,
    pub is_background: bool,
    /// tool_use id → (event_idx, block_idx) for the originating tool_use block.
    pub tool_use_index: HashMap<String, (usize, usize)>,
    /// tool_use id → (event_idx, result_idx) for the tool_result that replied to it.
    pub tool_result_index: HashMap<String, (usize, usize)>,
}

impl Session {
    pub fn new(id: String, project_slug: String, file: PathBuf) -> Self {
        Self {
            id,
            project_slug,
            file,
            title: None,
            first_user_line: None,
            started: None,
            last_event: None,
            last_mtime: None,
            process_open: false,
            events: Vec::new(),
            usage_totals: UsageTotals::default(),
            byte_offset: 0,
            loaded: false,
            sidechain_event_count: 0,
            is_background: false,
            tool_use_index: HashMap::new(),
            tool_result_index: HashMap::new(),
        }
    }

    pub fn display_label(&self) -> String {
        if let Some(t) = &self.title {
            return t.clone();
        }
        if let Some(p) = &self.first_user_line {
            return p.clone();
        }
        short_id(&self.id)
    }
}

pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
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
                + (u.cache_read_input_tokens.unwrap_or(0) as f64) / 1_000_000.0
                    * price.cache_read;
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

/// USD per million tokens. Approximate, update as Anthropic changes pricing.
pub fn model_price(model: &str) -> Option<ModelPrice> {
    let m = model.to_ascii_lowercase();
    if m.contains("opus") {
        Some(ModelPrice {
            input: 15.00,
            output: 75.00,
            cache_write: 18.75,
            cache_read: 1.50,
        })
    } else if m.contains("sonnet") {
        Some(ModelPrice {
            input: 3.00,
            output: 15.00,
            cache_write: 3.75,
            cache_read: 0.30,
        })
    } else if m.contains("haiku") {
        Some(ModelPrice {
            input: 1.00,
            output: 5.00,
            cache_write: 1.25,
            cache_read: 0.10,
        })
    } else {
        None
    }
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
        "file-history-snapshot" => Event::FileHistorySnapshot,
        other if other.is_empty() => Event::Unknown(String::from("?")),
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
        out.push_str("…");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
