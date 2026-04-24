//! `FileFinderDelegate` — the business logic of the file finder picker.
//! Owns the currently-matched state, the worktree search pipeline, and the
//! history-priority biasing. The `PickerDelegate` trait impl (render +
//! confirm) lives in `picker.rs`.

use inazuma::{App, Context, Entity, FocusHandle, KeyContext, Task, WeakEntity, Window, px};
use inazuma_fuzzy::{PathMatch, StringMatch, StringMatchCandidate};
use inazuma_picker::Picker;
use inazuma_settings_framework::Settings;
use inazuma_util::{ResultExt, paths::PathStyle, rel_path::RelPath};
use std::{
    borrow::Cow,
    path::Path,
    sync::{
        Arc,
        atomic::{self, AtomicBool},
    },
};

use carrot_channel::ChannelStore;
use carrot_open_path_prompt::file_finder_settings::FileFinderSettings;
use carrot_project::{PathMatchCandidateSet, Project, ProjectPath};
use carrot_ui::{
    Color, ContextMenu, HighlightedLabel, LabelCommon as _, LabelSize, PopoverMenuHandle, TextSize,
};
use carrot_workspace::Workspace;

use crate::FileFinder;
use crate::history::{FoundPath, should_hide_root_in_entry_path};
use crate::matches::{Match, Matches, ProjectPanelOrdMatch};
use crate::path_render::{PathComponentSlice, full_path_budget};
use crate::search_query::FileSearchQuery;

pub struct FileFinderDelegate {
    pub(crate) file_finder: WeakEntity<FileFinder>,
    pub(crate) workspace: WeakEntity<Workspace>,
    pub(crate) project: Entity<Project>,
    pub(crate) channel_store: Option<Entity<ChannelStore>>,
    pub(crate) search_count: usize,
    pub(crate) latest_search_id: usize,
    pub(crate) latest_search_did_cancel: bool,
    pub(crate) latest_search_query: Option<FileSearchQuery>,
    pub(crate) currently_opened_path: Option<FoundPath>,
    pub(crate) matches: Matches,
    pub(crate) selected_index: usize,
    pub(crate) has_changed_selected_index: bool,
    pub(crate) cancel_flag: Arc<AtomicBool>,
    pub(crate) history_items: Vec<FoundPath>,
    pub(crate) separate_history: bool,
    pub(crate) first_update: bool,
    pub(crate) filter_popover_menu_handle: PopoverMenuHandle<ContextMenu>,
    pub(crate) split_popover_menu_handle: PopoverMenuHandle<ContextMenu>,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) include_ignored: Option<bool>,
    pub(crate) include_ignored_refresh: Task<()>,
}

impl FileFinderDelegate {
    pub(crate) fn new(
        file_finder: WeakEntity<FileFinder>,
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        currently_opened_path: Option<FoundPath>,
        history_items: Vec<FoundPath>,
        separate_history: bool,
        window: &mut Window,
        cx: &mut Context<FileFinder>,
    ) -> Self {
        Self::subscribe_to_updates(&project, window, cx);
        let channel_store = if FileFinderSettings::get_global(cx).include_channels {
            ChannelStore::try_global(cx)
        } else {
            None
        };
        Self {
            file_finder,
            workspace,
            project,
            channel_store,
            search_count: 0,
            latest_search_id: 0,
            latest_search_did_cancel: false,
            latest_search_query: None,
            currently_opened_path,
            matches: Matches::default(),
            has_changed_selected_index: false,
            selected_index: 0,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            history_items,
            separate_history,
            first_update: true,
            filter_popover_menu_handle: PopoverMenuHandle::default(),
            split_popover_menu_handle: PopoverMenuHandle::default(),
            focus_handle: cx.focus_handle(),
            include_ignored: FileFinderSettings::get_global(cx).include_ignored,
            include_ignored_refresh: Task::ready(()),
        }
    }

