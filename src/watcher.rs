use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, DebouncedEventKind, Debouncer};

use crate::app::AppEvent;
use crate::store::FsEvent;

pub struct WatcherHandle {
    _debouncer: Debouncer<notify::RecommendedWatcher>,
}

pub fn spawn_watcher(
    projects_dir: PathBuf,
    tx: Sender<AppEvent>,
    debug: bool,
) -> Result<WatcherHandle> {
    let tx_inner = tx.clone();
    let mut debouncer = new_debouncer(
        Duration::from_millis(200),
        move |res: DebounceEventResult| match res {
            Ok(events) => {
                for ev in events {
                    if debug {
                        eprintln!("[watch] {:?} {:?}", ev.kind, ev.path);
                    }
                    // notify-debouncer-mini emits only Any and AnyContinuous; we can't
                    // distinguish create/modify/remove. Inspect the path to decide.
                    let path = ev.path.clone();
                    let send_kind = match ev.kind {
                        DebouncedEventKind::Any | DebouncedEventKind::AnyContinuous => {
                            if path.exists() {
                                if path.is_dir() {
                                    FsEvent::Created(path)
                                } else {
                                    FsEvent::Modified(path)
                                }
                            } else {
                                FsEvent::Removed(path)
                            }
                        }
                        _ => continue,
                    };
                    let _ = tx_inner.send(AppEvent::Fs(send_kind));
                }
            }
            Err(e) => {
                if debug {
                    eprintln!("[watch] error: {:?}", e);
                }
            }
        },
    )
    .context("creating debouncer")?;

    debouncer
        .watcher()
        .watch(&projects_dir, RecursiveMode::Recursive)
        .with_context(|| format!("watching {}", projects_dir.display()))?;

    Ok(WatcherHandle {
        _debouncer: debouncer,
    })
}
