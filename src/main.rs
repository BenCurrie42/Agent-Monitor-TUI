mod app;
mod data;
mod store;
mod ui;
mod watcher;

use std::collections::HashSet;
use std::io;
use std::panic;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use crossbeam_channel::{select, tick, unbounded};
use crossterm::event::{self, Event as CtEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::app::{AppEvent, AppState};
use crate::store::Store;
use crate::watcher::spawn_watcher;

#[derive(Parser, Debug)]
#[command(
    name = "agentmonitor",
    about = "AgentMonitorTUI — passive, lazydocker-style TUI for Claude Code sessions",
    version
)]
struct Args {
    /// Override the projects directory (defaults to ~/.claude/projects)
    #[arg(long)]
    projects_dir: Option<PathBuf>,

    /// Preselect a session by UUID on launch
    #[arg(long)]
    session: Option<String>,

    /// Start with auto-scroll (follow tail) disabled
    #[arg(long)]
    no_follow: bool,

    /// Print debug logs to stderr
    #[arg(long)]
    debug: bool,

    /// Headless: scan projects, print a summary, and exit (no TUI).
    #[arg(long)]
    dump: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let projects_dir = match args.projects_dir.clone() {
        Some(p) => p,
        None => default_projects_dir().context("locating ~/.claude/projects")?,
    };

    if !projects_dir.is_dir() {
        anyhow::bail!(
            "projects dir does not exist or is not a directory: {}",
            projects_dir.display()
        );
    }

    if args.dump {
        return run_dump(&projects_dir, args.session.clone());
    }

    install_panic_hook();
    enable_raw_mode().context("enabling raw mode")?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        SetTitle("AgentMonitorTUI")
    )
    .context("entering alt screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("creating terminal")?;

    let run_result = run(&mut terminal, &args, &projects_dir);

    // Always restore terminal, even on error.
    disable_raw_mode().ok();
    execute!(io::stdout(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    run_result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    args: &Args,
    projects_dir: &PathBuf,
) -> Result<()> {
    let mut store = Store::new(projects_dir.clone());
    store.initial_scan().context("initial projects scan")?;

    let mut app = AppState::new(!args.no_follow);
    if let Some(q) = &args.session {
        match resolve_session_id(&store, q) {
            Ok(sid) => app.preselect_session(&mut store, &sid),
            Err(_) => app.select_first(&mut store),
        }
    } else {
        app.select_first(&mut store);
    }

    let (tx, rx) = unbounded::<AppEvent>();

    // Initial open-file check (synchronous, before first render).
    store.apply_open_files(&claude_open_files());

    // Background open-file checker: re-checks every 5s via lsof.
    {
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            if tx.send(AppEvent::OpenFiles(claude_open_files())).is_err() {
                return;
            }
        });
    }

    // Input poll thread
    {
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            if let Ok(true) = event::poll(Duration::from_millis(250)) {
                if let Ok(ev) = event::read() {
                    match ev {
                        CtEvent::Key(k) if k.kind != KeyEventKind::Release => {
                            if tx.send(AppEvent::Key(k)).is_err() {
                                return;
                            }
                        }
                        CtEvent::Resize(_, _) => {
                            if tx.send(AppEvent::Resize).is_err() {
                                return;
                            }
                        }
                        _ => {}
                    }
                }
            }
        });
    }

    // FS watcher thread
    let _watcher_handle = spawn_watcher(projects_dir.clone(), tx.clone(), args.debug)
        .context("starting file watcher")?;

    // Render tick (for live-indicator freshness)
    let ticker = tick(Duration::from_millis(500));

    // First render
    app.resolve_selection(&mut store);
    terminal.draw(|f| ui::render(f, &store, &mut app))?;

    let mut last_draw = Instant::now();

    loop {
        let dirty;
        select! {
            recv(rx) -> msg => {
                dirty = match msg {
                    Ok(AppEvent::Key(k)) => {
                        if app.handle_key(k, &mut store) {
                            return Ok(());
                        }
                        true
                    }
                    Ok(AppEvent::Resize) => true,
                    Ok(AppEvent::Fs(fs_ev)) => {
                        store.apply_fs_event(fs_ev, args.debug);
                        true
                    }
                    Ok(AppEvent::OpenFiles(paths)) => {
                        store.apply_open_files(&paths);
                        true
                    }
                    Err(_) => return Ok(()),
                };
            }
            recv(ticker) -> _ => { dirty = true; }
        }

        // Coalesce: avoid drawing more than ~60fps
        if dirty && last_draw.elapsed() >= Duration::from_millis(16) {
            app.resolve_selection(&mut store);
            terminal.draw(|f| ui::render(f, &store, &mut app))?;
            last_draw = Instant::now();
        }
    }
}

fn default_projects_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not determine home directory")?;
    Ok(home.join(".claude").join("projects"))
}