    fn subscribe_to_updates(
        project: &Entity<Project>,
        window: &mut Window,
        cx: &mut Context<FileFinder>,
    ) {
        cx.subscribe_in(project, window, |file_finder, _, event, window, cx| {
            match event {
                carrot_project::Event::WorktreeUpdatedEntries(_, _)
                | carrot_project::Event::WorktreeAdded(_)
                | carrot_project::Event::WorktreeRemoved(_) => file_finder
                    .picker
                    .update(cx, |picker, cx| picker.refresh(window, cx)),
                _ => {}
            };
        })
        .detach();
    }

    pub(crate) fn spawn_search(
        &mut self,
        query: FileSearchQuery,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let relative_to = self
            .currently_opened_path
            .as_ref()
            .map(|found_path| Arc::clone(&found_path.project.path));
        let worktree_store = self.project.read(cx).worktree_store();
        let worktrees = worktree_store
            .read(cx)
            .visible_worktrees_and_single_files(cx)
            .collect::<Vec<_>>();
        let include_root_name = !should_hide_root_in_entry_path(&worktree_store, cx);
        let candidate_sets = worktrees
            .into_iter()
            .map(|worktree| {
                let worktree = worktree.read(cx);
                PathMatchCandidateSet {
                    snapshot: worktree.snapshot(),
                    include_ignored: self.include_ignored.unwrap_or_else(|| {
                        worktree.root_entry().is_some_and(|entry| entry.is_ignored)
                    }),
                    include_root_name,
                    candidates: carrot_project::Candidates::Files,
                }
            })
            .collect::<Vec<_>>();

        let search_id = inazuma_util::post_inc(&mut self.search_count);
        self.cancel_flag.store(true, atomic::Ordering::Release);
        self.cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_flag = self.cancel_flag.clone();
        cx.spawn_in(window, async move |picker, cx| {
            let matches = inazuma_fuzzy::match_path_sets(
                candidate_sets.as_slice(),
                query.path_query(),
                &relative_to,
                false,
                100,
                &cancel_flag,
                cx.background_executor().clone(),
            )
            .await
            .into_iter()
            .map(ProjectPanelOrdMatch);
            let did_cancel = cancel_flag.load(atomic::Ordering::Acquire);
            picker
                .update(cx, |picker, cx| {
                    picker
                        .delegate
                        .set_search_matches(search_id, did_cancel, query, matches, cx)
                })
                .log_err();
        })
    }

