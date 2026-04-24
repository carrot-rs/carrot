//! Library root for `carrot-file-finder`. Hosts the `FileFinder` modal, its
//! init + action wiring, and the `Render` surface. The picker's heavy lifting
//! (matching, ranking, path rendering, picker-delegate impl, live walking) is
//! split across sibling modules — this file is the composition point.

#[cfg(test)]
mod file_finder_tests;

pub mod delegate;
pub(crate) mod finder_mode;
pub mod history;
pub(crate) mod live_candidates;
pub mod live_walk_cache;
pub mod live_walker;
pub mod matches;
pub mod path_render;
pub mod picker;
pub mod search_query;

pub use carrot_open_path_prompt::OpenPathDelegate;
pub use delegate::FileFinderDelegate;
pub use live_walk_cache::{CacheEntry, LiveWalkCache};
pub use live_walker::{LiveWalker, LiveWalkerConfig, WalkResult};

use carrot_actions::search::ToggleIncludeIgnored;
use carrot_open_path_prompt::{
    OpenPathPrompt,
    file_finder_settings::{FileFinderSettings, FileFinderWidth},
};
use carrot_project::{ProjectPath, WorktreeId};
use carrot_ui::{prelude::*, v_flex};
use carrot_workspace::{ModalView, SplitDirection, Workspace, pane};
use futures::future::join_all;
use inazuma::{
    Action, App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Modifiers,
    ModifiersChangedEvent, ParentElement, Pixels, Render, Styled, Task, Window, actions, px, rems,
};
use inazuma_picker::{Picker, PickerDelegate};
use inazuma_settings_framework::Settings;
use std::sync::Arc;

use crate::history::{FoundPath, MAX_RECENT_SELECTIONS};
use crate::matches::Match;

actions!(
    file_finder,
    [
        /// Selects the previous item in the file finder.
        SelectPrevious,
        /// Toggles the file filter menu.
        ToggleFilterMenu,
        /// Toggles the split direction menu.
        ToggleSplitMenu
    ]
);

impl ModalView for FileFinder {
    fn on_before_dismiss(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> carrot_workspace::DismissDecision {
        let submenu_focused = self.picker.update(cx, |picker, cx| {
            picker
                .delegate
                .filter_popover_menu_handle
                .is_focused(window, cx)
                || picker
                    .delegate
                    .split_popover_menu_handle
                    .is_focused(window, cx)
        });
        carrot_workspace::DismissDecision::Dismiss(!submenu_focused)
    }
}

pub struct FileFinder {
    picker: Entity<Picker<FileFinderDelegate>>,
    picker_focus_handle: FocusHandle,
    init_modifiers: Option<Modifiers>,
}

pub fn init(cx: &mut App) {
    live_walk_cache::init(cx);
    cx.observe_new(FileFinder::register).detach();
    cx.observe_new(OpenPathPrompt::register).detach();
    cx.observe_new(OpenPathPrompt::register_new_path).detach();
}

pub enum Event {
    Selected(ProjectPath),
    Dismissed,
}

impl FileFinder {
    fn register(
        workspace: &mut Workspace,
        _window: Option<&mut Window>,
        _: &mut Context<Workspace>,
    ) {
        workspace.register_action(
            |workspace, action: &carrot_workspace::ToggleFileFinder, window, cx| {
                let Some(file_finder) = workspace.active_modal::<Self>(cx) else {
                    Self::open(workspace, action.separate_history, window, cx).detach();
                    return;
                };

                file_finder.update(cx, |file_finder, cx| {
                    file_finder.init_modifiers = Some(window.modifiers());
                    file_finder.picker.update(cx, |picker, cx| {
                        picker.cycle_selection(window, cx);
                    });
                });
            },
        );
    }

    fn open(
        workspace: &mut Workspace,
        separate_history: bool,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<()> {
        let project = workspace.project().read(cx);
        let fs = project.fs();

        let currently_opened_path = workspace.active_item(cx).and_then(|item| {
            let project_path = item.project_path(cx)?;
            let abs_path = project
                .worktree_for_id(project_path.worktree_id, cx)?
                .read(cx)
                .absolutize(&project_path.path);
            Some(FoundPath::new(project_path, abs_path))
        });

        let history_items = workspace
            .recent_navigation_history(Some(MAX_RECENT_SELECTIONS), cx)
            .into_iter()
            .filter_map(|(project_path, abs_path)| {
                if project.entry_for_path(&project_path, cx).is_some() {
                    return Some(Task::ready(Some(FoundPath::new(project_path, abs_path?))));
                }
                let abs_path = abs_path?;
                if project.is_local() {
                    let fs = fs.clone();
                    Some(cx.background_spawn(async move {
                        if fs.is_file(&abs_path).await {
                            Some(FoundPath::new(project_path, abs_path))
                        } else {
                            None
                        }
                    }))
                } else {
                    Some(Task::ready(Some(FoundPath::new(project_path, abs_path))))
                }
            })
            .collect::<Vec<_>>();
        cx.spawn_in(window, async move |workspace, cx| {
            let history_items = join_all(history_items).await.into_iter().flatten();

            workspace
                .update_in(cx, |workspace, window, cx| {
                    let project = workspace.project().clone();
                    let weak_workspace = cx.entity().downgrade();
                    workspace.toggle_modal(window, cx, |window, cx| {
                        let delegate = FileFinderDelegate::new(
                            cx.entity().downgrade(),
                            weak_workspace,
                            project,
                            currently_opened_path,
                            history_items.collect(),
                            separate_history,
                            window,
                            cx,
                        );

                        FileFinder::new(delegate, window, cx)
                    });
                })
                .ok();
        })
    }

    fn new(delegate: FileFinderDelegate, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let picker = cx.new(|cx| Picker::uniform_list(delegate, window, cx));
        let picker_focus_handle = picker.focus_handle(cx);
        picker.update(cx, |picker, _| {
            picker.delegate.focus_handle = picker_focus_handle.clone();
        });
        Self {
            picker,
            picker_focus_handle,
            init_modifiers: window.modifiers().modified().then_some(window.modifiers()),
        }
    }

    fn handle_modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(init_modifiers) = self.init_modifiers.take() else {
            return;
        };
        if self.picker.read(cx).delegate.has_changed_selected_index
            && (!event.modified() || !init_modifiers.is_subset_of(event))
        {
            self.init_modifiers = None;
            window.dispatch_action(inazuma_menu::Confirm.boxed_clone(), cx);
        }
    }

