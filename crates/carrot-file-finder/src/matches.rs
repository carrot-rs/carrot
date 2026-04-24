//! Match collection — the ordered list of `Match` entries shown to the user.
//! Handles sorting (history-vs-search, filename-vs-path, score), deduping,
//! and history-priority biasing.

use inazuma::{App, Entity, SharedString};
use inazuma_fuzzy::{PathMatch, StringMatch};
use inazuma_util::{paths::PathStyle, rel_path::RelPath};
use std::{cmp, path::PathBuf, sync::Arc};

use carrot_client::ChannelId;
use carrot_project::{Project, ProjectPath, WorktreeId, worktree_store::WorktreeStore};

use crate::history::{FoundPath, matching_history_items, should_hide_root_in_entry_path};
use crate::search_query::FileSearchQuery;

/// Use a custom ordering for file finder: the regular one
/// defines max element with the highest score and the latest alphanumerical path (in case of a tie on other params), e.g:
/// `[{score: 0.5, path = "c/d" }, { score: 0.5, path = "/a/b" }]`
///
/// In the file finder, we would prefer to have the max element with the highest score and the earliest alphanumerical path, e.g:
/// `[{ score: 0.5, path = "/a/b" }, {score: 0.5, path = "c/d" }]`
/// as the files are shown in the project panel lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectPanelOrdMatch(pub(crate) PathMatch);

impl Ord for ProjectPanelOrdMatch {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.0
            .score
            .partial_cmp(&other.0.score)
            .unwrap_or(cmp::Ordering::Equal)
            .then_with(|| self.0.worktree_id.cmp(&other.0.worktree_id))
            .then_with(|| {
                other
                    .0
                    .distance_to_relative_ancestor
                    .cmp(&self.0.distance_to_relative_ancestor)
            })
            .then_with(|| self.0.path.cmp(&other.0.path).reverse())
    }
}

impl PartialOrd for ProjectPanelOrdMatch {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Default)]
pub(crate) struct Matches {
    pub(crate) separate_history: bool,
    pub(crate) matches: Vec<Match>,
}

#[derive(Debug, Clone)]
pub(crate) enum Match {
    History {
        path: FoundPath,
        panel_match: Option<ProjectPanelOrdMatch>,
    },
    Search(ProjectPanelOrdMatch),
    Channel {
        channel_id: ChannelId,
        channel_name: SharedString,
        string_match: StringMatch,
    },
    CreateNew(ProjectPath),
}

impl Match {
    pub(crate) fn relative_path(&self) -> Option<&Arc<RelPath>> {
        match self {
            Match::History { path, .. } => Some(&path.project.path),
            Match::Search(panel_match) => Some(&panel_match.0.path),
            Match::Channel { .. } | Match::CreateNew(_) => None,
        }
    }

    pub(crate) fn abs_path(&self, project: &Entity<Project>, cx: &App) -> Option<PathBuf> {
        match self {
            Match::History { path, .. } => Some(path.absolute.clone()),
            Match::Search(ProjectPanelOrdMatch(path_match)) => Some(
                project
                    .read(cx)
                    .worktree_for_id(WorktreeId::from_usize(path_match.worktree_id), cx)?
                    .read(cx)
                    .absolutize(&path_match.path),
            ),
            Match::Channel { .. } | Match::CreateNew(_) => None,
        }
    }

    pub(crate) fn panel_match(&self) -> Option<&ProjectPanelOrdMatch> {
        match self {
            Match::History { panel_match, .. } => panel_match.as_ref(),
            Match::Search(panel_match) => Some(panel_match),
            Match::Channel { .. } | Match::CreateNew(_) => None,
        }
    }
}

impl Matches {
    pub(crate) fn len(&self) -> usize {
        self.matches.len()
    }

    pub(crate) fn get(&self, index: usize) -> Option<&Match> {
        self.matches.get(index)
    }

    pub(crate) fn position(
        &self,
        entry: &Match,
        currently_opened: Option<&FoundPath>,
    ) -> Result<usize, usize> {
        if let Match::History {
            path,
            panel_match: None,
        } = entry
        {
            // Slow case: linear search by path. Should not happen actually,
            // since we call `position` only if matches set changed, but the query has not changed.
            // And History entries do not have panel_match if query is empty, so there's no
            // reason for the matches set to change.
            self.matches
                .iter()
                .position(|m| match m.relative_path() {
                    Some(p) => path.project.path == *p,
                    None => false,
                })
                .ok_or(0)
        } else {
            self.matches.binary_search_by(|m| {
                // `reverse()` since if cmp_matches(a, b) == Ordering::Greater, then a is better than b.
                // And we want the better entries go first.
                Self::cmp_matches(self.separate_history, currently_opened, m, entry).reverse()
            })
        }
    }

