mod app;
mod data;
mod sources;
mod store;
mod theme;
mod ui;
mod watcher;

use std::io;
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use crossbeam_channel::{select, tick, unbounded};
use crossterm::event::{self, Event as CtEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::app::{AppEvent, AppState, Mode};
use crate::data::SourceKind;
use crate::sources::{ClaudeSource, OpencodeSource, Source};
use crate::store::Store;
use crate::watcher::spawn_watcher;

/// Which data sources to load. The `--source` control surface; defaults to
/// `all` so a machine with both tools shows everything with no flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SourceSelection {
    /// Claude Code sessions only.
    Claude,
    /// OpenCode sessions only.
    Opencode,
    /// Every source that is present on disk.
    All,
}

#[derive(Parser, Debug)]
#[command(
    name = "agentmonitor",
    about = "AgentMonitorTUI — passive, lazydocker-style TUI for Claude Code sessions",
    version
)]
struct Args {
    /// Override the Claude projects directory (defaults to ~/.claude/projects)
    #[arg(long)]
    projects_dir: Option<PathBuf>,

    /// Override the OpenCode storage directory
    /// (defaults to $XDG_DATA_HOME/opencode/storage → ~/.local/share/opencode/storage)
    #[arg(long)]
    opencode_dir: Option<PathBuf>,

    /// Which data sources to load: claude, opencode, or all (default).
    #[arg(long, value_enum, default_value_t = SourceSelection::All)]
    source: SourceSelection,

    /// Preselect a session by UUID/prefix on launch (matches across sources)
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

    let claude_dir = match args.projects_dir.clone() {
        Some(p) => p,
        None => default_projects_dir().context("locating ~/.claude/projects")?,
    };
    let opencode_dir = match args.opencode_dir.clone() {
        Some(p) => p,
        None => default_opencode_dir().context("locating OpenCode storage dir")?,
    };

    let sources = build_sources(args.source, &claude_dir, &opencode_dir);
    if sources.is_empty() {
        // Nothing to load: report what the selection wanted and why it's missing.
        match args.source {
            SourceSelection::Claude => anyhow::bail!(
                "Claude projects dir does not exist or is not a directory: {}",
                claude_dir.display()
            ),
            SourceSelection::Opencode => anyhow::bail!(
                "OpenCode storage dir does not exist or is not a directory: {}",
                opencode_dir.display()
            ),
            SourceSelection::All => anyhow::bail!(
                "no data source found: neither {} nor {} exists",
                claude_dir.display(),
                opencode_dir.display()
            ),
        }
    }

    if args.dump {
        return run_dump(&sources, args.session.clone());
    }

    install_panic_hook();
    enable_raw_mode().context("enabling raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, SetTitle("AgentMonitorTUI"))
        .context("entering alt screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("creating terminal")?;

    let run_result = run(&mut terminal, &args, sources);

    // Always restore terminal, even on error.
    disable_raw_mode().ok();
    execute!(io::stdout(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    run_result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    args: &Args,
    sources: Vec<Arc<dyn Source>>,
) -> Result<()> {
    let mut store = Store::new(sources.clone());
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
    store.apply_open_files(&gather_active_dirs(&sources));

    // Background open-file checker: re-checks every 1s.
    {
        let tx = tx.clone();
        let sources = sources.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            if tx
                .send(AppEvent::OpenFiles(gather_active_dirs(&sources)))
                .is_err()
            {
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
                        CtEvent::Key(k)
                            if k.kind != KeyEventKind::Release
                                && tx.send(AppEvent::Key(k)).is_err() =>
                        {
                            return;
                        }
                        CtEvent::Resize(_, _) if tx.send(AppEvent::Resize).is_err() => {
                            return;
                        }
                        _ => {}
                    }
                }
            }
        });
    }

    // FS watcher thread, one per source root.
    let mut _watcher_handles = Vec::new();
    for src in &sources {
        let handle = spawn_watcher(src.root().to_path_buf(), tx.clone(), args.debug)
            .context("starting file watcher")?;
        _watcher_handles.push(handle);
    }

    // Render tick (for live-indicator freshness)
    let ticker = tick(Duration::from_millis(500));

    // First render
    app.resolve_selection(&mut store);
    terminal.draw(|f| ui::render(f, &store, &mut app))?;

    let mut last_draw = Instant::now();
    let mut last_mode = app.mode;

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
                    Ok(AppEvent::Resize) => {
                        terminal.clear().ok();
                        true
                    }
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
            // Force a full repaint when leaving an overlay mode (Detail, Filter,
            // Help, DeleteConfirm). Without this, ratatui's buffer diff can leave
            // residual cells from the prior modal — most visibly when closing
            // the detail modal whose content included syntax-highlighted code.
            if last_mode != app.mode && last_mode != Mode::Normal {
                terminal.clear().ok();
            }
            last_mode = app.mode;
            terminal.draw(|f| ui::render(f, &store, &mut app))?;
            last_draw = Instant::now();
        }
    }
}

fn default_projects_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not determine home directory")?;
    Ok(home.join(".claude").join("projects"))
}

/// Resolve the OpenCode storage dir: `$XDG_DATA_HOME/opencode/storage` if
/// `XDG_DATA_HOME` is set, otherwise `~/.local/share/opencode/storage`.
fn default_opencode_dir() -> Result<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(xdg).join("opencode").join("storage"));
    }
    let home = dirs::home_dir().context("could not determine home directory")?;
    Ok(home
        .join(".local")
        .join("share")
        .join("opencode")
        .join("storage"))
}