    pub(crate) fn set_search_matches(
        &mut self,
        search_id: usize,
        did_cancel: bool,
        query: FileSearchQuery,
        matches: impl IntoIterator<Item = ProjectPanelOrdMatch>,
        cx: &mut Context<Picker<Self>>,
    ) {
        if search_id >= self.latest_search_id {
            self.latest_search_id = search_id;
            let query_changed = Some(query.path_query())
                != self
                    .latest_search_query
                    .as_ref()
                    .map(|query| query.path_query());
            let extend_old_matches = self.latest_search_did_cancel && !query_changed;

            let selected_match = if query_changed {
                None
            } else {
                self.matches.get(self.selected_index).cloned()
            };

            let path_style = self.project.read(cx).path_style(cx);
            self.matches.push_new_matches(
                self.project.read(cx).worktree_store(),
                cx,
                &self.history_items,
                self.currently_opened_path.as_ref(),
                Some(&query),
                matches.into_iter(),
                extend_old_matches,
                path_style,
            );

            // Add channel matches
            if let Some(channel_store) = &self.channel_store {
                let channel_store = channel_store.read(cx);
                let channels: Vec<_> = channel_store.channels().cloned().collect();
                if !channels.is_empty() {
                    let candidates = channels
                        .iter()
                        .enumerate()
                        .map(|(id, channel)| StringMatchCandidate::new(id, &channel.name));
                    let channel_query = query.path_query();
                    let query_lower = channel_query.to_lowercase();
                    let mut channel_matches = Vec::new();
                    for candidate in candidates {
                        let channel_name = candidate.string;
                        let name_lower = channel_name.to_lowercase();

                        let mut positions = Vec::new();
                        let mut query_idx = 0;
                        for (name_idx, name_char) in name_lower.char_indices() {
                            if query_idx < query_lower.len() {
                                let query_char =
                                    query_lower[query_idx..].chars().next().unwrap_or_default();
                                if name_char == query_char {
                                    positions.push(name_idx);
                                    query_idx += query_char.len_utf8();
                                }
                            }
                        }

                        if query_idx == query_lower.len() {
                            let channel = &channels[candidate.id];
                            let score = if name_lower == query_lower {
                                1.0
                            } else if name_lower.starts_with(&query_lower) {
                                0.8
                            } else {
                                0.5 * (query_lower.len() as f64 / name_lower.len() as f64)
                            };
                            channel_matches.push(Match::Channel {
                                channel_id: channel.id,
                                channel_name: channel.name.clone(),
                                string_match: StringMatch {
                                    candidate_id: candidate.id,
                                    score,
                                    positions,
                                    string: channel_name,
                                },
                            });
                        }
                    }
                    for channel_match in channel_matches {
                        match self
                            .matches
                            .position(&channel_match, self.currently_opened_path.as_ref())
                        {
                            Ok(_duplicate) => {}
                            Err(ix) => self.matches.matches.insert(ix, channel_match),
                        }
                    }
                }
            }

            let query_path = query.raw_query.as_str();
            if let Ok(mut query_path) = RelPath::new(Path::new(query_path), path_style) {
                let available_worktree = self
                    .project
                    .read(cx)
                    .visible_worktrees(cx)
                    .filter(|worktree| !worktree.read(cx).is_single_file())
                    .collect::<Vec<_>>();
                let worktree_count = available_worktree.len();
                let mut expect_worktree = available_worktree.first().cloned();
                for worktree in &available_worktree {
                    let worktree_root = worktree.read(cx).root_name();
                    if worktree_count > 1 {
                        if let Ok(suffix) = query_path.strip_prefix(worktree_root) {
                            query_path = Cow::Owned(suffix.to_owned());
                            expect_worktree = Some(worktree.clone());
                            break;
                        }
                    }
                }

                if let Some(FoundPath { ref project, .. }) = self.currently_opened_path {
                    let worktree_id = project.worktree_id;
                    let focused_file_in_available_worktree = available_worktree
                        .iter()
                        .any(|wt| wt.read(cx).id() == worktree_id);

                    if focused_file_in_available_worktree {
                        expect_worktree = self.project.read(cx).worktree_for_id(worktree_id, cx);
                    }
                }

                if let Some(worktree) = expect_worktree {
                    let worktree = worktree.read(cx);
                    if worktree.entry_for_path(&query_path).is_none()
                        && !query.raw_query.ends_with("/")
                        && !(path_style.is_windows() && query.raw_query.ends_with("\\"))
                    {
                        self.matches.matches.push(Match::CreateNew(ProjectPath {
                            worktree_id: worktree.id(),
                            path: query_path.into_arc(),
                        }));
                    }
                }
            }

            self.selected_index = selected_match.map_or_else(
                || self.calculate_selected_index(cx),
                |m| {
                    self.matches
                        .position(&m, self.currently_opened_path.as_ref())
                        .unwrap_or(0)
                },
            );

            self.latest_search_query = Some(query);
            self.latest_search_did_cancel = did_cancel;

            cx.notify();
        }
    }

