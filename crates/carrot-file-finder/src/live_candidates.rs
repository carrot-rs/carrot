//! `LiveCandidatePool` — a file-finder input source backed by a
//! streaming `LiveWalker`. Owns the walker, accumulates results in
//! memory, integrates with `LiveWalkCache` for TTL reuse, and fuzzy-
//! matches the current candidate set against a user query.
//!
//! The pool is the Live arm of `FinderMode`. It's deliberately
//! scope-local: one pool per opened picker, rooted at a single scope
//! directory (the worktree root the user's cwd sits inside, or the cwd
//! itself for scopes outside any worktree).

use crossbeam::channel::TryRecvError;
use inazuma::App;
use inazuma_fuzzy::{CharBag, PathMatch, PathMatchCandidate};
use inazuma_util::paths::PathStyle;
use inazuma_util::rel_path::RelPath;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use carrot_project::WorktreeId;

use crate::live_walk_cache::LiveWalkCache;
use crate::live_walker::{LiveWalker, LiveWalkerConfig, WalkResult};

/// A bounded, drainable pool of filesystem entries collected by a
/// `LiveWalker`. Holds the walker, the accumulated paths, and enough
/// metadata (scanned count, truncation flag, done flag) to render a
/// picker footer.
pub(crate) struct LiveCandidatePool {
    scope_root: PathBuf,
    worktree_id: WorktreeId,
    walker: Option<LiveWalker>,
    results: Vec<PathBuf>,
    scanned: usize,
    truncated: bool,
    done: bool,
}

impl LiveCandidatePool {
    /// Construct a pool for `scope_root`. Consults `LiveWalkCache` first;
    /// on a fresh hit returns a pre-populated, already-done pool without
    /// spawning any walker. On a miss (or missing global) spawns a new
    /// walker with `config` and returns a pool that will fill on `drain`.
    pub(crate) fn new_with_cache(
        scope_root: PathBuf,
        worktree_id: WorktreeId,
        config: LiveWalkerConfig,
        cx: &mut App,
    ) -> Self {
        let ttl = Duration::from_secs(config.ttl_cache_seconds);
        if let Some(cache) = cx.try_global::<LiveWalkCache>()
            && let Some(hit) = cache.get_fresh(&scope_root, ttl)
        {
            return Self {
                scope_root,
                worktree_id,
                walker: None,
                results: hit.results.clone(),
                scanned: hit.scanned,
                truncated: hit.truncated,
                done: true,
            };
        }
        let walker = LiveWalker::spawn(scope_root.clone(), config);
        Self {
            scope_root,
            worktree_id,
            walker: Some(walker),
            results: Vec::new(),
            scanned: 0,
            truncated: false,
            done: false,
        }
    }

    /// Pull every result the walker has produced so far, non-blocking.
    /// Returns `true` if the pool state changed (new entries or the
    /// walker reported `Done`).
    pub(crate) fn drain_nonblocking(&mut self) -> bool {
        let Some(walker) = self.walker.as_ref() else {
            return false;
        };
        let mut changed = false;
        loop {
            match walker.results_rx.try_recv() {
                Ok(WalkResult::File(path)) => {
                    self.results.push(path);
                    changed = true;
                }
                Ok(WalkResult::Done { scanned, truncated }) => {
                    self.scanned = scanned;
                    self.truncated = truncated;
                    self.done = true;
                    self.walker = None;
                    changed = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // Walker thread exited without a Done — treat as done.
                    self.done = true;
                    self.walker = None;
                    changed = true;
                    break;
                }
            }
        }
        changed
    }