    fn handle_select_prev(
        &mut self,
        _: &SelectPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.init_modifiers = Some(window.modifiers());
        window.dispatch_action(Box::new(inazuma_menu::SelectPrevious), cx);
    }

    fn handle_filter_toggle_menu(
        &mut self,
        _: &ToggleFilterMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.update(cx, |picker, cx| {
            let menu_handle = &picker.delegate.filter_popover_menu_handle;
            if menu_handle.is_deployed() {
                menu_handle.hide(cx);
            } else {
                menu_handle.show(window, cx);
            }
        });
    }

    fn handle_split_toggle_menu(
        &mut self,
        _: &ToggleSplitMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.update(cx, |picker, cx| {
            let menu_handle = &picker.delegate.split_popover_menu_handle;
            if menu_handle.is_deployed() {
                menu_handle.hide(cx);
            } else {
                menu_handle.show(window, cx);
            }
        });
    }

    fn handle_toggle_ignored(
        &mut self,
        _: &ToggleIncludeIgnored,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.update(cx, |picker, cx| {
            picker.delegate.include_ignored = match picker.delegate.include_ignored {
                Some(true) => FileFinderSettings::get_global(cx)
                    .include_ignored
                    .map(|_| false),
                Some(false) => Some(true),
                None => Some(true),
            };
            picker.delegate.include_ignored_refresh =
                picker.delegate.update_matches(picker.query(cx), window, cx);
        });
    }

    fn go_to_file_split_left(
        &mut self,
        _: &pane::SplitLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.go_to_file_split_inner(SplitDirection::Left, window, cx)
    }

    fn go_to_file_split_right(
        &mut self,
        _: &pane::SplitRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.go_to_file_split_inner(SplitDirection::Right, window, cx)
    }

    fn go_to_file_split_up(
        &mut self,
        _: &pane::SplitUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.go_to_file_split_inner(SplitDirection::Up, window, cx)
    }

    fn go_to_file_split_down(
        &mut self,
        _: &pane::SplitDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.go_to_file_split_inner(SplitDirection::Down, window, cx)
    }

    fn go_to_file_split_inner(
        &mut self,
        split_direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.update(cx, |picker, cx| {
            let delegate = &mut picker.delegate;
            if let Some(workspace) = delegate.workspace.upgrade()
                && let Some(m) = delegate.matches.get(delegate.selected_index())
            {
                let path = match m {
                    Match::History { path, .. } => {
                        let worktree_id = path.project.worktree_id;
                        ProjectPath {
                            worktree_id,
                            path: Arc::clone(&path.project.path),
                        }
                    }
                    Match::Search(m) => ProjectPath {
                        worktree_id: WorktreeId::from_usize(m.0.worktree_id),
                        path: m.0.path.clone(),
                    },
                    Match::CreateNew(p) => p.clone(),
                    Match::Channel { .. } => return,
                };
                let open_task = workspace.update(cx, move |workspace, cx| {
                    workspace.split_path_preview(path, false, Some(split_direction), window, cx)
                });
                open_task.detach_and_log_err(cx);
            }
        })
    }

    pub fn modal_max_width(width_setting: FileFinderWidth, window: &mut Window) -> Pixels {
        let window_width = window.viewport_size().width;
        let small_width = rems(34.).to_pixels(window.rem_size());

        match width_setting {
            FileFinderWidth::Small => small_width,
            FileFinderWidth::Full => window_width,
            FileFinderWidth::XLarge => (window_width - px(512.)).max(small_width),
            FileFinderWidth::Large => (window_width - px(768.)).max(small_width),
            FileFinderWidth::Medium => (window_width - px(1024.)).max(small_width),
        }
    }
}

impl EventEmitter<DismissEvent> for FileFinder {}

impl Focusable for FileFinder {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.picker_focus_handle.clone()
    }
}

impl Render for FileFinder {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let key_context = self.picker.read(cx).delegate.key_context(window, cx);

        let file_finder_settings = FileFinderSettings::get_global(cx);
        let modal_max_width = Self::modal_max_width(file_finder_settings.modal_max_width, window);

        v_flex()
            .key_context(key_context)
            .w(modal_max_width)
            .on_modifiers_changed(cx.listener(Self::handle_modifiers_changed))
            .on_action(cx.listener(Self::handle_select_prev))
            .on_action(cx.listener(Self::handle_filter_toggle_menu))
            .on_action(cx.listener(Self::handle_split_toggle_menu))
            .on_action(cx.listener(Self::handle_toggle_ignored))
            .on_action(cx.listener(Self::go_to_file_split_left))
            .on_action(cx.listener(Self::go_to_file_split_right))
            .on_action(cx.listener(Self::go_to_file_split_up))
            .on_action(cx.listener(Self::go_to_file_split_down))
            .child(self.picker.clone())
    }
}