    pub(crate) fn labels_for_match(
        &self,
        path_match: &Match,
        window: &mut Window,
        cx: &App,
    ) -> (HighlightedLabel, HighlightedLabel) {
        let path_style = self.project.read(cx).path_style(cx);
        let (file_name, file_name_positions, mut full_path, mut full_path_positions) =
            match &path_match {
                Match::History {
                    path: entry_path,
                    panel_match,
                } => {
                    let worktree_id = entry_path.project.worktree_id;
                    let worktree = self
                        .project
                        .read(cx)
                        .worktree_for_id(worktree_id, cx)
                        .filter(|worktree| worktree.read(cx).is_visible());

                    if let Some(panel_match) = panel_match {
                        self.labels_for_path_match(&panel_match.0, path_style)
                    } else if let Some(worktree) = worktree {
                        let worktree_store = self.project.read(cx).worktree_store();
                        let full_path = if should_hide_root_in_entry_path(&worktree_store, cx) {
                            entry_path.project.path.clone()
                        } else {
                            worktree.read(cx).root_name().join(&entry_path.project.path)
                        };
                        let mut components = full_path.components();
                        let filename = components.next_back().unwrap_or("");
                        let prefix = components.rest();
                        (
                            filename.to_string(),
                            Vec::new(),
                            prefix.display(path_style).to_string() + path_style.primary_separator(),
                            Vec::new(),
                        )
                    } else {
                        (
                            entry_path
                                .absolute
                                .file_name()
                                .map_or(String::new(), |f| f.to_string_lossy().into_owned()),
                            Vec::new(),
                            entry_path.absolute.parent().map_or(String::new(), |path| {
                                path.to_string_lossy().into_owned() + path_style.primary_separator()
                            }),
                            Vec::new(),
                        )
                    }
                }
                Match::Search(path_match) => self.labels_for_path_match(&path_match.0, path_style),
                Match::Channel {
                    channel_name,
                    string_match,
                    ..
                } => (
                    channel_name.to_string(),
                    string_match.positions.clone(),
                    "Channel Notes".to_string(),
                    vec![],
                ),
                Match::CreateNew(project_path) => (
                    format!("Create file: {}", project_path.path.display(path_style)),
                    vec![],
                    String::from(""),
                    vec![],
                ),
            };

        if file_name_positions.is_empty() {
            let user_home_path = inazuma_util::paths::home_dir().to_string_lossy();
            if !user_home_path.is_empty() && full_path.starts_with(&*user_home_path) {
                full_path.replace_range(0..user_home_path.len(), "~");
                full_path_positions.retain_mut(|pos| {
                    if *pos >= user_home_path.len() {
                        *pos -= user_home_path.len();
                        *pos += 1;
                        true
                    } else {
                        false
                    }
                })
            }
        }

        if full_path.is_ascii() {
            let file_finder_settings = FileFinderSettings::get_global(cx);
            let max_width =
                FileFinder::modal_max_width(file_finder_settings.modal_max_width, window);
            let (normal_em, small_em) = {
                let style = window.text_style();
                let font_id = window.text_system().resolve_font(&style.font());
                let font_size = TextSize::Default.rems(cx).to_pixels(window.rem_size());
                let normal = cx
                    .text_system()
                    .em_width(font_id, font_size)
                    .unwrap_or(px(16.));
                let font_size = TextSize::Small.rems(cx).to_pixels(window.rem_size());
                let small = cx
                    .text_system()
                    .em_width(font_id, font_size)
                    .unwrap_or(px(10.));
                (normal, small)
            };
            let budget = full_path_budget(&file_name, normal_em, small_em, max_width);
            // If the computed budget is zero, we certainly won't be able to achieve it,
            // so no point trying to elide the path.
            if budget > 0 && full_path.len() > budget {
                let components = PathComponentSlice::new(&full_path);
                if let Some(elided_range) =
                    components.elision_range(budget - 1, &full_path_positions)
                {
                    let elided_len = elided_range.end - elided_range.start;
                    let placeholder = "…";
                    full_path_positions.retain_mut(|mat| {
                        if *mat >= elided_range.end {
                            *mat -= elided_len;
                            *mat += placeholder.len();
                        } else if *mat >= elided_range.start {
                            return false;
                        }
                        true
                    });
                    full_path.replace_range(elided_range, placeholder);
                }
            }
        }

        (
            HighlightedLabel::new(file_name, file_name_positions),
            HighlightedLabel::new(full_path, full_path_positions)
                .size(LabelSize::Small)
                .color(Color::Muted),
        )
    }

