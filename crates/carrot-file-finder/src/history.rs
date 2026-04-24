//! History matching — recent-navigation items fuzzy-matched against the
//! current query, plus the `FoundPath` value type shared across modules.

use inazuma::{App, Entity};
use inazuma_collections::HashMap;
use inazuma_fuzzy::{CharBag, PathMatchCandidate};
use inazuma_util::{paths::PathStyle, rel_path::RelPath};
use std::{path::PathBuf, sync::Arc};

use carrot_project::{ProjectPath, WorktreeId, worktree_store::WorktreeStore};
use carrot_project_panel::project_panel_settings::ProjectPanelSettings;
use inazuma_settings_framework::Settings;

use crate::matches::{Match, ProjectPanelOrdMatch};
use crate::search_query::FileSearchQuery;

pub(crate) const MAX_RECENT_SELECTIONS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct FoundPath {
    pub(crate) project: ProjectPath,
    pub(crate) absolute: PathBuf,
}

impl FoundPath {
    pub(crate) fn new(project: ProjectPath, absolute: PathBuf) -> Self {
        Self { project, absolute }
    }
}

pub(crate) fn matching_history_items<'a>(
    history_items: impl IntoIterator<Item = &'a FoundPath>,
    currently_opened: Option<&'a FoundPath>,
    worktree_name_by_id: Option<HashMap<WorktreeId, Arc<RelPath>>>,
    query: &FileSearchQuery,
    path_style: PathStyle,
) -> HashMap<ProjectPath, Match> {
    let mut candidates_paths = HashMap::default();

    let history_items_by_worktrees = history_items
        .into_iter()
        .chain(currently_opened)
        .filter_map(|found_path| {
            let candidate = PathMatchCandidate {
                is_dir: false, // You can't open directories as project items
                path: &found_path.project.path,
                // Only match history items names, otherwise their paths may match too many queries, producing false positives.
                // E.g. `foo` would match both `something/foo/bar.rs` and `something/foo/foo.rs` and if the former is a history item,
                // it would be shown first always, despite the latter being a better match.
                char_bag: CharBag::from_iter(
                    found_path
                        .project
                        .path
                        .file_name()?
                        .to_string()
                        .to_lowercase()
                        .chars(),
                ),
            };
            candidates_paths.insert(&found_path.project, found_path);
            Some((found_path.project.worktree_id, candidate))
        })
        .fold(
            HashMap::default(),
            |mut candidates, (worktree_id, new_candidate)| {
                candidates
                    .entry(worktree_id)
                    .or_insert_with(Vec::new)
                    .push(new_candidate);
                candidates
            },
        );
    let mut matching_history_paths = HashMap::default();
    for (worktree, candidates) in history_items_by_worktrees {
        let max_results = candidates.len() + 1;
        let worktree_root_name = worktree_name_by_id
            .as_ref()
            .and_then(|w| w.get(&worktree).cloned());
        matching_history_paths.extend(
            inazuma_fuzzy::match_fixed_path_set(
                candidates,
                worktree.to_usize(),
                worktree_root_name,
                query.path_query(),
                false,
                max_results,
                path_style,
            )
            .into_iter()
            .filter_map(|path_match| {
                candidates_paths
                    .remove_entry(&ProjectPath {
                        worktree_id: WorktreeId::from_usize(path_match.worktree_id),
                        path: Arc::clone(&path_match.path),
                    })
                    .map(|(project_path, found_path)| {
                        (
                            project_path.clone(),
                            Match::History {
                                path: found_path.clone(),
                                panel_match: Some(ProjectPanelOrdMatch(path_match)),
                            },
                        )
                    })
            }),
        );
    }
    matching_history_paths
}

pub(crate) fn should_hide_root_in_entry_path(
    worktree_store: &Entity<WorktreeStore>,
    cx: &App,
) -> bool {
    let multiple_worktrees = worktree_store
        .read(cx)
        .visible_worktrees(cx)
        .filter(|worktree| !worktree.read(cx).is_single_file())
        .nth(1)
        .is_some();
    ProjectPanelSettings::get_global(cx).hide_root && !multiple_worktrees
}
