//! `PickerDelegate` impl for `FileFinderDelegate`. The render + confirm
//! surface — turns the matched rows into list items, spawns file-open on
//! confirm, handles split-direction / filter popovers.

use carrot_editor::Editor;
use carrot_file_icons::FileIcons;
use carrot_open_path_prompt::file_finder_settings::FileFinderSettings;
use carrot_project::{ProjectPath, WorktreeId};
use carrot_ui::{
    Button, ButtonLike, ButtonStyle, Color, ContextMenu, Icon, IconButton, IconName, IconSize,
    Indicator, KeyBinding, Label, ListItem, ListItemSpacing, PopoverMenu, TintColor, Tooltip,
    h_flex, prelude::*, rems_from_px, v_flex,
};
use carrot_workspace::{
    OpenChannelNotesById, OpenOptions, OpenVisible, Workspace, item::PreviewTabsSettings,
    notifications::NotifyResultExt, pane,
};

use inazuma::{Action as _, AnyElement, App, Context, DismissEvent, Task, Window, px};
use inazuma_picker::{Picker, PickerDelegate};
use inazuma_settings_framework::Settings;
use inazuma_util::{ResultExt, maybe, paths::PathWithPosition, post_inc, rel_path::RelPath};
use std::sync::Arc;

use carrot_actions::search::ToggleIncludeIgnored;

use crate::delegate::FileFinderDelegate;
use crate::matches::{Match, Matches};
use crate::search_query::FileSearchQuery;
use crate::{ToggleFilterMenu, ToggleSplitMenu};

