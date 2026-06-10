//! Data-source abstraction.
//!
//! A [`Source`] owns everything source-specific: how a source's projects and
//! sessions are laid out on disk, how a session file is parsed and tail-loaded,
//! how a raw FS path maps back to a session, and how the source's running
//! processes are discovered for liveness. `Store` stays generic over the set of
//! sources and only knows about the source-agnostic pieces (project/session
//! maps, ordering, the N-most-recent process-attribution logic).

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::data::{Project, Session, SourceKind};

mod claude;

pub use claude::ClaudeSource;

/// Everything that varies between data sources (Claude Code, OpenCode, …).
///
/// Object-safe: stored as `Arc<dyn Source>` in `Store`. `Send + Sync` so the
/// background liveness thread can call [`Source::active_dirs`] off the main
/// thread.
pub trait Source: Send + Sync {
    /// Stable identifier for this source.
    fn kind(&self) -> SourceKind;

    /// Root directory this source reads from (watched by the FS watcher).
    fn root(&self) -> &Path;

    /// Discover all projects + their session metadata (cheap; no full parse).
    /// Each returned `Project` has an empty `sessions` list — the store owns the
    /// per-project session-id list and its ordering.
    fn scan(&self) -> Vec<(Project, Vec<Session>)>;

    /// Scan a single newly-created project directory, if `dir` belongs to this
    /// source. Returns the same shape as one entry of [`Source::scan`].
    fn scan_project(&self, dir: &Path) -> Option<(Project, Vec<Session>)>;

    /// Build a metadata-scanned `Session` plus its owning `Project` for a newly
    /// created session file. `None` if the path is not a session file of this
    /// source.
    fn discover_session(&self, file: &Path) -> Option<(Project, Session)>;

    /// Full-load one session's events/usage in place.
    fn load_session(&self, session: &mut Session) -> Result<()>;

    /// Incrementally load anything new for a session. For a loaded session this
    /// is a byte-offset tail load; for a not-yet-loaded session it refreshes the
    /// cheap metadata scan.
    fn refresh_session(&self, session: &mut Session) -> Result<()>;

    /// Map a raw FS path (from the watcher) to the affected session id, if this
    /// source owns it. Cheap: inspects the path only, never stats the file, so
    /// it works for removed paths too.
    fn session_id_for_path(&self, path: &Path) -> Option<String>;

    /// Working dirs of this source's running processes (one entry per process,
    /// duplicates preserved) — used for liveness attribution.
    fn active_dirs(&self) -> Vec<PathBuf>;

    /// Map a process working directory to the project slug it corresponds to,
    /// using this source's path-encoding scheme.
    fn slug_for_cwd(&self, cwd: &Path) -> Option<String>;
}
