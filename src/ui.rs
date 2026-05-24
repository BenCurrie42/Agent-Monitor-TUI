use chrono::{DateTime, Utc};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use serde_json::Value;

use crate::app::{
    stream_items, sidebar_rows, AppState, DetailView, Focus, Mode, SidebarRow, StreamItem,
};
use crate::store::is_session_live;
use crate::data::{
    decode_slug, short_id, AssistantBlock, Event, EventRecord, Session, ToolResult, UserContent,
};
use crate::store::Store;

pub fn render(f: &mut Frame, store: &Store, app: &mut AppState) {
    let area = f.area();
    f.render_widget(Clear, area);
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(outer[1]);

    render_session_info(f, store, app, outer[0]);
    render_sidebar(f, store, app, bottom[0]);
    render_stream(f, store, app, bottom[1]);
    render_statusline(f, app, outer[2]);

    if app.mode == Mode::Detail {
        render_detail_modal(f, store, app, area);
    }
    if app.mode == Mode::Filter {
        render_filter_overlay(f, app, area);
    }
    if app.mode == Mode::Help {
        render_help_modal(f, area);
    }
    if app.mode == Mode::DeleteConfirm {
        render_delete_confirm_modal(f, store, area);
    }
}

fn render_sidebar(f: &mut Frame, store: &Store, app: &AppState, area: Rect) {
    let rows = sidebar_rows(store, &app.expanded);
    let mut items: Vec<ListItem> = Vec::with_capacity(rows.len());
    for row in &rows {
        match row {
            SidebarRow::Project { slug, closed } => {
                let proj = store.projects.get(slug);
                // Count only what's visible in this section (live or closed).
                let n = proj
                    .map(|p| {
                        p.sessions
                            .iter()
                            .filter(|sid| {
                                store
                                    .sessions
                                    .get(*sid)
                                    .map(|s| is_session_live(s) != *closed)
                                    .unwrap_or(false)
                            })
                            .count()
                    })
                    .unwrap_or(0);
                let expanded = app.expanded.contains(slug);
                let chevron = if expanded { "▼" } else { "▶" };
                let display = proj
                    .map(|p| p.display_path.clone())
                    .unwrap_or_else(|| decode_slug(slug));
                let name = project_short_name(&display);
                let name_style = if *closed {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().add_modifier(Modifier::BOLD)
                };
                let line = Line::from(vec![
                    Span::raw(format!("{chevron} ")),
                    Span::styled(name, name_style),
                    Span::styled(
                        format!("  ({n})"),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]);
                items.push(ListItem::new(line));
            }
            SidebarRow::Session {
                session_id, closed, ..
            } => {
                let s = store.sessions.get(session_id);
                let label = s
                    .map(|s| s.display_label())
                    .unwrap_or_else(|| short_id(session_id));
                let live_color = s.map(liveness_color).unwrap_or(Color::DarkGray);
                let sidechain = s.map(|s| s.sidechain_event_count).unwrap_or(0);
                let bullet = if *closed { "○" } else { "●" };
                let label_style = if *closed {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                };
                let mut spans = vec![
                    Span::raw("  "),
                    Span::styled(bullet, Style::default().fg(live_color)),
                    Span::raw(" "),
                    Span::styled(
                        truncate_line(&label, area.width.saturating_sub(10) as usize),
                        label_style,
                    ),
                ];
                if sidechain > 0 {
                    spans.push(Span::styled(
                        format!("  ↳{sidechain}"),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                items.push(ListItem::new(Line::from(spans)));
            }
            SidebarRow::ClosedHeader {
                project_count,
                session_count,
                expanded,
            } => {
                let chevron = if *expanded { "▼" } else { "▶" };
                let line = Line::from(vec![
                    Span::raw(format!("{chevron} ")),
                    Span::styled(
                        "Closed",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  ({session_count} in {project_count})"),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]);
                items.push(ListItem::new(line));
            }
            SidebarRow::SubAgentHeader { session_count, expanded } => {
                let chevron = if *expanded { "▼" } else { "▶" };
                let line = Line::from(vec![
                    Span::raw(format!("{chevron} ")),
                    Span::styled(
                        "Sub-agents",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  ({session_count})"),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]);
                items.push(ListItem::new(line));
            }
            SidebarRow::DeleteClosedRow => {
                let line = Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        "[D] Delete all closed",
                        Style::default().fg(Color::DarkGray),
                    ),
                ]);
                items.push(ListItem::new(line));
            }
        }
    }

    let title = if app.focus == Focus::Sidebar {
        " Sessions  [Tab→stream] "
    } else {
        " Sessions "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style(app.focus == Focus::Sidebar))
        .title(title);
    let mut state = ListState::default();
    let max = rows.len().saturating_sub(1);
    let cursor = app.sidebar_cursor.min(max);
    state.select(if rows.is_empty() { None } else { Some(cursor) });
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_widget(Clear, area);
    f.render_stateful_widget(list, area, &mut state);
}

fn render_session_info(f: &mut Frame, store: &Store, app: &AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Session Info ");
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);

    let session = app
        .selected_session
        .as_ref()
        .and_then(|sid| store.sessions.get(sid));

    let lines: Vec<Line> = match session {
        Some(s) => session_info_lines(s),
        None => vec![Line::from(Span::styled(
            "No session selected.",
            Style::default().fg(Color::DarkGray),
        ))],
    };
    let p = Paragraph::new(lines).wrap(Wrap { trim: false });
    f.render_widget(p, inner);
}

fn session_info_lines(s: &Session) -> Vec<Line<'_>> {
    let label = Style::default().fg(Color::DarkGray);
    let title = s.title.clone().unwrap_or_else(|| s.display_label());
    let proj = decode_slug(&s.project_slug);

    let live_span = match liveness(s) {
        Liveness::Live => Span::styled("● live", Style::default().fg(Color::Green)),
        Liveness::Recent => Span::styled("● recent", Style::default().fg(Color::Yellow)),
        Liveness::Cold => Span::styled("○ cold", Style::default().fg(Color::DarkGray)),
    };
    let last = s
        .last_event
        .or(s.last_mtime)
        .map(relative_time)
        .unwrap_or_else(|| "—".to_string());

    // Line 1: title + liveness badge + last-active time
    let line1 = Line::from(vec![
        Span::styled(title, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        live_span,
        Span::styled(format!("  · {last}"), label),
    ]);

    // Line 2: cost, tokens, model
    let (cost_part, tok_part, model_part) = if s.usage_totals.has_usage {
        let cost = if s.usage_totals.cost_usd > 0.0 || !s.usage_totals.unknown_model {
            format!("${:.4}", s.usage_totals.cost_usd)
        } else {
            "—".to_string()
        };
        let total = s.usage_totals.input + s.usage_totals.output;
        let cache = s.usage_totals.cache_read + s.usage_totals.cache_creation;
        let tok = if cache > 0 {
            format!("{} + {}c", format_count(total), format_count(cache))
        } else {
            format_count(total)
        };
        let model = s
            .events
            .iter()
            .rev()
            .find_map(|r| r.model.as_deref())
            .map(model_short_name)
            .unwrap_or_else(|| "—".to_string());
        (cost, tok, model)
    } else {
        ("—".to_string(), "—".to_string(), "—".to_string())
    };
    let line2 = Line::from(vec![
        Span::styled("Cost: ", label),
        Span::raw(cost_part),
        Span::styled("   Tokens: ", label),
        Span::raw(tok_part),
        Span::styled("   Model: ", label),
        Span::raw(model_part),
    ]);

    // Line 3: project + session ID + started
    let started = s
        .started
        .map(relative_time)
        .unwrap_or_else(|| "—".to_string());
    let line3 = Line::from(vec![
        Span::styled("Project: ", label),
        Span::raw(project_short_name(&proj)),
        Span::styled("   ID: ", label),
        Span::raw(short_id(&s.id)),
        Span::styled("   Started: ", label),
        Span::raw(started),
    ]);

    vec![line1, line2, line3]
}

fn model_short_name(model: &str) -> String {
    // Shorten "claude-sonnet-4-6" → "sonnet-4-6", etc.
    model
        .strip_prefix("claude-")
        .unwrap_or(model)
        .to_string()
}


fn render_stream(f: &mut Frame, store: &Store, app: &mut AppState, area: Rect) {
    let mut title = String::from(" Events ");
    if !app.filter.is_empty() {
        title = format!(" Events  [filter: {}] ", app.filter);
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style(app.focus == Focus::Stream))
        .title(title);

    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);

    let Some(sid) = &app.selected_session else {
        f.render_widget(
            Paragraph::new(Span::styled(
                "Select a session in the sidebar.",
                Style::default().fg(Color::DarkGray),
            )),
            inner,
        );
        return;
    };
    let Some(session) = store.sessions.get(sid) else {
        return;
    };

    let items = stream_items(session, &app.filter, app.show_meta);
    if items.is_empty() {
        let msg = if !app.filter.is_empty() {
            format!("No events match \"{}\". Esc to clear.", app.filter)
        } else if session.events.is_empty() {
            "No events yet — waiting for activity…".to_string()
        } else if !app.show_meta {
            "Nothing here. Press v to show hidden meta events.".to_string()
        } else {
            "Nothing here.".to_string()
        };
        f.render_widget(
            Paragraph::new(Span::styled(msg, Style::default().fg(Color::DarkGray))),
            inner,
        );
        return;
    }
    let max_rows = inner.height as usize;
    let total = items.len();
    let cursor = if app.follow && total > 0 {
        total.saturating_sub(1)
    } else {
        app.stream_cursor.min(total.saturating_sub(1).max(0))
    };

    // Viewport invariant: keep cursor visible, otherwise leave viewport alone.
    // The cursor moves *within* the viewport until it hits an edge, then the
    // viewport scrolls by exactly enough to keep the cursor on-screen.
    let mut viewport = app.stream_viewport;
    let max_viewport = total.saturating_sub(max_rows);
    if viewport > max_viewport {
        viewport = max_viewport;
    }
    if cursor < viewport {
        viewport = cursor;
    } else if max_rows > 0 && cursor >= viewport + max_rows {
        viewport = cursor + 1 - max_rows;
    }
    app.stream_viewport = viewport;
    let start = viewport;

    let mut rendered: Vec<ListItem> = Vec::with_capacity(max_rows);
    for (visible_i, item) in items.iter().enumerate().skip(start).take(max_rows) {
        let line = summarize_item(session, item);
        let mut li = ListItem::new(line);
        if is_error_item(session, item) {
            li = li.style(Style::default().fg(Color::Red));
        }
        if visible_i == cursor {
            li = li.style(Style::default().add_modifier(Modifier::REVERSED));
        }
        rendered.push(li);
    }

    let list = List::new(rendered);
    f.render_widget(list, inner);
}

fn is_error_item(session: &Session, item: &StreamItem) -> bool {
    let Some(rec) = session.events.get(item.event_idx) else {
        return false;
    };
    if let (Event::User(UserContent::ToolResults(rs)), Some(r)) = (&rec.event, item.sub_idx) {
        return rs.get(r).map(|tr| tr.is_error).unwrap_or(false);
    }
    false
}

fn render_statusline(f: &mut Frame, app: &AppState, area: Rect) {
    f.render_widget(Clear, area);
    let hints = match app.mode {
        Mode::Normal | Mode::Help | Mode::DeleteConfirm => {
            let follow = if app.follow { "F" } else { "f" };
            let meta = if app.show_meta { "V" } else { "v" };
            format!(
                " ? help · q quit · j/k navigate · Tab · / filter · [{follow}] follow · [{meta}] meta"
            )
        }
        Mode::Detail => " j/k scroll · u/d page · g/G · R raw · Esc close".to_string(),
        Mode::Filter => " type to filter · Enter apply · Esc cancel".to_string(),
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(hints, Style::default().fg(Color::DarkGray)))),
        area,
    );
}

fn render_filter_overlay(f: &mut Frame, app: &AppState, area: Rect) {
    let h = 3;
    let w = std::cmp::min(area.width.saturating_sub(4), 60);
    let x = area.x + (area.width - w) / 2;
    let y = area.y + area.height.saturating_sub(h + 1);
    let rect = Rect { x, y, width: w, height: h };
    f.render_widget(Clear, rect);
    let block = Block::default().borders(Borders::ALL).title(" Filter ");
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let p = Paragraph::new(Line::from(vec![
        Span::raw("/ "),
        Span::styled(&app.filter_input, Style::default().fg(Color::Yellow)),
        Span::raw("  "),
        Span::styled("[Enter apply, Esc cancel]", Style::default().fg(Color::DarkGray)),
    ]));
    f.render_widget(p, inner);
}

fn render_help_modal(f: &mut Frame, area: Rect) {
    let w = std::cmp::min(area.width.saturating_sub(8), 58);
    let h: u16 = 22;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect { x, y, width: w, height: h };
    f.render_widget(Clear, rect);
    let block = Block::default().borders(Borders::ALL).title(" Help  [? / Esc to close] ");
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let dim = Style::default().fg(Color::DarkGray);
    let head = Style::default().add_modifier(Modifier::BOLD);
    let lines = vec![
        Line::from(Span::styled("Navigation", head)),
        Line::from(vec![Span::styled("  j / k / ↑ / ↓  ", dim), Span::raw("Move up / down")]),
        Line::from(vec![Span::styled("  h / l / ← / →  ", dim), Span::raw("Step out / in (l on a session = focus events)")]),
        Line::from(vec![Span::styled("  Tab             ", dim), Span::raw("Switch focus sidebar ↔ events")]),
        Line::from(vec![Span::styled("  g / G           ", dim), Span::raw("Top / bottom")]),
        Line::from(Span::raw("")),
        Line::from(Span::styled("Actions", head)),
        Line::from(vec![Span::styled("  Enter           ", dim), Span::raw("Sidebar: focus events · Events: open detail")]),
        Line::from(vec![Span::styled("  /               ", dim), Span::raw("Filter events")]),
        Line::from(vec![Span::styled("  f               ", dim), Span::raw("Toggle follow (auto-scroll)")]),
        Line::from(vec![Span::styled("  v               ", dim), Span::raw("Toggle meta events")]),
        Line::from(vec![Span::styled("  D               ", dim), Span::raw("Delete all closed sessions")]),
        Line::from(vec![Span::styled("  ?               ", dim), Span::raw("This help screen")]),
        Line::from(vec![Span::styled("  q / Ctrl-C      ", dim), Span::raw("Quit")]),
        Line::from(Span::raw("")),
        Line::from(Span::styled("Detail modal", head)),
        Line::from(vec![Span::styled("  j / k           ", dim), Span::raw("Scroll")]),
        Line::from(vec![Span::styled("  u / d           ", dim), Span::raw("Page up / down")]),
        Line::from(vec![Span::styled("  R               ", dim), Span::raw("Toggle raw JSON")]),
        Line::from(vec![Span::styled("  Esc             ", dim), Span::raw("Close")]),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_delete_confirm_modal(f: &mut Frame, store: &Store, area: Rect) {
    let closed_count = store
        .sessions
        .values()
        .filter(|s| !is_session_live(s))
        .count();
    let w = std::cmp::min(area.width.saturating_sub(8), 52);
    let h: u16 = 6;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect { x, y, width: w, height: h };
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Delete Closed Sessions ");
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let lines = vec![
        Line::from(format!(
            "Delete all {} closed sessions from disk?",
            closed_count
        )),
        Line::from(Span::styled(
            "This cannot be undone.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::raw("")),
        Line::from(vec![
            Span::styled("[y] ", Style::default().fg(Color::Red)),
            Span::raw("Yes, delete   "),
            Span::styled("[n / Esc] ", Style::default().fg(Color::DarkGray)),
            Span::raw("Cancel"),
        ]),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_detail_modal(f: &mut Frame, store: &Store, app: &mut AppState, area: Rect) {
    let inset = 4;
    let rect = Rect {
        x: area.x + inset / 2,
        y: area.y + 1,
        width: area.width.saturating_sub(inset),
        height: area.height.saturating_sub(2),
    };
    f.render_widget(Clear, rect);
    let title = match app.detail_view {
        DetailView::Pretty => " Detail  [Esc close · j/k scroll · R raw] ",
        DetailView::Raw => " Detail (raw)  [Esc close · j/k scroll · R back] ",
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let Some(sid) = &app.selected_session else { return };
    let Some(session) = store.sessions.get(sid) else { return };
    let Some(item) = current_item(session, app) else {
        f.render_widget(
            Paragraph::new(Span::styled("Nothing here.", Style::default().fg(Color::DarkGray))),
            inner,
        );
        return;
    };
    let Some(rec) = session.events.get(item.event_idx) else { return };

    let detail_view = app.detail_view;
    let scroll = &mut app.detail_scroll;
    match detail_view {
        DetailView::Raw => render_detail_raw(f, store, sid, rec, inner, scroll),
        DetailView::Pretty => render_detail_pretty(f, session, &item, rec, inner, scroll),
    }
}

fn current_item(session: &Session, app: &AppState) -> Option<StreamItem> {
    let items = stream_items(session, &app.filter, app.show_meta);
    if items.is_empty() {
        return None;
    }
    let cursor = if app.follow {
        items.len() - 1
    } else {
        app.stream_cursor.min(items.len() - 1)
    };
    Some(items[cursor])
}

fn render_detail_raw(
    f: &mut Frame,
    store: &Store,
    session_id: &str,
    rec: &EventRecord,
    area: Rect,
    scroll: &mut u16,
) {
    let raw_text = store
        .raw_line_for(session_id, rec.file_offset, rec.file_len)
        .unwrap_or_else(|| "(could not re-read source line)".to_string());
    let pretty = pretty_json(&raw_text);
    let visual_total = visual_line_count_str(&pretty, area.width);
    let clamped = clamp_scroll(*scroll, visual_total, area.height);
    *scroll = clamped;
    let p = Paragraph::new(pretty)
        .wrap(Wrap { trim: false })
        .scroll((clamped, 0));
    f.render_widget(p, area);
}

fn render_detail_pretty(
    f: &mut Frame,
    session: &Session,
    item: &StreamItem,
    rec: &EventRecord,
    area: Rect,
    scroll: &mut u16,
) {
    let lines = pretty_lines_for(session, item, rec);
    let visual_total = visual_line_count_lines(&lines, area.width);
    let clamped = clamp_scroll(*scroll, visual_total, area.height);
    *scroll = clamped;
    let p = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((clamped, 0));
    f.render_widget(p, area);
}

fn clamp_scroll(requested: u16, total: u16, height: u16) -> u16 {
    let max = total.saturating_sub(height);
    requested.min(max)
}

/// Count visual (wrapped) lines for a plain-text string at a given terminal width.
fn visual_line_count_str(text: &str, width: u16) -> u16 {
    if width == 0 {
        return 0;
    }
    let w = width as usize;
    text.lines()
        .map(|line| {
            let len = line.chars().count();
            (len.max(1) + w - 1) / w
        })
        .sum::<usize>()
        .min(u16::MAX as usize) as u16
}

/// Count visual (wrapped) lines for ratatui `Line` objects at a given terminal width.
fn visual_line_count_lines(lines: &[Line], width: u16) -> u16 {
    if width == 0 {
        return 0;
    }
    let w = width as usize;
    lines
        .iter()
        .map(|line| {
            let len = line.width();
            (len.max(1) + w - 1) / w
        })
        .sum::<usize>()
        .min(u16::MAX as usize) as u16
}

fn pretty_lines_for(
    session: &Session,
    item: &StreamItem,
    rec: &EventRecord,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    // Header: timestamp + a short event-type label.
    if let Some(ts) = rec.timestamp {
        out.push(Line::from(Span::styled(
            ts.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            Style::default().fg(Color::DarkGray),
        )));
    }

    match (&rec.event, item.sub_idx) {
        (Event::User(UserContent::Text(s)), _) => {
            out.push(header_line("USER", Color::Blue));
            out.push(Line::raw(""));
            extend_wrapped(&mut out, s);
        }
        (Event::User(UserContent::ToolResults(rs)), Some(r)) => {
            if let Some(tr) = rs.get(r) {
                render_tool_result(&mut out, session, tr);
            }
        }
        (Event::User(UserContent::ToolResults(rs)), None) => {
            for tr in rs {
                render_tool_result(&mut out, session, tr);
                out.push(Line::raw(""));
            }
        }
        (Event::Assistant { blocks, .. }, Some(b)) => {
            if let Some(blk) = blocks.get(b) {
                render_assistant_block(&mut out, session, blk);
            }
        }
        (Event::Assistant { blocks, .. }, None) => {
            for blk in blocks {
                render_assistant_block(&mut out, session, blk);
                out.push(Line::raw(""));
            }
        }
        (Event::System { subtype, body }, _) => {
            out.push(header_line(
                &format!("SYSTEM · {subtype}"),
                Color::DarkGray,
            ));
            out.push(Line::raw(""));
            extend_wrapped(&mut out, &value_preview(body));
        }
        (Event::AiTitle(t), _) => {
            out.push(header_line("AI TITLE", Color::Yellow));
            out.push(Line::raw(""));
            extend_wrapped(&mut out, t);
        }
        (Event::LastPrompt(t), _) => {
            out.push(header_line("LAST PROMPT", Color::DarkGray));
            out.push(Line::raw(""));
            extend_wrapped(&mut out, t);
        }
        (Event::PermissionMode(m), _) => {
            out.push(header_line("PERMISSION MODE", Color::DarkGray));
            out.push(Line::raw(""));
            extend_wrapped(&mut out, m);
        }
        (Event::Attachment(v), _) => {
            out.push(header_line("ATTACHMENT", Color::DarkGray));
            out.push(Line::raw(""));
            extend_wrapped(
                &mut out,
                &serde_json::to_string_pretty(v).unwrap_or_default(),
            );
        }
        (Event::FileHistorySnapshot, _) => {
            out.push(header_line("FILE HISTORY SNAPSHOT", Color::DarkGray));
        }
        (Event::Unknown(t), _) => {
            out.push(header_line(&format!("UNKNOWN · {t}"), Color::DarkGray));
        }
    }
    out
}

fn header_line(label: &str, color: Color) -> Line<'static> {
    Line::from(Span::styled(
        format!("── {label} ──"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
}

fn render_assistant_block(out: &mut Vec<Line<'static>>, session: &Session, blk: &AssistantBlock) {
    match blk {
        AssistantBlock::Thinking { text } => {
            out.push(header_line("THINKING", Color::Magenta));
            out.push(Line::raw(""));
            extend_wrapped(out, text);
        }
        AssistantBlock::Text { text } => {
            out.push(header_line("ASSISTANT", Color::Green));
            out.push(Line::raw(""));
            extend_wrapped(out, text);
        }
        AssistantBlock::ToolUse { id, name, input } => {
            render_tool_use(out, session, id, name, input);
        }
    }
}

fn render_tool_use(
    out: &mut Vec<Line<'static>>,
    session: &Session,
    id: &str,
    name: &str,
    input: &Value,
) {
    out.push(header_line(&format!("TOOL · {name}"), Color::Cyan));
    out.push(Line::raw(""));
    let command_lines = render_tool_command(name, input);
    out.extend(command_lines);
    out.push(Line::raw(""));

    if let Some((evt_idx, res_idx)) = session.tool_result_index.get(id).cloned() {
        if let Some(rec) = session.events.get(evt_idx) {
            if let Event::User(UserContent::ToolResults(rs)) = &rec.event {
                if let Some(tr) = rs.get(res_idx) {
                    let header = if tr.is_error { "ERROR" } else { "OUTPUT" };
                    let color = if tr.is_error { Color::Red } else { Color::DarkGray };
                    out.push(header_line(header, color));
                    out.push(Line::raw(""));
                    if tr.content.trim().is_empty() {
                        out.push(Line::from(Span::styled(
                            "(empty)",
                            Style::default().fg(Color::DarkGray),
                        )));
                    } else if name == "Read" {
                        let fp = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                        extend_highlighted(out, &tr.content, fp);
                    } else {
                        extend_wrapped(out, &tr.content);
                    }
                    return;
                }
            }
        }
    }
    out.push(Line::from(Span::styled(
        "(awaiting result…)",
        Style::default().fg(Color::DarkGray),
    )));
}

fn render_tool_command(name: &str, input: &Value) -> Vec<Line<'static>> {
    let str_field = |k: &str| {
        input
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    match name {
        "Bash" => {
            let cmd = str_field("command");
            let mut out = vec![Line::from(vec![
                Span::styled("$ ", Style::default().fg(Color::Yellow)),
                Span::raw(cmd),
            ])];
            let desc = str_field("description");
            if !desc.is_empty() {
                out.push(Line::from(Span::styled(
                    format!("# {desc}"),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            out
        }
        "Read" => {
            let path = str_field("file_path");
            let mut s = format!("Read {path}");
            let off = input.get("offset").and_then(|v| v.as_u64());
            let lim = input.get("limit").and_then(|v| v.as_u64());
            if off.is_some() || lim.is_some() {
                s.push_str(&format!(
                    " (offset {}, limit {})",
                    off.unwrap_or(0),
                    lim.map(|l| l.to_string()).unwrap_or_else(|| "—".into())
                ));
            }
            vec![Line::raw(s)]
        }
        "Write" => {
            let path = str_field("file_path");
            let content = str_field("content");
            let mut out = vec![Line::raw(format!("Write {path}"))];
            out.push(Line::raw(""));
            out.push(header_line("CONTENT", Color::DarkGray));
            extend_wrapped(&mut out, &content);
            out
        }
        "Edit" => {
            let path = str_field("file_path");
            let old = str_field("old_string");
            let new = str_field("new_string");
            let mut out = vec![Line::raw(format!("Edit {path}"))];
            out.push(Line::raw(""));
            out.push(Line::from(Span::styled(
                "── from ──",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
            extend_diff_lines(&mut out, &old, Color::Red);
            out.push(Line::raw(""));
            out.push(Line::from(Span::styled(
                "── to ──",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            )));
            extend_diff_lines(&mut out, &new, Color::Green);
            out
        }
        "Glob" => vec![Line::raw(format!(
            "Glob {} in {}",
            str_field("pattern"),
            input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".")
        ))],
        "Grep" => {
            let pat = str_field("pattern");
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            vec![Line::raw(format!("Grep '{pat}' in {path}"))]
        }
        "WebFetch" => vec![Line::raw(format!(
            "WebFetch {}",
            str_field("url")
        ))],
        "WebSearch" => vec![Line::raw(format!(
            "WebSearch \"{}\"",
            str_field("query")
        ))],
        "Task" | "Agent" => {
            let desc = str_field("description");
            let prompt = str_field("prompt");
            let mut out = vec![Line::raw(format!("Subagent: {desc}"))];
            if !prompt.is_empty() {
                out.push(Line::raw(""));
                out.push(header_line("PROMPT", Color::DarkGray));
                extend_wrapped(&mut out, &prompt);
            }
            out
        }
        _ => {
            let pretty = serde_json::to_string_pretty(input).unwrap_or_default();
            let mut out = Vec::new();
            extend_wrapped(&mut out, &pretty);
            out
        }
    }
}

fn render_tool_result(out: &mut Vec<Line<'static>>, session: &Session, tr: &ToolResult) {
    let mut header = if tr.is_error { "TOOL ERROR".to_string() } else { "TOOL RESULT".to_string() };
    let color = if tr.is_error { Color::Red } else { Color::DarkGray };
    let mut tool_name: Option<String> = None;
    let mut file_path: Option<String> = None;
    if let Some(tid) = &tr.tool_use_id {
        if let Some((evt_idx, blk_idx)) = session.tool_use_index.get(tid).cloned() {
            if let Some(rec) = session.events.get(evt_idx) {
                if let Event::Assistant { blocks, .. } = &rec.event {
                    if let Some(AssistantBlock::ToolUse { name, input, .. }) = blocks.get(blk_idx) {
                        header.push_str(&format!(" · {name}"));
                        tool_name = Some(name.clone());
                        file_path = input.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string());
                    }
                }
            }
        }
    }
    out.push(header_line(&header, color));
    out.push(Line::raw(""));
    if tr.content.trim().is_empty() {
        out.push(Line::from(Span::styled(
            "(empty)",
            Style::default().fg(Color::DarkGray),
        )));
    } else if tool_name.as_deref() == Some("Read") {
        extend_highlighted(out, &tr.content, file_path.as_deref().unwrap_or(""));
    } else {
        extend_wrapped(out, &tr.content);
    }
}

fn extend_wrapped(out: &mut Vec<Line<'static>>, s: &str) {
    for line in s.lines() {
        out.push(Line::raw(line.to_string()));
    }
}

static SYNTAX_SET: std::sync::OnceLock<syntect::parsing::SyntaxSet> = std::sync::OnceLock::new();
static THEME_SET: std::sync::OnceLock<syntect::highlighting::ThemeSet> = std::sync::OnceLock::new();

fn extend_highlighted(out: &mut Vec<Line<'static>>, content: &str, path: &str) {
    use syntect::easy::HighlightLines;
    use syntect::util::LinesWithEndings;

    let ss = SYNTAX_SET.get_or_init(syntect::parsing::SyntaxSet::load_defaults_newlines);
    let ts = THEME_SET.get_or_init(syntect::highlighting::ThemeSet::load_defaults);

    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let syntax = if ext.is_empty() {
        ss.find_syntax_by_path(path)
            .unwrap_or_else(|| ss.find_syntax_plain_text())
    } else {
        ss.find_syntax_by_extension(ext)
            .unwrap_or_else(|| ss.find_syntax_plain_text())
    };

    if syntax.name == "Plain Text" {
        extend_wrapped(out, content);
        return;
    }

    let theme = &ts.themes["base16-ocean.dark"];
    let mut h = HighlightLines::new(syntax, theme);

    for line in LinesWithEndings::from(content) {
        match h.highlight_line(line, ss) {
            Ok(ranges) => {
                let spans: Vec<Span<'static>> = ranges
                    .into_iter()
                    .filter_map(|(style, text)| {
                        let text = text.trim_end_matches('\n').to_string();
                        if text.is_empty() {
                            return None;
                        }
                        let c = style.foreground;
                        Some(Span::styled(
                            text,
                            Style::default().fg(Color::Rgb(c.r, c.g, c.b)),
                        ))
                    })
                    .collect();
                out.push(Line::from(spans));
            }
            Err(_) => out.push(Line::raw(line.trim_end_matches('\n').to_string())),
        }
    }
}

fn extend_diff_lines(out: &mut Vec<Line<'static>>, s: &str, color: Color) {
    let prefix = if color == Color::Red { "- " } else { "+ " };
    for line in s.lines() {
        out.push(Line::from(Span::styled(
            format!("{prefix}{line}"),
            Style::default().fg(color),
        )));
    }
}

fn pretty_json(raw: &str) -> String {
    match serde_json::from_str::<Value>(raw) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| raw.to_string()),
        Err(_) => raw.to_string(),
    }
}

fn summarize_item(session: &Session, item: &StreamItem) -> Line<'static> {
    let Some(rec) = session.events.get(item.event_idx) else {
        return Line::raw("?");
    };
    // Show timestamp only on the "first row" of a multi-row event so multi-block
    // assistant turns read as a single grouped action.
    let show_ts = matches!(item.sub_idx, None | Some(0));
    let ts = if show_ts {
        rec.timestamp
            .map(|t| t.format("%H:%M:%S").to_string())
            .unwrap_or_else(|| "        ".to_string())
    } else {
        "        ".to_string()
    };

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(
        format!("{ts}  "),
        Style::default().fg(Color::DarkGray),
    ));
    if rec.is_sidechain {
        spans.push(Span::styled(
            "└─ ",
            Style::default().fg(Color::DarkGray),
        ));
    }

    match (&rec.event, item.sub_idx) {
        (Event::Assistant { blocks, .. }, Some(b)) => {
            if let Some(blk) = blocks.get(b) {
                spans.extend(summarize_block(blk));
            }
        }
        (Event::User(UserContent::ToolResults(rs)), Some(r)) => {
            if let Some(tr) = rs.get(r) {
                let color = if tr.is_error { Color::Red } else { Color::DarkGray };
                spans.push(Span::styled("↩     ", Style::default().fg(color)));
                spans.push(Span::raw(first_line_owned(&tr.content, 200)));
            }
        }
        (Event::User(UserContent::Text(s)), _) => {
            spans.push(Span::styled("user  ", Style::default().fg(Color::Blue)));
            spans.push(Span::raw(first_line_owned(s, 200)));
        }
        (Event::System { subtype, body }, _) => {
            spans.push(Span::styled(
                format!("sys   {subtype}  "),
                Style::default().fg(Color::DarkGray),
            ));
            spans.push(Span::raw(first_line_owned(&value_preview(body), 200)));
        }
        (Event::AiTitle(t), _) => {
            spans.push(Span::styled("title ", Style::default().fg(Color::Yellow)));
            spans.push(Span::raw(first_line_owned(t, 200)));
        }
        (Event::LastPrompt(t), _) => {
            spans.push(Span::raw(format!("· last-prompt: {}", first_line_owned(t, 200))));
        }
        (Event::PermissionMode(m), _) => {
            spans.push(Span::raw(format!("· permission-mode: {m}")));
        }
        (Event::Attachment(_), _) => spans.push(Span::raw("· attachment")),
        (Event::FileHistorySnapshot, _) => spans.push(Span::raw("· file-history-snapshot")),
        (Event::Unknown(t), _) => spans.push(Span::raw(format!("· {t}"))),
        (Event::Assistant { .. }, None) => spans.push(Span::raw("· assistant (empty)")),
        (Event::User(UserContent::ToolResults(_)), None) => spans.push(Span::raw("· result")),
    }
    Line::from(spans)
}

fn summarize_block(b: &AssistantBlock) -> Vec<Span<'static>> {
    match b {
        AssistantBlock::Thinking { text } => {
            let n = text.chars().count();
            let detail = if n > 0 {
                format!("({n} chars)")
            } else {
                "(extended thinking)".to_string()
            };
            vec![
                Span::styled("think ", Style::default().fg(Color::Magenta)),
                Span::styled(detail, Style::default().fg(Color::Magenta)),
            ]
        }
        AssistantBlock::Text { text } => vec![
            Span::styled("asst  ", Style::default().fg(Color::Green)),
            Span::raw(first_line_owned(text, 200)),
        ],
        AssistantBlock::ToolUse { name, input, .. } => {
            let summary = tool_summary(name, input);
            vec![
                Span::styled("tool  ", Style::default().fg(Color::Cyan)),
                Span::styled(format!("{name}  "), Style::default().fg(Color::Cyan)),
                Span::raw(summary),
            ]
        }
    }
}

fn tool_summary(name: &str, input: &Value) -> String {
    let v = |k: &str| input.get(k).and_then(|x| x.as_str()).unwrap_or("");
    let pick = match name {
        "Bash" => v("command"),
        "Read" | "Write" | "NotebookEdit" => v("file_path"),
        "Edit" => v("file_path"),
        "Glob" => v("pattern"),
        "Grep" => v("pattern"),
        "WebFetch" | "WebSearch" => {
            let q = v("query");
            if !q.is_empty() {
                q
            } else {
                v("url")
            }
        }
        "Task" | "Agent" => v("description"),
        _ => "",
    };
    if !pick.is_empty() {
        return first_line_owned(pick, 200);
    }
    let s = serde_json::to_string(input).unwrap_or_default();
    first_line_owned(&s, 200)
}

fn value_preview(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn first_line_owned(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim_end();
    if line.chars().count() <= max {
        line.to_string()
    } else {
        let cut: String = line.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// Show the trailing 2 path segments so projects with the same leaf name
/// (e.g. multiple "scraper" dirs) don't collide visually. Falls back to the
/// leaf if only one segment is available.
fn project_short_name(display: &str) -> String {
    let segs: Vec<&str> = display
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    match segs.len() {
        0 => display.to_string(),
        1 => segs[0].to_string(),
        n => format!("{}/{}", segs[n - 2], segs[n - 1]),
    }
}

fn truncate_line(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

enum Liveness {
    Live,
    Recent,
    Cold,
}

fn liveness(s: &Session) -> Liveness {
    if s.exit_observed {
        return Liveness::Cold;
    }
    if s.process_open {
        // Process is in the project. Green if file is being actively written to,
        // yellow if quiet but still alive (e.g. user reading a response).
        let age = s
            .last_event
            .or(s.last_mtime)
            .map(|t| Utc::now().signed_duration_since(t).num_seconds())
            .unwrap_or(i64::MAX);
        if age < 30 {
            Liveness::Live
        } else {
            Liveness::Recent
        }
    } else if s.process_ever_open {
        Liveness::Cold
    } else {
        let t = s.last_event.or(s.last_mtime);
        let Some(t) = t else { return Liveness::Cold };
        let age = Utc::now().signed_duration_since(t).num_seconds();
        if age < 30 {
            Liveness::Live
        } else if age < 300 {
            Liveness::Recent
        } else {
            Liveness::Cold
        }
    }
}

fn liveness_color(s: &Session) -> Color {
    match liveness(s) {
        Liveness::Live => Color::Green,
        Liveness::Recent => Color::Yellow,
        Liveness::Cold => Color::DarkGray,
    }
}

fn relative_time(t: DateTime<Utc>) -> String {
    let now = Utc::now();
    let secs = now.signed_duration_since(t).num_seconds();
    if secs < 0 {
        return "in the future".to_string();
    }
    if secs < 60 {
        return format!("{}s ago", secs);
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m ago", mins);
    }
    let hrs = mins / 60;
    if hrs < 24 {
        return format!("{}h ago", hrs);
    }
    let days = hrs / 24;
    format!("{}d ago", days)
}

fn format_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