impl PickerDelegate for FileFinderDelegate {
    type ListItem = ListItem;

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "Search project files...".into()
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.has_changed_selected_index = true;
        self.selected_index = ix;
        cx.notify();
    }

    fn separators_after_indices(&self) -> Vec<usize> {
        if self.separate_history {
            let first_non_history_index = self
                .matches
                .matches
                .iter()
                .enumerate()
                .find(|(_, m)| !matches!(m, Match::History { .. }))
                .map(|(i, _)| i);
            if let Some(first_non_history_index) = first_non_history_index
                && first_non_history_index > 0
            {
                return vec![first_non_history_index - 1];
            }
        }
        Vec::new()
    }

    fn update_matches(
        &mut self,
        raw_query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let raw_query = raw_query.replace(' ', "");
        let raw_query = raw_query.trim();

        let raw_query = match &raw_query.get(0..2) {
            Some(".\\" | "./") => &raw_query[2..],
            Some(prefix @ ("a\\" | "a/" | "b\\" | "b/")) => {
                if self
                    .workspace
                    .upgrade()
                    .into_iter()
                    .flat_map(|workspace| workspace.read(cx).worktrees(cx))
                    .all(|worktree| {
                        worktree
                            .read(cx)
                            .entry_for_path(RelPath::unix(prefix.split_at(1).0).unwrap())
                            .is_none_or(|entry| !entry.is_dir())
                    })
                {
                    &raw_query[2..]
                } else {
                    raw_query
                }
            }
            _ => raw_query,
        };

        if raw_query.is_empty() {
            // if there was no query before, and we already have some (history) matches
            // there's no need to update anything, since nothing has changed.
            // We also want to populate matches set from history entries on the first update.
            if self.latest_search_query.is_some() || self.first_update {
                let project = self.project.read(cx);

                self.latest_search_id = post_inc(&mut self.search_count);
                self.latest_search_query = None;
                self.matches = Matches {
                    separate_history: self.separate_history,
                    ..Matches::default()
                };
                let path_style = self.project.read(cx).path_style(cx);

                self.matches.push_new_matches(
                    project.worktree_store(),
                    cx,
                    self.history_items.iter().filter(|history_item| {
                        project
                            .worktree_for_id(history_item.project.worktree_id, cx)
                            .is_some()
                            || project.is_local()
                            || project.is_via_remote_server()
                    }),
                    self.currently_opened_path.as_ref(),
                    None,
                    None.into_iter(),
                    false,
                    path_style,
                );

                self.first_update = false;
                self.selected_index = 0;
            }
            cx.notify();
            Task::ready(())
        } else {
            let path_position = PathWithPosition::parse_str(raw_query);
            let raw_query = raw_query.trim().trim_end_matches(':').to_owned();
            let path = path_position.path.clone();
            let path_str = path_position.path.to_str();
            let path_trimmed = path_str.unwrap_or(&raw_query).trim_end_matches(':');
            let file_query_end = if path_trimmed == raw_query {
                None
            } else {
                // Safe to unwrap as we won't get here when the unwrap in if fails
                Some(path_str.unwrap().len())
            };

            let query = FileSearchQuery {
                raw_query,
                file_query_end,
                path_position,
            };

            cx.spawn_in(window, async move |this, cx| {
                let _ = maybe!(async move {
                    let is_absolute_path = path.is_absolute();
                    let did_resolve_abs_path = is_absolute_path
                        && this
                            .update_in(cx, |this, window, cx| {
                                this.delegate
                                    .lookup_absolute_path(query.clone(), window, cx)
                            })?
                            .await;

                    // Only check for relative paths if no absolute paths were
                    // found.
                    if !did_resolve_abs_path {
                        this.update_in(cx, |this, window, cx| {
                            this.delegate.spawn_search(query, window, cx)
                        })?
                        .await;
                    }
                    anyhow::Ok(())
                })
                .await;
            })
        }
    }

    fn confirm(
        &mut self,
        secondary: bool,
        window: &mut Window,
        cx: &mut Context<Picker<FileFinderDelegate>>,
    ) {
        if let Some(m) = self.matches.get(self.selected_index())
            && let Some(workspace) = self.workspace.upgrade()
        {
            // Channel matches are handled separately since they dispatch an action
            // rather than directly opening a file path.
            if let Match::Channel { channel_id, .. } = m {
                let channel_id = channel_id.0;
                let finder = self.file_finder.clone();
                window.dispatch_action(OpenChannelNotesById { channel_id }.boxed_clone(), cx);
                finder.update(cx, |_, cx| cx.emit(DismissEvent)).log_err();
                return;
            }

            let open_task = workspace.update(cx, |workspace, cx| {
                let split_or_open =
                    |workspace: &mut Workspace,
                     project_path,
                     window: &mut Window,
                     cx: &mut Context<Workspace>| {
                        let allow_preview =
                            PreviewTabsSettings::get_global(cx).enable_preview_from_file_finder;
                        if secondary {
                            workspace.split_path_preview(
                                project_path,
                                allow_preview,
                                None,
                                window,
                                cx,
                            )
                        } else {
                            workspace.open_path_preview(
                                project_path,
                                None,
                                true,
                                allow_preview,
                                true,
                                window,
                                cx,
                            )
                        }
                    };
                match &m {
                    Match::CreateNew(project_path) => {
                        // Create a new file with the given filename
                        if secondary {
                            workspace.split_path_preview(
                                project_path.clone(),
                                false,
                                None,
                                window,
                                cx,
                            )
                        } else {
                            workspace.open_path_preview(
                                project_path.clone(),
                                None,
                                true,
                                false,
                                true,
                                window,
                                cx,
                            )
                        }
                    }

                    Match::History { path, .. } => {
                        let worktree_id = path.project.worktree_id;
                        if workspace
                            .project()
                            .read(cx)
                            .worktree_for_id(worktree_id, cx)
                            .is_some()
                        {
                            split_or_open(
                                workspace,
                                ProjectPath {
                                    worktree_id,
                                    path: Arc::clone(&path.project.path),
                                },
                                window,
                                cx,
                            )
                        } else if secondary {
                            workspace.split_abs_path(path.absolute.clone(), false, window, cx)
                        } else {
                            workspace.open_abs_path(
                                path.absolute.clone(),
                                OpenOptions {
                                    visible: Some(OpenVisible::None),
                                    ..Default::default()
                                },
                                window,
                                cx,
                            )
                        }
                    }
                    Match::Search(m) => split_or_open(
                        workspace,
                        ProjectPath {
                            worktree_id: WorktreeId::from_usize(m.0.worktree_id),
                            path: m.0.path.clone(),
                        },
                        window,
                        cx,
                    ),
                    Match::Channel { .. } => unreachable!("handled above"),
                }
            });

            let row = self
                .latest_search_query
                .as_ref()
                .and_then(|query| query.path_position.row)
                .map(|row| row.saturating_sub(1));
            let col = self
                .latest_search_query
                .as_ref()
                .and_then(|query| query.path_position.column)
                .unwrap_or(0)
                .saturating_sub(1);
            let finder = self.file_finder.clone();
            let workspace = self.workspace.clone();

            cx.spawn_in(window, async move |_, mut cx| {
                let item = open_task
                    .await
                    .notify_workspace_async_err(workspace, &mut cx)?;
                if let Some(row) = row
                    && let Some(active_editor) = item.downcast::<Editor>()
                {
                    active_editor
                        .downgrade()
                        .update_in(cx, |editor, window, cx| {
                            let Some(buffer) = editor.buffer().read(cx).as_singleton() else {
                                return;
                            };
                            let buffer_snapshot = buffer.read(cx).snapshot();
                            let point = buffer_snapshot.point_from_external_input(row, col);
                            editor.go_to_singleton_buffer_point(point, window, cx);
                        })
                        .log_err();
                }
                finder.update(cx, |_, cx| cx.emit(DismissEvent)).ok()?;

                Some(())
            })
            .detach();
        }
    }

    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<FileFinderDelegate>>) {
        self.file_finder
            .update(cx, |_, cx| cx.emit(DismissEvent))
            .log_err();
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let settings = FileFinderSettings::get_global(cx);

        let path_match = self.matches.get(ix)?;

        let end_icon = match path_match {
            Match::History { .. } => Icon::new(IconName::HistoryRerun)
                .color(Color::Muted)
                .size(IconSize::Small)
                .into_any_element(),
            Match::Search(_) => v_flex()
                .flex_none()
                .size(IconSize::Small.rems())
                .into_any_element(),
            Match::Channel { .. } => v_flex()
                .flex_none()
                .size(IconSize::Small.rems())
                .into_any_element(),
            Match::CreateNew(_) => Icon::new(IconName::Plus)
                .color(Color::Muted)
                .size(IconSize::Small)
                .into_any_element(),
        };
        let (file_name_label, full_path_label) = self.labels_for_match(path_match, window, cx);

        let file_icon = match path_match {
            Match::Channel { .. } => Some(Icon::new(IconName::Hash).color(Color::Muted)),
            _ => maybe!({
                if !settings.file_icons {
                    return None;
                }
                let abs_path = path_match.abs_path(&self.project, cx)?;
                let file_name = abs_path.file_name()?;
                let icon = FileIcons::get_icon(file_name.as_ref(), cx)?;
                Some(Icon::from_path(icon).color(Color::Muted))
            }),
        };

        Some(
            ListItem::new(ix)
                .spacing(ListItemSpacing::Sparse)
                .start_slot::<Icon>(file_icon)
                .end_slot::<AnyElement>(end_icon)
                .inset(true)
                .toggle_state(selected)
                .child(
                    h_flex()
                        .gap_2()
                        .py_px()
                        .child(file_name_label)
                        .child(full_path_label),
                ),
        )
    }

    fn render_footer(&self, _: &mut Window, cx: &mut Context<Picker<Self>>) -> Option<AnyElement> {
        let focus_handle = self.focus_handle.clone();

        Some(
            h_flex()
                .w_full()
                .p_1p5()
                .justify_between()
                .border_t_1()
                .border_color(cx.theme().colors().border_variant)
                .child(
                    PopoverMenu::new("filter-menu-popover")
                        .with_handle(self.filter_popover_menu_handle.clone())
                        .attach(inazuma::Corner::BottomRight)
                        .anchor(inazuma::Corner::BottomLeft)
                        .offset(inazuma::Point {
                            x: px(1.0),
                            y: px(1.0),
                        })
                        .trigger_with_tooltip(
                            IconButton::new("filter-trigger", IconName::Sliders)
                                .icon_size(IconSize::Small)
                                .icon_size(IconSize::Small)
                                .toggle_state(self.include_ignored.unwrap_or(false))
                                .when(self.include_ignored.is_some(), |this| {
                                    this.indicator(Indicator::dot().color(Color::Info))
                                }),
                            {
                                let focus_handle = focus_handle.clone();
                                move |_window, cx| {
                                    Tooltip::for_action_in(
                                        "Filter Options",
                                        &ToggleFilterMenu,
                                        &focus_handle,
                                        cx,
                                    )
                                }
                            },
                        )
                        .menu({
                            let focus_handle = focus_handle.clone();
                            let include_ignored = self.include_ignored;

                            move |window, cx| {
                                Some(ContextMenu::build(window, cx, {
                                    let focus_handle = focus_handle.clone();
                                    move |menu, _, _| {
                                        menu.context(focus_handle.clone())
                                            .header("Filter Options")
                                            .toggleable_entry(
                                                "Include Ignored Files",
                                                include_ignored.unwrap_or(false),
                                                carrot_ui::IconPosition::End,
                                                Some(ToggleIncludeIgnored.boxed_clone()),
                                                move |window, cx| {
                                                    window.focus(&focus_handle, cx);
                                                    window.dispatch_action(
                                                        ToggleIncludeIgnored.boxed_clone(),
                                                        cx,
                                                    );
                                                },
                                            )
                                    }
                                }))
                            }
                        }),
                )
                .child(
                    h_flex()
                        .gap_0p5()
                        .child(
                            PopoverMenu::new("split-menu-popover")
                                .with_handle(self.split_popover_menu_handle.clone())
                                .attach(inazuma::Corner::BottomRight)
                                .anchor(inazuma::Corner::BottomLeft)
                                .offset(inazuma::Point {
                                    x: px(1.0),
                                    y: px(1.0),
                                })
                                .trigger(
                                    ButtonLike::new("split-trigger")
                                        .child(Label::new("Split…"))
                                        .selected_style(ButtonStyle::tinted(TintColor::Accent))
                                        .child(
                                            KeyBinding::for_action_in(
                                                &ToggleSplitMenu,
                                                &focus_handle,
                                                cx,
                                            )
                                            .size(rems_from_px(12.)),
                                        ),
                                )
                                .menu({
                                    let focus_handle = focus_handle.clone();

                                    move |window, cx| {
                                        Some(ContextMenu::build(window, cx, {
                                            let focus_handle = focus_handle.clone();
                                            move |menu, _, _| {
                                                menu.context(focus_handle)
                                                    .action(
                                                        "Split Left",
                                                        pane::SplitLeft::default().boxed_clone(),
                                                    )
                                                    .action(
                                                        "Split Right",
                                                        pane::SplitRight::default().boxed_clone(),
                                                    )
                                                    .action(
                                                        "Split Up",
                                                        pane::SplitUp::default().boxed_clone(),
                                                    )
                                                    .action(
                                                        "Split Down",
                                                        pane::SplitDown::default().boxed_clone(),
                                                    )
                                            }
                                        }))
                                    }
                                }),
                        )
                        .child(
                            Button::new("open-selection", "Open")
                                .key_binding(
                                    KeyBinding::for_action_in(
                                        &inazuma_menu::Confirm,
                                        &focus_handle,
                                        cx,
                                    )
                                    .map(|kb| kb.size(rems_from_px(12.))),
                                )
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(inazuma_menu::Confirm.boxed_clone(), cx)
                                }),
                        ),
                )
                .into_any(),
        )
    }
}