/// Build the active source list from a CLI selection and the two candidate
/// directories. A source is only included if its directory exists on disk, so
/// `all` on a Claude-only machine yields just the Claude source. Pure +
/// dependency-light so it is unit-testable without a TTY.
fn build_sources(
    selection: SourceSelection,
    claude_dir: &Path,
    opencode_dir: &Path,
) -> Vec<Arc<dyn Source>> {
    let want_claude = matches!(selection, SourceSelection::Claude | SourceSelection::All);
    let want_opencode = matches!(selection, SourceSelection::Opencode | SourceSelection::All);

    let mut sources: Vec<Arc<dyn Source>> = Vec::new();
    if want_claude && claude_dir.is_dir() {
        sources.push(Arc::new(ClaudeSource::new(claude_dir.to_path_buf())));
    }
    if want_opencode && opencode_dir.is_dir() {
        sources.push(Arc::new(OpencodeSource::new(opencode_dir.to_path_buf())));
    }
    sources
}

/// Short source tag for the `--dump` summary (`cc` = Claude Code, `oc` = OpenCode).
fn source_tag(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Claude => "cc",
        SourceKind::Opencode => "oc",
    }
}

fn run_dump(sources: &[Arc<dyn Source>], session_id: Option<String>) -> Result<()> {
    let mut store = Store::new(sources.to_vec());
    store.initial_scan().context("scan")?;
    let source_labels: Vec<&str> = sources.iter().map(|s| source_tag(s.kind())).collect();
    println!(
        "{} project(s), {} session(s) across source(s): {}",
        store.projects.len(),
        store.sessions.len(),
        source_labels.join(", ")
    );
    for slug in store.project_order_by_recency() {
        let Some(proj) = store.projects.get(&slug) else {
            continue;
        };
        println!(
            "  [{}] {} — {} session(s)",
            source_tag(proj.source),
            proj.display_path,
            proj.sessions.len()
        );
        for sid in proj.sessions.iter().take(5) {
            if let Some(s) = store.sessions.get(sid) {
                let last = s
                    .last_event
                    .or(s.last_mtime)
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| "—".to_string());
                println!(
                    "    [{}] {} {:<60.60} last={}",
                    source_tag(s.source),
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
        println!("\n--- session {} ({} events) ---", sid, s.events.len());
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
                crate::data::Event::AgentName(_) => "agent-name",
                crate::data::Event::Mode(_) => "mode",
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

/// Gather each source's running-process working dirs, tagged by source kind.
/// Consumed by `Store::apply_open_files` for liveness attribution.
fn gather_active_dirs(sources: &[Arc<dyn Source>]) -> Vec<(SourceKind, Vec<PathBuf>)> {
    sources
        .iter()
        .map(|src| (src.kind(), src.active_dirs()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Self-cleaning temp directory (mirrors the source-module test helper to
    /// avoid pulling in an extra dev-dependency).
    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "am-main-src-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
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

    /// Create two temp dirs that look like a Claude projects dir and an
    /// OpenCode storage dir (presence is all `build_sources` checks).
    fn fake_dirs() -> (TmpDir, PathBuf, PathBuf) {
        let tmp = TmpDir::new();
        let claude = tmp.path().join("claude_projects");
        let opencode = tmp.path().join("opencode_storage");
        fs::create_dir_all(&claude).unwrap();
        fs::create_dir_all(&opencode).unwrap();
        (tmp, claude, opencode)
    }

    fn kinds(sources: &[Arc<dyn Source>]) -> Vec<SourceKind> {
        sources.iter().map(|s| s.kind()).collect()
    }

    #[test]
    fn all_selection_includes_both_present_sources() {
        let (_tmp, claude, opencode) = fake_dirs();
        let sources = build_sources(SourceSelection::All, &claude, &opencode);
        let ks = kinds(&sources);
        assert!(
            ks.contains(&SourceKind::Claude),
            "expected claude in {ks:?}"
        );
        assert!(
            ks.contains(&SourceKind::Opencode),
            "expected opencode in {ks:?}"
        );
        assert_eq!(sources.len(), 2);
    }

    #[test]
    fn claude_selection_excludes_opencode() {
        let (_tmp, claude, opencode) = fake_dirs();
        let sources = build_sources(SourceSelection::Claude, &claude, &opencode);
        let ks = kinds(&sources);
        assert_eq!(ks, vec![SourceKind::Claude]);
        assert!(!ks.contains(&SourceKind::Opencode));
    }

    #[test]
    fn opencode_selection_excludes_claude() {
        let (_tmp, claude, opencode) = fake_dirs();
        let sources = build_sources(SourceSelection::Opencode, &claude, &opencode);
        assert_eq!(kinds(&sources), vec![SourceKind::Opencode]);
    }

    #[test]
    fn missing_dir_is_dropped_even_under_all() {
        let (_tmp, claude, _opencode) = fake_dirs();
        let missing = claude.parent().unwrap().join("does_not_exist");
        // Only Claude dir exists; `all` should yield just Claude.
        let sources = build_sources(SourceSelection::All, &claude, &missing);
        assert_eq!(kinds(&sources), vec![SourceKind::Claude]);
    }

    #[test]
    fn explicit_selection_with_missing_dir_yields_empty() {
        let (_tmp, claude, _opencode) = fake_dirs();
        let missing = claude.parent().unwrap().join("nope");
        // Asking for opencode when its dir is absent → empty (caller errors).
        let sources = build_sources(SourceSelection::Opencode, &claude, &missing);
        assert!(sources.is_empty());
    }
}
