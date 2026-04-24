//! `FinderMode` — dispatcher for the file-finder's two candidate sources.
//!
//! `Indexed` is used when the active scope sits inside a worktree whose
//! background scanner is active: candidates come from the worktree's
//! snapshot, which is what the classic file-finder pipeline already does.
//!
//! `Live` is used for every other scope — Browseable worktrees, cwd
//! outside any worktree, or scopes whose scanner hasn't finished yet —
//! and streams candidates from a `LiveCandidatePool` backed by an
//! on-demand filesystem walker.

use inazuma::{App, Entity};
use std::path::{Path, PathBuf};

use carrot_project::{Project, WorktreeId};

use crate::live_candidates::LiveCandidatePool;
use crate::live_walker::LiveWalkerConfig;

pub(crate) enum FinderMode {
    /// Read candidates from the worktree snapshots the project already
    /// has indexed. No per-picker state — the existing fuzzy pipeline
    /// owns the data.
    Indexed,
    /// Stream candidates live. The pool owns its walker, its results,
    /// and its progress counters.
    Live(LiveCandidatePool),
}

impl FinderMode {
    pub(crate) fn is_live(&self) -> bool {
        matches!(self, Self::Live(_))
    }

    pub(crate) fn as_live(&self) -> Option<&LiveCandidatePool> {
        match self {
            Self::Live(pool) => Some(pool),
            Self::Indexed => None,
        }
    }

    pub(crate) fn as_live_mut(&mut self) -> Option<&mut LiveCandidatePool> {
        match self {
            Self::Live(pool) => Some(pool),
            Self::Indexed => None,
        }
    }
}

/// Decide the mode for a freshly-opened picker.
///
/// If `cwd_hint` falls inside a visible worktree whose scanner is
/// enabled, we pick `Indexed`: that worktree already feeds the classic
/// match pipeline. Otherwise we pick `Live`, anchored at the best scope
/// root we can derive — preferring a worktree root when one covers the
/// cwd (even a Browseable one), falling back to the cwd itself, and
/// finally to the filesystem root when no hint is available.
pub(crate) fn determine_mode(
    project: &Entity<Project>,
    cwd_hint: Option<&Path>,
    config: LiveWalkerConfig,
    cx: &mut App,
) -> FinderMode {
    let project_r = project.read(cx);

    if let Some(cwd) = cwd_hint
        && let Some((worktree, _)) = project_r.find_worktree(cwd, cx)
    {
        let wt = worktree.read(cx);
        if wt.is_visible() && wt.scanning_enabled() {
            return FinderMode::Indexed;
        }
    }

    let (scope_root, worktree_id) = resolve_scope(project, cwd_hint, cx);
    FinderMode::Live(LiveCandidatePool::new_with_cache(
        scope_root,
        worktree_id,
        config,
        cx,
    ))
}

/// Pick the scope root + worktree id a Live pool should anchor at.
/// Prefers the worktree root when one contains the cwd; otherwise uses
/// the cwd directly with a synthetic worktree id.
fn resolve_scope(
    project: &Entity<Project>,
    cwd_hint: Option<&Path>,
    cx: &App,
) -> (PathBuf, WorktreeId) {
    if let Some(cwd) = cwd_hint
        && let Some((worktree, _)) = project.read(cx).find_worktree(cwd, cx)
    {
        let wt = worktree.read(cx);
        return (wt.abs_path().to_path_buf(), wt.id());
    }
    let root = cwd_hint
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));
    // Synthetic id for scopes outside any worktree. Picks a value that
    // won't collide with real worktree ids (which are allocated from a
    // monotonic counter starting at a low usize).
    (root, WorktreeId::from_usize(usize::MAX))
}
