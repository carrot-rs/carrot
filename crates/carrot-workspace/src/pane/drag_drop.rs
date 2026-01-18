use super::*;

impl Pane {
    pub(crate) fn handle_drag_move<T: 'static>(
        &mut self,
        event: &DragMoveEvent<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let can_split_predicate = self.can_split_predicate.take();
        let can_split = match &can_split_predicate {
            Some(can_split_predicate) => {
                can_split_predicate(self, event.dragged_item(), window, cx)
            }
            None => false,
        };
        self.can_split_predicate = can_split_predicate;
        if !can_split {
            return;
        }

        let rect = event.bounds.size;

        let size = event.bounds.size.width.min(event.bounds.size.height)
            * WorkspaceSettings::get_global(cx).drop_target_size;

        let relative_cursor = Point::new(
            event.event.position.x - event.bounds.left(),
            event.event.position.y - event.bounds.top(),
        );

        let direction = if relative_cursor.x < size
            || relative_cursor.x > rect.width - size
            || relative_cursor.y < size
            || relative_cursor.y > rect.height - size
        {
            [
                SplitDirection::Up,
                SplitDirection::Right,
                SplitDirection::Down,
                SplitDirection::Left,
            ]
            .iter()
            .min_by_key(|side| match side {
                SplitDirection::Up => relative_cursor.y,
                SplitDirection::Right => rect.width - relative_cursor.x,
                SplitDirection::Down => rect.height - relative_cursor.y,
                SplitDirection::Left => relative_cursor.x,
            })
            .cloned()
        } else {
            None
        };