    pub(crate) fn labels_for_path_match(
        &self,
        path_match: &PathMatch,
        path_style: PathStyle,
    ) -> (String, Vec<usize>, String, Vec<usize>) {
        let full_path = path_match.path_prefix.join(&path_match.path);
        let mut path_positions = path_match.positions.clone();

        let file_name = full_path.file_name().unwrap_or("");
        let file_name_start = full_path.as_unix_str().len() - file_name.len();
        let file_name_positions = path_positions
            .iter()
            .filter_map(|pos| {
                if pos >= &file_name_start {
                    Some(pos - file_name_start)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        let full_path = full_path
            .display(path_style)
            .trim_end_matches(&file_name)
            .to_string();
        path_positions.retain(|idx| *idx < full_path.len());

        debug_assert!(
            file_name_positions
                .iter()
                .all(|ix| file_name[*ix..].chars().next().is_some()),
            "invalid file name positions {file_name:?} {file_name_positions:?}"
        );
        debug_assert!(
            path_positions
                .iter()
                .all(|ix| full_path[*ix..].chars().next().is_some()),
            "invalid path positions {full_path:?} {path_positions:?}"
        );

        (
            file_name.to_string(),
            file_name_positions,
            full_path,
            path_positions,
        )
    }

    /// Attempts to resolve an absolute file path and update the search matches if found.
    ///
    /// If the query path resolves to an absolute file that exists in the project,
    /// this method will find the corresponding worktree and relative path, create a
    /// match for it, and update the picker's search results.
    ///
    /// Returns `true` if the absolute path exists, otherwise returns `false`.
    pub(crate) fn lookup_absolute_path(
        &self,
        query: FileSearchQuery,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<bool> {
        cx.spawn_in(window, async move |picker, cx| {
            let Some(project) = picker
                .read_with(cx, |picker, _| picker.delegate.project.clone())
                .log_err()
            else {
                return false;
            };

            let query_path = Path::new(query.path_query());
            let mut path_matches = Vec::new();

            let abs_file_exists = project
                .update(cx, |this, cx| {
                    this.resolve_abs_file_path(query.path_query(), cx)
                })
                .await
                .is_some();

            if abs_file_exists {
                project.update(cx, |project, cx| {
                    if let Some((worktree, relative_path)) = project.find_worktree(query_path, cx) {
                        path_matches.push(ProjectPanelOrdMatch(PathMatch {
                            score: 1.0,
                            positions: Vec::new(),
                            worktree_id: worktree.read(cx).id().to_usize(),
                            path: relative_path,
                            path_prefix: RelPath::empty().into(),
                            is_dir: false, // File finder doesn't support directories
                            distance_to_relative_ancestor: usize::MAX,
                        }));
                    }
                });
            }

            picker
                .update_in(cx, |picker, _, cx| {
                    let picker_delegate = &mut picker.delegate;
                    let search_id = inazuma_util::post_inc(&mut picker_delegate.search_count);
                    picker_delegate.set_search_matches(search_id, false, query, path_matches, cx);

                    anyhow::Ok(())
                })
                .log_err();
            abs_file_exists
        })
    }

    /// Skips first history match (that is displayed topmost) if it's currently opened.
    pub(crate) fn calculate_selected_index(&self, cx: &mut Context<Picker<Self>>) -> usize {
        if FileFinderSettings::get_global(cx).skip_focus_for_active_in_search
            && let Some(Match::History { path, .. }) = self.matches.get(0)
            && Some(path) == self.currently_opened_path.as_ref()
        {
            let elements_after_first = self.matches.len() - 1;
            if elements_after_first > 0 {
                return 1;
            }
        }

        0
    }

    pub(crate) fn key_context(&self, window: &Window, cx: &App) -> KeyContext {
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add("FileFinder");

        if self.filter_popover_menu_handle.is_focused(window, cx) {
            key_context.add("filter_menu_open");
        }

        if self.split_popover_menu_handle.is_focused(window, cx) {
            key_context.add("split_menu_open");
        }
        key_context
    }
}
