//! `FilesSource` — streaming filesystem search backed by [`LiveWalker`].
//!
//! The source is stateful: one [`PoolState`] instance persists across
//! `collect()` calls so a walker spawned on the first keystroke continues
//! to fill results as the user keeps typing. State mutation is guarded by
//! a `Mutex` so the [`SearchSource`] trait's `&self` API stays intact.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use carrot_workspace::Workspace;
use crossbeam::channel::TryRecvError;
use inazuma::{App, BorrowAppContext, Entity, SharedString, Window};
use inazuma_fuzzy::{CharBag, PathMatchCandidate};

use crate::category::SearchCategory;
use crate::source::{SearchAction, SearchResult, SearchSource};

use super::live_walk_cache::LiveWalkCache;
use super::live_walker::{LiveWalker, LiveWalkerConfig, WalkResult};

/// Minimum query length before we spawn a walker. Below this we return an
/// empty result set so the modal doesn't launch a full-disk scan on a
/// stray keystroke.
const MIN_QUERY_CHARS: usize = 2;

/// Max entries returned per `collect()` call. Enough to fill the visible
/// result list with headroom — anything beyond this gets fuzzy-scored off
/// anyway.
const MAX_RESULTS: usize = 200;

/// How many recent-open entries the frecency list keeps. Bigger than the
/// suggestions section but bounded so a long session doesn't bloat state.
const MAX_RECENTS: usize = 50;

/// Footer status carried from [`FilesSource::footer_status`] to the modal
/// so the UI can render a per-scope breadcrumb + scan counter.
#[derive(Debug, Clone)]
pub struct FilesSourceStatus {
    pub scope_root: PathBuf,
    pub scanned: usize,
    pub truncated: bool,
    pub done: bool,
}

struct PoolState {
    scope_root: PathBuf,
    walker: Option<LiveWalker>,
    results: Vec<PathBuf>,
    scanned: usize,
    truncated: bool,
    done: bool,
    config: LiveWalkerConfig,
    /// Frecency list — most-recently-opened paths float to the top when
    /// the query is empty or short. Populated lazily as the user selects.
    recents: Vec<PathBuf>,
    respect_ignored: bool,
}

impl PoolState {
    fn empty() -> Self {
        Self {
            scope_root: PathBuf::new(),
            walker: None,
            results: Vec::new(),
            scanned: 0,
            truncated: false,
            done: false,
            config: LiveWalkerConfig::default(),
            recents: Vec::new(),
            respect_ignored: true,
        }
    }

    fn drain(&mut self) -> bool {
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
                    self.done = true;
                    self.walker = None;
                    changed = true;
                    break;
                }
            }
        }
        changed
    }

    fn reset_to(&mut self, scope_root: PathBuf) {
        if let Some(walker) = &self.walker {
            walker.cancel();
        }
        self.scope_root = scope_root;
        self.walker = None;
        self.results.clear();
        self.scanned = 0;
        self.truncated = false;
        self.done = false;
    }
}

/// Files-category source: spawns a bounded filesystem walker rooted at
/// the workspace's project scope and fuzzy-matches the user's query
/// against the resulting path set.
pub struct FilesSource {
    state: Mutex<PoolState>,
}

impl FilesSource {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(PoolState::empty()),
        }
    }

    /// Push `path` to the front of the frecency list so it floats to the
    /// top of the empty-query suggestions next time the modal opens.
    /// Called by the modal when the user activates a file result.
    pub fn record_open(&self, path: PathBuf) {
        if let Ok(mut state) = self.state.lock() {
            state.recents.retain(|p| p != &path);
            state.recents.insert(0, path);
            if state.recents.len() > MAX_RECENTS {
                state.recents.truncate(MAX_RECENTS);
            }
        }
    }

    /// Flip the `respect_ignored` walker flag. On change the current pool
    /// is reset so the next `collect()` re-spawns with the new setting.
    pub fn set_include_ignored(&self, include_ignored: bool) {
        if let Ok(mut state) = self.state.lock() {
            let new_respect = !include_ignored;
            if state.respect_ignored == new_respect {
                return;
            }
            state.respect_ignored = new_respect;
            if let Some(walker) = &state.walker {
                walker.cancel();
            }
            state.walker = None;
            state.results.clear();
            state.scanned = 0;
            state.truncated = false;
            state.done = false;
        }
    }

    fn resolve_scope(workspace: &Workspace, cx: &App) -> PathBuf {
        // Prefer the first visible worktree root; fall back to the user's
        // home directory so a fresh workspace still produces some results
        // instead of an empty pane.
        workspace
            .project()
            .read(cx)
            .visible_worktrees(cx)
            .next()
            .map(|wt| wt.read(cx).abs_path().to_path_buf())
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
    }

    /// Expand a leading `~/` into the user's home directory so queries
    /// like `~/Projects` behave like the shell.
    fn expand_tilde(query: &str) -> (PathBuf, String) {
        if let Some(rest) = query.strip_prefix("~/")
            && let Some(home) = dirs::home_dir()
        {
            return (home, rest.to_string());
        }
        (PathBuf::new(), query.to_string())
    }
}

impl Default for FilesSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchSource for FilesSource {
    fn category(&self) -> SearchCategory {
        SearchCategory::Files
    }

