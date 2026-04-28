//! Bounded filesystem walker for the files source. Runs in a dedicated
//! thread, streams `WalkResult::File(path)` over a crossbeam channel, and
//! emits `WalkResult::Done` on completion or budget exhaustion.

use crossbeam::channel::{Receiver, bounded};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LiveWalkerConfig {
    pub max_entries: usize,
    pub max_wall_time_ms: u64,
    pub max_depth: usize,
    pub parallel_walkers: usize,
    pub respect_gitignore: bool,
    pub respect_carrotignore: bool,
    pub respect_hidden: bool,
    pub ttl_cache_seconds: u64,
}

impl Default for LiveWalkerConfig {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            max_wall_time_ms: 5_000,
            max_depth: 25,
            parallel_walkers: num_cpus::get().max(1),
            respect_gitignore: true,
            respect_carrotignore: true,
            respect_hidden: true,
            ttl_cache_seconds: 30,
        }
    }
}

#[derive(Debug)]
pub enum WalkResult {
    File(PathBuf),
    Done { scanned: usize, truncated: bool },
}

pub struct LiveWalker {
    cancel: Arc<AtomicBool>,
    pub results_rx: Receiver<WalkResult>,
}

impl LiveWalker {
    pub fn spawn(scope_root: PathBuf, config: LiveWalkerConfig) -> Self {
        let (tx, rx) = bounded(2048);
        let cancel = Arc::new(AtomicBool::new(false));

        std::thread::spawn({
            let cancel = cancel.clone();
            move || {
                use ignore::{WalkBuilder, WalkState};
                let start = std::time::Instant::now();
                let count = Arc::new(AtomicUsize::new(0));

                let mut builder = WalkBuilder::new(&scope_root);
                builder
                    .hidden(config.respect_hidden)
                    .git_ignore(config.respect_gitignore)
                    .max_depth(Some(config.max_depth))
                    .threads(config.parallel_walkers);
                if config.respect_carrotignore {
                    builder.add_custom_ignore_filename(".carrotignore");
                }

                builder.build_parallel().run(|| {
                    let cancel = cancel.clone();
                    let count = count.clone();
                    let tx = tx.clone();
                    let max_entries = config.max_entries;
                    let max_wall_time_ms = config.max_wall_time_ms;
                    Box::new(move |entry| {
                        if cancel.load(Ordering::Relaxed)
                            || count.load(Ordering::Relaxed) >= max_entries
                            || start.elapsed().as_millis() as u64 >= max_wall_time_ms
                        {
                            return WalkState::Quit;
                        }
                        if let Ok(e) = entry
                            && e.file_type().is_some_and(|ft| ft.is_file())
                            && tx.send(WalkResult::File(e.into_path())).is_ok()
                        {
                            count.fetch_add(1, Ordering::Relaxed);
                        }
                        WalkState::Continue
                    })
                });

                let scanned = count.load(Ordering::Relaxed);
                let _ = tx.send(WalkResult::Done {
                    scanned,
                    truncated: scanned >= config.max_entries,
                });
            }
        });

        LiveWalker {
            cancel,
            results_rx: rx,
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn drain(walker: &LiveWalker) -> (Vec<PathBuf>, usize, bool) {
        let mut files = Vec::new();
        let (mut scanned, mut truncated) = (0, false);
        while let Ok(msg) = walker.results_rx.recv() {
            match msg {
                WalkResult::File(p) => files.push(p),
                WalkResult::Done {
                    scanned: s,
                    truncated: t,
                } => {
                    scanned = s;
                    truncated = t;
                    break;
                }
            }
        }
        (files, scanned, truncated)
    }

    #[test]
    fn walks_flat_directory() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..10 {
            fs::write(dir.path().join(format!("f{i}.txt")), "x").unwrap();
        }
        let walker = LiveWalker::spawn(dir.path().to_path_buf(), LiveWalkerConfig::default());
        let (files, scanned, truncated) = drain(&walker);
        assert_eq!(files.len(), 10);
        assert_eq!(scanned, 10);
        assert!(!truncated);
    }

    #[test]
    fn respects_max_entries() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..50 {
            fs::write(dir.path().join(format!("f{i}.txt")), "x").unwrap();
        }
        let config = LiveWalkerConfig {
            max_entries: 10,
            ..LiveWalkerConfig::default()
        };
        let walker = LiveWalker::spawn(dir.path().to_path_buf(), config);
        let (_, scanned, truncated) = drain(&walker);
        assert!(scanned >= 10);
        assert!(truncated);
    }
}