        if direction != self.drag_split_direction {
            self.drag_split_direction = direction;
        }
    }

    pub fn handle_tab_drop(
        &mut self,
        dragged_tab: &DraggedTab,
        ix: usize,
        is_pane_target: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if is_pane_target
            && ix == self.active_item_index()
            && let Some(active_item) = self.active_item()
            && active_item.handle_drop(self, dragged_tab, window, cx)
        {
            return;
        }

        let mut to_pane = cx.entity();
        let split_direction = self.drag_split_direction;
        let item_id = dragged_tab.item.item_id();
        self.unpreview_item_if_preview(item_id);

        let is_clone = cfg!(target_os = "macos") && window.modifiers().alt
            || cfg!(not(target_os = "macos")) && window.modifiers().control;

        let from_pane = dragged_tab.pane.clone();

        self.workspace
            .update(cx, |_, cx| {
                cx.defer_in(window, move |workspace, window, cx| {
                    if let Some(split_direction) = split_direction {
                        to_pane = workspace.split_pane(to_pane, split_direction, window, cx);
                    }
                    let database_id = workspace.database_id();
                    if is_clone {
                        let Some(item) = from_pane
                            .read(cx)
                            .items()
                            .find(|item| item.item_id() == item_id)
                            .cloned()
                        else {
                            return;
                        };
                        if item.can_split(cx) {
                            let task = item.clone_on_split(database_id, window, cx);
                            let to_pane = to_pane.downgrade();
                            cx.spawn_in(window, async move |_, cx| {
                                if let Some(item) = task.await {
                                    to_pane
                                        .update_in(cx, |pane, window, cx| {
                                            pane.add_item(item, true, true, None, window, cx)
                                        })
                                        .ok();
                                }
                            })
                            .detach();
                        } else {
                            move_item(&from_pane, &to_pane, item_id, ix, true, window, cx);
                        }
                    } else {
                        move_item(&from_pane, &to_pane, item_id, ix, true, window, cx);
                    }
                    // Single-item pane has no pinned tab tracking
                });
            })
            .log_err();
    }

    pub(crate) fn handle_dragged_selection_drop(
        &mut self,
        dragged_selection: &DraggedSelection,
        dragged_onto: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(active_item) = self.active_item()
            && active_item.handle_drop(self, dragged_selection, window, cx)
        {
            return;
        }

        self.handle_project_entry_drop(
            &dragged_selection.active_selection.entry_id,
            dragged_onto,
            window,
            cx,
        );
    }

    fn handle_project_entry_drop(
        &mut self,
        project_entry_id: &ProjectEntryId,
        target: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(active_item) = self.active_item()
            && active_item.handle_drop(self, project_entry_id, window, cx)
        {
            return;
        }

        let mut to_pane = cx.entity();
        let split_direction = self.drag_split_direction;
        let project_entry_id = *project_entry_id;
        self.workspace
            .update(cx, |_, cx| {
                cx.defer_in(window, move |workspace, window, cx| {
                    if let Some(project_path) = workspace
                        .project()
                        .read(cx)
                        .path_for_entry(project_entry_id, cx)
                    {
                        let load_path_task = workspace.load_path(project_path.clone(), window, cx);
                        cx.spawn_in(window, async move |workspace, mut cx| {
                            if let Some((project_entry_id, build_item)) = load_path_task
                                .await
                                .notify_workspace_async_err(workspace.clone(), &mut cx)
                            {
                                workspace
                                    .update_in(cx, |workspace, window, cx| {
                                        if let Some(split_direction) = split_direction {
                                            to_pane = workspace.split_pane(
                                                to_pane,
                                                split_direction,
                                                window,
                                                cx,
                                            );
                                        }
                                        to_pane.update(cx, |pane, cx| {
                                            pane.open_item(
                                                project_entry_id,
                                                project_path,
                                                true,
                                                false,
                                                true,
                                                target,
                                                window,
                                                cx,
                                                build_item,
                                            );
                                        });
                                    })
                                    .log_err()?;
                            }
                            Some(())
                        })
                        .detach();
                    };
                });
            })
            .log_err();
    }

    pub(super) fn handle_external_paths_drop(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(active_item) = self.active_item()
            && active_item.handle_drop(self, paths, window, cx)
        {
            return;
        }

        let mut to_pane = cx.entity();
        let mut split_direction = self.drag_split_direction;
        let paths = paths.paths().to_vec();
        let is_remote = self
            .workspace
            .update(cx, |workspace, cx| {
                if workspace.project().read(cx).is_via_collab() {
                    workspace.show_error(
                        &anyhow::anyhow!("Cannot drop files on a remote project"),
                        cx,
                    );
                    true
                } else {
                    false
                }
            })
            .unwrap_or(true);
        if is_remote {
            return;
        }

        self.workspace
            .update(cx, |workspace, cx| {
                let fs = Arc::clone(workspace.project().read(cx).fs());
                cx.spawn_in(window, async move |workspace, cx| {
                    let mut is_file_checks = FuturesUnordered::new();
                    for path in &paths {
                        is_file_checks.push(fs.is_file(path))
                    }
                    let mut has_files_to_open = false;
                    while let Some(is_file) = is_file_checks.next().await {
                        if is_file {
                            has_files_to_open = true;
                            break;
                        }
                    }
                    drop(is_file_checks);
                    if !has_files_to_open {
                        split_direction = None;
                    }

                    if let Ok((open_task, to_pane)) =
                        workspace.update_in(cx, |workspace, window, cx| {
                            if let Some(split_direction) = split_direction {
                                to_pane =
                                    workspace.split_pane(to_pane, split_direction, window, cx);
                            }
                            (
                                workspace.open_paths(
                                    paths,
                                    OpenOptions {
                                        visible: Some(OpenVisible::OnlyDirectories),
                                        ..Default::default()
                                    },
                                    Some(to_pane.downgrade()),
                                    window,
                                    cx,
                                ),
                                to_pane,
                            )
                        })
                    {
                        let opened_items: Vec<_> = open_task.await;
                        _ = workspace.update_in(cx, |workspace, window, cx| {
                            for item in opened_items.into_iter().flatten() {
                                if let Err(e) = item {
                                    workspace.show_error(&e, cx);
                                }
                            }
                            if to_pane.read(cx).items_len() == 0 {
                                workspace.remove_pane(to_pane, None, window, cx);
                            }
                        });
                    }
                })
                .detach();
            })
            .log_err();
    }

    pub fn drag_split_direction(&self) -> Option<SplitDirection> {
        self.drag_split_direction
    }
}