    /// Cancel the walker thread (no effect if already done).
    pub(crate) fn cancel(&self) {
        if let Some(walker) = self.walker.as_ref() {
            walker.cancel();
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        self.done
    }

    /// Files seen so far. When the walker is still running this mirrors
    /// `results.len()`; once `Done`, it reflects the walker's final
    /// counter (which may be larger than `results.len()` if truncation
    /// discarded entries).
    pub(crate) fn scanned(&self) -> usize {
        if self.done && self.scanned > 0 {
            self.scanned
        } else {
            self.results.len()
        }
    }

    pub(crate) fn truncated(&self) -> bool {
        self.truncated
    }

    /// Snapshot the pool's current results in cache-entry shape. Callers
    /// decide when to persist this into the `LiveWalkCache` global — we
    /// don't do the `cx.update_global` here because the caller already
    /// holds the right context flavour.
    pub(crate) fn cache_entry(&self) -> (PathBuf, Vec<PathBuf>, usize, bool) {
        (
            self.scope_root.clone(),
            self.results.clone(),
            self.scanned.max(self.results.len()),
            self.truncated,
        )
    }

    /// Whether stashing to the cache is worthwhile. True only for
    /// completed walks — in-progress pools would cache a partial set and
    /// poison the next open.
    pub(crate) fn worth_caching(&self) -> bool {
        self.done && self.walker.is_none() && !self.results.is_empty()
    }

    /// Fuzzy-match the current candidate set against `query`. Synthesises
    /// `PathMatch` entries whose `worktree_id` is our scope's id so the
    /// merged match list plays well with downstream sort/dedup logic.
    pub(crate) fn fuzzy_match(
        &self,
        query: &str,
        path_style: PathStyle,
        max_results: usize,
    ) -> Vec<PathMatch> {
        if self.results.is_empty() {
            return Vec::new();
        }
        // Convert absolute paths into scope-relative `Arc<RelPath>` so the
        // matcher's path-ranking (prefix, ancestor distance) is consistent
        // with what the Indexed path produces.
        let rel_paths: Vec<Arc<RelPath>> = self
            .results
            .iter()
            .filter_map(|abs| {
                let suffix = abs.strip_prefix(&self.scope_root).ok()?;
                let cow = RelPath::new(suffix, path_style).ok()?;
                Some(cow.as_ref().into_arc())
            })
            .collect();
        if rel_paths.is_empty() {
            return Vec::new();
        }
        let candidates: Vec<PathMatchCandidate<'_>> = rel_paths
            .iter()
            .map(|rp| PathMatchCandidate {
                is_dir: false,
                path: rp,
                char_bag: CharBag::from_iter(
                    rp.file_name()
                        .map(|n| n.to_string().to_lowercase())
                        .unwrap_or_default()
                        .chars(),
                ),
            })
            .collect();
        inazuma_fuzzy::match_fixed_path_set(
            candidates,
            self.worktree_id.to_usize(),
            None,
            query,
            false,
            max_results,
            path_style,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_is_stored_on_construction() {
        let config = LiveWalkerConfig::default();
        // Not constructing via App here; just assert ttl field exists as expected.
        let ttl = Duration::from_secs(config.ttl_cache_seconds);
        assert_eq!(ttl, Duration::from_secs(30));
    }

    #[test]
    fn empty_pool_matches_nothing() {
        let pool = LiveCandidatePool {
            scope_root: PathBuf::from("/x"),
            worktree_id: WorktreeId::from_usize(0),
            walker: None,
            results: Vec::new(),
            scanned: 0,
            truncated: false,
            done: true,
        };
        let matches = pool.fuzzy_match("foo", PathStyle::local(), 100);
        assert!(matches.is_empty());
    }

    #[test]
    fn pool_fuzzy_matches_filename() {
        let pool = LiveCandidatePool {
            scope_root: PathBuf::from("/tmp/proj"),
            worktree_id: WorktreeId::from_usize(42),
            walker: None,
            results: vec![
                PathBuf::from("/tmp/proj/src/main.rs"),
                PathBuf::from("/tmp/proj/README.md"),
            ],
            scanned: 2,
            truncated: false,
            done: true,
        };
        let matches = pool.fuzzy_match("main", PathStyle::local(), 100);
        assert!(matches.iter().any(|m| m.path.as_unix_str().ends_with("main.rs")));
    }
}