fn run_dump(projects_dir: &PathBuf, session_id: Option<String>) -> Result<()> {
    let mut store = Store::new(projects_dir.clone());
    store.initial_scan().context("scan")?;
    println!(
        "{} project(s), {} session(s) in {}",
        store.projects.len(),
        store.sessions.len(),
        projects_dir.display()
    );
    for slug in store.project_order_by_recency() {
        let Some(proj) = store.projects.get(&slug) else { continue };
        let display = crate::data::decode_slug(&slug);
        println!("  {} — {} session(s)", display, proj.sessions.len());
        for sid in proj.sessions.iter().take(5) {
            if let Some(s) = store.sessions.get(sid) {
                let last = s
                    .last_event
                    .or(s.last_mtime)
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| "—".to_string());
                println!(
                    "    {} {:<60.60} last={}",
                    crate::data::short_id(&s.id),
                    s.display_label(),
                    last
                );
            }
        }
        if proj.sessions.len() > 5 {
            println!("    … {} more", proj.sessions.len() - 5);
        }
    }
    if let Some(query) = session_id {
        let sid = resolve_session_id(&store, &query)
            .with_context(|| format!("resolving session {query}"))?;
        store
            .ensure_loaded(&sid)
            .with_context(|| format!("loading session {sid}"))?;
        let Some(s) = store.sessions.get(&sid) else {
            anyhow::bail!("session {sid} not found");
        };
        println!(
            "\n--- session {} ({} events) ---",
            sid,
            s.events.len()
        );
        let totals = &s.usage_totals;
        if totals.has_usage {
            println!(
                "tokens in/out: {}/{}  cache w/r: {}/{}  cost: ${:.4}{}",
                totals.input,
                totals.output,
                totals.cache_creation,
                totals.cache_read,
                totals.cost_usd,
                if totals.unknown_model { "*" } else { "" }
            );
        } else {
            println!("tokens: n/a");
        }
        println!("sidechain events: {}", s.sidechain_event_count);
        for (i, rec) in s.events.iter().enumerate().take(20) {
            let ts = rec
                .timestamp
                .map(|t| t.format("%H:%M:%S").to_string())
                .unwrap_or_else(|| "        ".to_string());
            let sc = if rec.is_sidechain { "↳" } else { " " };
            let kind = match &rec.event {
                crate::data::Event::User(_) => "user",
                crate::data::Event::Assistant { .. } => "assistant",
                crate::data::Event::System { subtype, .. } => return_str_pad("system:", subtype),
                crate::data::Event::AiTitle(_) => "ai-title",
                crate::data::Event::LastPrompt(_) => "last-prompt",
                crate::data::Event::PermissionMode(_) => "permission-mode",
                crate::data::Event::Attachment(_) => "attachment",
                crate::data::Event::FileHistorySnapshot => "file-history-snapshot",
                crate::data::Event::Unknown(t) => return_str_pad("?:", t),
            };
            println!("  {:>4}. {} {} {}", i, ts, sc, kind);
        }
        if s.events.len() > 20 {
            println!("  … {} more", s.events.len() - 20);
        }
    }
    Ok(())
}

// Tiny helper: produce a leaked str so we can return a borrow above without lifetimes.
fn return_str_pad(prefix: &str, value: &str) -> &'static str {
    let s = format!("{prefix}{value}");
    Box::leak(s.into_boxed_str())
}

/// Resolve a session id query against the store. Accepts a full UUID or a
/// unique prefix (>= 4 chars).
fn resolve_session_id(store: &Store, query: &str) -> Result<String> {
    if store.sessions.contains_key(query) {
        return Ok(query.to_string());
    }
    if query.len() < 4 {
        anyhow::bail!("session id query too short (need >= 4 chars)");
    }
    let matches: Vec<&String> = store
        .sessions
        .keys()
        .filter(|k| k.starts_with(query))
        .collect();
    match matches.len() {
        0 => anyhow::bail!("no session matching '{query}'"),
        1 => Ok(matches[0].clone()),
        n => anyhow::bail!("{n} sessions match prefix '{query}'; disambiguate"),
    }
}

/// Returns the set of JSONL file paths currently held open by any `claude` process.
/// Uses `lsof -F n -c claude`; returns an empty set if lsof is unavailable or fails.
fn claude_open_files() -> HashSet<PathBuf> {
    let Ok(out) = std::process::Command::new("lsof")
        .args(["-F", "n", "-c", "claude"])
        .output()
    else {
        return HashSet::new();
    };
    let Ok(stdout) = std::str::from_utf8(&out.stdout) else {
        return HashSet::new();
    };
    stdout
        .lines()
        .filter_map(|l| l.strip_prefix('n'))
        .filter(|p| p.ends_with(".jsonl"))
        .map(PathBuf::from)
        .collect()
}

fn install_panic_hook() {
    let original = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        disable_raw_mode().ok();
        execute!(io::stdout(), LeaveAlternateScreen).ok();
        original(info);
    }));
}