    fn default_visible(&self) -> bool {
        // Part of the universal-mode mix, but only once the user has
        // typed something — see `collect()`.
        true
    }

    fn footer_status(&self, _cx: &App) -> Option<FilesSourceStatus> {
        let state = self.state.lock().ok()?;
        if state.scope_root.as_os_str().is_empty() {
            return None;
        }
        Some(FilesSourceStatus {
            scope_root: state.scope_root.clone(),
            scanned: state.scanned.max(state.results.len()),
            truncated: state.truncated,
            done: state.done,
        })
    }

    fn collect(
        &self,
        workspace: &Entity<Workspace>,
        query: &str,
        _window: &Window,
        cx: &mut App,
    ) -> Vec<SearchResult> {
        // `~` expansion happens first so the walker gets spawned on the
        // expanded root, not the literal "~" prefix.
        let (tilde_root, effective_query) = Self::expand_tilde(query);
        let mut scope_root = if !tilde_root.as_os_str().is_empty() {
            tilde_root
        } else {
            Self::resolve_scope(workspace.read(cx), cx)
        };
        scope_root = scope_root.canonicalize().unwrap_or(scope_root);

        let mut state = match self.state.lock() {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        // Scope change → rip down the old walker, cache the current set,
        // start fresh.
        if state.scope_root != scope_root {
            let old_scope = std::mem::take(&mut state.scope_root);
            if state.done && !state.results.is_empty() {
                let cache_key = old_scope;
                let cache_entry = (
                    state.results.clone(),
                    state.scanned.max(state.results.len()),
                    state.truncated,
                );
                drop(state);
                if let Some(cache) = cx.try_global::<LiveWalkCache>() {
                    let _ = cache;
                    cx.update_global::<LiveWalkCache, _>(|c, _| {
                        c.put(cache_key, cache_entry.0, cache_entry.1, cache_entry.2)
                    });
                }
                state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            }
            state.reset_to(scope_root.clone());
        }

        // No walker yet? Check the shared cache; spawn on miss.
        if state.walker.is_none() && !state.done {
            let ttl = Duration::from_secs(state.config.ttl_cache_seconds);
            let cache_hit = cx
                .try_global::<LiveWalkCache>()
                .and_then(|c| c.get_fresh(&scope_root, ttl).cloned());

            if let Some(hit) = cache_hit {
                state.results = hit.results;
                state.scanned = hit.scanned;
                state.truncated = hit.truncated;
                state.done = true;
            } else {
                let mut config = state.config.clone();
                config.respect_gitignore = state.respect_ignored;
                config.respect_hidden = state.respect_ignored;
                state.walker = Some(LiveWalker::spawn(scope_root.clone(), config));
            }
        }

        state.drain();

        // No query → surface recent opens only. We intentionally do *not*
        // return raw walker results here: Cmd+P's "Suggested" section
        // wants a curated mix, not the first 20 arbitrary files the
        // walker happened to scan first. If the user has no recent opens
        // yet they see an empty slice; typing ≥ 2 chars kicks in the
        // fuzzy search below.
        if effective_query.len() < MIN_QUERY_CHARS {
            return state
                .recents
                .iter()
                .take(10)
                .cloned()
                .map(|path| build_result(&state.scope_root, path))
                .collect();
        }

        // Fuzzy match the pool against the query. Paths are
        // scope-relative so ranking hits the right characters — absolute
        // paths would add noise from the shared prefix.
        let rel_paths: Vec<_> = state
            .results
            .iter()
            .filter_map(|abs| {
                abs.strip_prefix(&state.scope_root)
                    .ok()
                    .map(|rel| (abs.clone(), rel.to_path_buf()))
            })
            .collect();

        let path_style = inazuma_util::paths::PathStyle::local();
        let rel_path_arcs: Vec<_> = rel_paths
            .iter()
            .filter_map(|(_, rel)| {
                let cow = inazuma_util::rel_path::RelPath::new(rel, path_style).ok()?;
                Some(cow.as_ref().into_arc())
            })
            .collect();

        let candidates: Vec<PathMatchCandidate<'_>> = rel_path_arcs
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

        let matches = inazuma_fuzzy::match_fixed_path_set(
            candidates,
            0,
            None,
            &effective_query,
            false,
            MAX_RESULTS,
            path_style,
        );

        matches
            .into_iter()
            .filter_map(|m| {
                let rel_str = m.path.as_unix_str();
                rel_paths
                    .iter()
                    .find(|(_, rel)| rel.to_string_lossy() == rel_str)
                    .map(|(abs, _)| build_result(&state.scope_root, abs.clone()))
            })
            .collect()
    }
}

fn build_result(scope_root: &Path, abs_path: PathBuf) -> SearchResult {
    let display = abs_path
        .strip_prefix(scope_root)
        .unwrap_or(&abs_path)
        .to_path_buf();
    let title = display
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| display.to_string_lossy().to_string());
    let parent = display
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_string_lossy().to_string());
    SearchResult {
        id: format!("file:{}", abs_path.display()).into(),
        category: SearchCategory::Files,
        title: SharedString::from(title),
        subtitle: parent.map(SharedString::from),
        icon: SearchCategory::Files.icon(),
        action: SearchAction::OpenPath(abs_path),
    }
}