    pub(crate) fn push_new_matches<'a>(
        &'a mut self,
        worktree_store: Entity<WorktreeStore>,
        cx: &'a App,
        history_items: impl IntoIterator<Item = &'a FoundPath> + Clone,
        currently_opened: Option<&'a FoundPath>,
        query: Option<&FileSearchQuery>,
        new_search_matches: impl Iterator<Item = ProjectPanelOrdMatch>,
        extend_old_matches: bool,
        path_style: PathStyle,
    ) {
        let Some(query) = query else {
            // assuming that if there's no query, then there's no search matches.
            self.matches.clear();
            let path_to_entry = |found_path: &FoundPath| Match::History {
                path: found_path.clone(),
                panel_match: None,
            };

            self.matches
                .extend(history_items.into_iter().map(path_to_entry));
            return;
        };

        let worktree_name_by_id = if should_hide_root_in_entry_path(&worktree_store, cx) {
            None
        } else {
            Some(
                worktree_store
                    .read(cx)
                    .worktrees()
                    .map(|worktree| {
                        let snapshot = worktree.read(cx).snapshot();
                        (snapshot.id(), snapshot.root_name().into())
                    })
                    .collect(),
            )
        };
        let new_history_matches = matching_history_items(
            history_items,
            currently_opened,
            worktree_name_by_id,
            query,
            path_style,
        );
        let new_search_matches: Vec<Match> = new_search_matches
            .filter(|path_match| {
                !new_history_matches.contains_key(&ProjectPath {
                    path: path_match.0.path.clone(),
                    worktree_id: WorktreeId::from_usize(path_match.0.worktree_id),
                })
            })
            .map(Match::Search)
            .collect();

        if extend_old_matches {
            // since we take history matches instead of new search matches
            // and history matches has not changed(since the query has not changed and we do not extend old matches otherwise),
            // old matches can't contain paths present in history_matches as well.
            self.matches.retain(|m| matches!(m, Match::Search(_)));
        } else {
            self.matches.clear();
        }

        // At this point we have an unsorted set of new history matches, an unsorted set of new search matches
        // and a sorted set of old search matches.
        // It is possible that the new search matches' paths contain some of the old search matches' paths.
        // History matches' paths are unique, since store in a HashMap by path.
        // We build a sorted Vec<Match>, eliminating duplicate search matches.
        // Search matches with the same paths should have equal `ProjectPanelOrdMatch`, so we should
        // not have any duplicates after building the final list.
        for new_match in new_history_matches
            .into_values()
            .chain(new_search_matches.into_iter())
        {
            match self.position(&new_match, currently_opened) {
                Ok(_duplicate) => continue,
                Err(i) => {
                    self.matches.insert(i, new_match);
                    if self.matches.len() == 100 {
                        break;
                    }
                }
            }
        }
    }

    /// If a < b, then a is a worse match, aligning with the `ProjectPanelOrdMatch` ordering.
    pub(crate) fn cmp_matches(
        separate_history: bool,
        currently_opened: Option<&FoundPath>,
        a: &Match,
        b: &Match,
    ) -> cmp::Ordering {
        // Handle CreateNew variant - always put it at the end
        match (a, b) {
            (Match::CreateNew(_), _) => return cmp::Ordering::Less,
            (_, Match::CreateNew(_)) => return cmp::Ordering::Greater,
            _ => {}
        }

        match (&a, &b) {
            // bubble currently opened files to the top
            (Match::History { path, .. }, _) if Some(path) == currently_opened => {
                return cmp::Ordering::Greater;
            }
            (_, Match::History { path, .. }) if Some(path) == currently_opened => {
                return cmp::Ordering::Less;
            }

            _ => {}
        }

        if separate_history {
            match (a, b) {
                (Match::History { .. }, Match::Search(_)) => return cmp::Ordering::Greater,
                (Match::Search(_), Match::History { .. }) => return cmp::Ordering::Less,

                _ => {}
            }
        }

        // For file-vs-file matches, use the existing detailed comparison.
        if let (Some(a_panel), Some(b_panel)) = (a.panel_match(), b.panel_match()) {
            let a_in_filename = Self::is_filename_match(a_panel);
            let b_in_filename = Self::is_filename_match(b_panel);

            match (a_in_filename, b_in_filename) {
                (true, false) => return cmp::Ordering::Greater,
                (false, true) => return cmp::Ordering::Less,
                _ => {}
            }

            return a_panel.cmp(b_panel);
        }

        let a_score = Self::match_score(a);
        let b_score = Self::match_score(b);
        // When at least one side is a channel, compare by raw score.
        a_score
            .partial_cmp(&b_score)
            .unwrap_or(cmp::Ordering::Equal)
    }

    fn match_score(m: &Match) -> f64 {
        match m {
            Match::History { panel_match, .. } => panel_match.as_ref().map_or(0.0, |pm| pm.0.score),
            Match::Search(pm) => pm.0.score,
            Match::Channel { string_match, .. } => string_match.score,
            Match::CreateNew(_) => 0.0,
        }
    }

    /// Determines if the match occurred within the filename rather than in the path
    fn is_filename_match(panel_match: &ProjectPanelOrdMatch) -> bool {
        if panel_match.0.positions.is_empty() {
            return false;
        }

        if let Some(filename) = panel_match.0.path.file_name() {
            let path_str = panel_match.0.path.as_unix_str();

            if let Some(filename_pos) = path_str.rfind(filename)
                && panel_match.0.positions[0] >= filename_pos
            {
                let mut prev_position = panel_match.0.positions[0];
                for p in &panel_match.0.positions[1..] {
                    if *p != prev_position + 1 {
                        return false;
                    }
                    prev_position = *p;
                }
                return true;
            }
        }

        false
    }
}
