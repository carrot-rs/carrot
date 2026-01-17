use super::*;

impl Pane {
    pub fn close_active_item(
        &mut self,
        action: &CloseActiveItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        if self.item.is_none() {
            // Close the window when there's no active items to close, if configured
            if WorkspaceSettings::get_global(cx)
                .when_closing_with_no_tabs
                .should_close()
            {
                window.dispatch_action(Box::new(CloseWindow), cx);
            }

            return Task::ready(Ok(()));
        }
        // Single-item pane has no pinned tabs, so we always proceed to close

        let active_item_id = self.active_item_id();

        self.close_item_by_id(
            active_item_id,
            action.save_intent.unwrap_or(SaveIntent::Close),
            window,
            cx,
        )
    }

    pub fn close_item_by_id(
        &mut self,
        item_id_to_close: EntityId,
        save_intent: SaveIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        self.close_items(window, cx, save_intent, &move |view_id| {
            view_id == item_id_to_close
        })
    }

    pub fn close_items_for_project_path(
        &mut self,
        project_path: &ProjectPath,
        save_intent: SaveIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let matching_item_ids: Vec<_> = self
            .items()
            .filter(|item| item.project_path(cx).as_ref() == Some(project_path))
            .map(|item| item.item_id())
            .collect();
        self.close_items(window, cx, save_intent, &move |item_id| {
            matching_item_ids.contains(&item_id)
        })
    }

    pub fn close_other_items(
        &mut self,
        action: &CloseOtherItems,
        target_item_id: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        if self.item.is_none() {
            return Task::ready(Ok(()));
        }

        let active_item_id = match target_item_id {
            Some(result) => result,
            None => self.active_item_id(),
        };

        self.close_items(
            window,
            cx,
            action.save_intent.unwrap_or(SaveIntent::Close),
            &move |item_id| item_id != active_item_id,
        )
    }

    pub fn close_multibuffer_items(
        &mut self,
        action: &CloseMultibufferItems,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        if self.item.is_none() {
            return Task::ready(Ok(()));
        }

        let multibuffer_items = self.multibuffer_item_ids(cx);

        self.close_items(
            window,
            cx,
            action.save_intent.unwrap_or(SaveIntent::Close),
            &move |item_id| multibuffer_items.contains(&item_id),
        )
    }

    pub fn close_clean_items(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        if self.item.is_none() {
            return Task::ready(Ok(()));
        }

        let clean_item_ids = self.clean_item_ids(cx);

        self.close_items(window, cx, SaveIntent::Close, &move |item_id| {
            clean_item_ids.contains(&item_id)
        })
    }

    pub fn close_items_to_the_left_by_id(
        &mut self,
        item_id: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        self.close_items_to_the_side_by_id(item_id, Side::Left, window, cx)
    }

    pub fn close_items_to_the_right_by_id(
        &mut self,
        item_id: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        self.close_items_to_the_side_by_id(item_id, Side::Right, window, cx)
    }

    pub fn close_items_to_the_side_by_id(
        &mut self,
        item_id: Option<EntityId>,
        side: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        if self.item.is_none() {
            return Task::ready(Ok(()));
        }

        let item_id = item_id.unwrap_or_else(|| self.active_item_id());
        let to_the_side_item_ids = self.to_the_side_item_ids(item_id, side);

        self.close_items(window, cx, SaveIntent::Close, &move |item_id| {
            to_the_side_item_ids.contains(&item_id)
        })
    }

    pub fn close_all_items(
        &mut self,
        action: &CloseAllItems,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        if self.item.is_none() {
            return Task::ready(Ok(()));
        }

        self.close_items(
            window,
            cx,
            action.save_intent.unwrap_or(SaveIntent::Close),
            &|_item_id| true,
        )
    }

    // Usually when you close an item that has unsaved changes, we prompt you to
    // save it. That said, if you still have the buffer open in a different pane
    // we can close this one without fear of losing data.
    pub fn skip_save_on_close(item: &dyn ItemHandle, workspace: &Workspace, cx: &App) -> bool {
        let mut dirty_project_item_ids = Vec::new();
        item.for_each_project_item(cx, &mut |project_item_id, project_item| {
            if project_item.is_dirty() {
                dirty_project_item_ids.push(project_item_id);
            }
        });
        if dirty_project_item_ids.is_empty() {
            return !(item.buffer_kind(cx) == ItemBufferKind::Singleton && item.is_dirty(cx));
        }

        for open_item in workspace.items(cx) {
            if open_item.item_id() == item.item_id() {
                continue;
            }
            if open_item.buffer_kind(cx) != ItemBufferKind::Singleton {
                continue;
            }
            let other_project_item_ids = open_item.project_item_model_ids(cx);
            dirty_project_item_ids.retain(|id| !other_project_item_ids.contains(id));
        }
        dirty_project_item_ids.is_empty()
    }

    pub(crate) fn file_names_for_prompt(
        items: &mut dyn Iterator<Item = &Box<dyn ItemHandle>>,
        cx: &App,
    ) -> String {
        let mut file_names = BTreeSet::default();
        for item in items {
            item.for_each_project_item(cx, &mut |_, project_item| {
                if !project_item.is_dirty() {
                    return;
                }
                let filename = project_item
                    .project_path(cx)
                    .and_then(|path| path.path.file_name().map(ToOwned::to_owned));
                file_names.insert(filename.unwrap_or("untitled".to_string()));
            });
        }
        if file_names.len() > 6 {
            format!(
                "{}\n.. and {} more",
                file_names.iter().take(5).join("\n"),
                file_names.len() - 5
            )
        } else {
            file_names.into_iter().join("\n")
        }
    }

    pub fn close_items(
        &self,
        window: &mut Window,
        cx: &mut Context<Pane>,
        mut save_intent: SaveIntent,
        should_close: &dyn Fn(EntityId) -> bool,
    ) -> Task<Result<()>> {
        // Find the items to close.
        let mut items_to_close = Vec::new();
        if let Some(item) = &self.item {
            if should_close(item.item_id()) {
                items_to_close.push(item.boxed_clone());
            }
        }

        let active_item_id = self.active_item().map(|item| item.item_id());

        items_to_close.sort_by_key(|item| {
            let path = item.project_path(cx);
            // Put the currently active item at the end, because if the currently active item is not closed last
            // closing the currently active item will cause the focus to switch to another item
            // This will cause Carrot to expand the content of the currently active item
            //
            // Beyond that sort in order of project path, with untitled files and multibuffers coming last.
            (active_item_id == Some(item.item_id()), path.is_none(), path)
        });

        let workspace = self.workspace.clone();
        let Some(project) = self.project.upgrade() else {
            return Task::ready(Ok(()));
        };
        cx.spawn_in(window, async move |pane, cx| {
            let dirty_items = workspace.update(cx, |workspace, cx| {
                items_to_close
                    .iter()
                    .filter(|item| {
                        item.is_dirty(cx) && !Self::skip_save_on_close(item.as_ref(), workspace, cx)
                    })
                    .map(|item| item.boxed_clone())
                    .collect::<Vec<_>>()
            })?;

            if save_intent == SaveIntent::Close && dirty_items.len() > 1 {
                let answer = pane.update_in(cx, |_, window, cx| {
                    let detail = Self::file_names_for_prompt(&mut dirty_items.iter(), cx);
                    window.prompt(
                        PromptLevel::Warning,
                        "Do you want to save changes to the following files?",
                        Some(&detail),
                        &["Save all", "Discard all", "Cancel"],
                        cx,
                    )
                })?;
                match answer.await {
                    Ok(0) => save_intent = SaveIntent::SaveAll,
                    Ok(1) => save_intent = SaveIntent::Skip,
                    Ok(2) => return Ok(()),
                    _ => {}
                }
            }

            for item_to_close in items_to_close {
                let mut should_close = true;
                let mut should_save = true;
                if save_intent == SaveIntent::Close {
                    workspace.update(cx, |workspace, cx| {
                        if Self::skip_save_on_close(item_to_close.as_ref(), workspace, cx) {
                            should_save = false;
                        }
                    })?;
                }

                if should_save {
                    match Self::save_item(project.clone(), &pane, &*item_to_close, save_intent, cx)
                        .await
                    {
                        Ok(success) => {
                            if !success {
                                should_close = false;
                            }
                        }
                        Err(err) => {
                            let answer = pane.update_in(cx, |_, window, cx| {
                                let detail = Self::file_names_for_prompt(
                                    &mut [&item_to_close].into_iter(),
                                    cx,
                                );
                                window.prompt(
                                    PromptLevel::Warning,
                                    &format!("Unable to save file: {}", &err),
                                    Some(&detail),
                                    &["Close Without Saving", "Cancel"],
                                    cx,
                                )
                            })?;
                            match answer.await {
                                Ok(0) => {}
                                Ok(1..) | Err(_) => should_close = false,
                            }
                        }
                    }
                }

                // Remove the item from the pane.
                if should_close {
                    pane.update_in(cx, |pane, window, cx| {
                        pane.remove_item(
                            item_to_close.item_id(),
                            false,
                            pane.close_pane_if_empty,
                            window,
                            cx,
                        );
                    })
                    .ok();
                }
            }

            pane.update(cx, |_, cx| cx.notify()).ok();
            Ok(())
        })
    }

    pub fn take_active_item(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Box<dyn ItemHandle>> {
        let item = self.active_item()?;
        self.remove_item(item.item_id(), false, false, window, cx);
        Some(item)
    }

    pub fn remove_item(
        &mut self,
        item_id: EntityId,
        activate_pane: bool,
        close_pane_if_empty: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item_index) = self.index_for_item_id(item_id) else {
            return;
        };
        self._remove_item(
            item_index,
            activate_pane,
            close_pane_if_empty,
            None,
            window,
            cx,
        )
    }

    pub fn remove_item_and_focus_on_pane(
        &mut self,
        item_index: usize,
        activate_pane: bool,
        focus_on_pane_if_closed: Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._remove_item(
            item_index,
            activate_pane,
            true,
            Some(focus_on_pane_if_closed),
            window,
            cx,
        )
    }

    fn _remove_item(
        &mut self,
        _item_index: usize,
        activate_pane: bool,
        close_pane_if_empty: bool,
        focus_on_pane_if_closed: Option<Entity<Pane>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.item.take() else {
            return;
        };

        let should_activate = activate_pane || self.has_focus(window, cx);
        if should_activate {
            self.focus_handle.focus(window, cx);
        }

        cx.emit(Event::RemovedItem { item: item.clone() });
        item.deactivated(window, cx);
        if close_pane_if_empty {
            self.update_toolbar(window, cx);
            cx.emit(Event::Remove {
                focus_on_pane: focus_on_pane_if_closed,
            });
        }

        let mode = self.nav_history.mode();
        self.nav_history.set_mode(NavigationMode::ClosingItem);
        item.deactivated(window, cx);
        item.on_removed(cx);
        self.nav_history.set_mode(mode);

        if let Some(path) = item.project_path(cx) {
            let abs_path = self
                .nav_history
                .0
                .lock()
                .paths_by_item
                .get(&item.item_id())
                .and_then(|(_, abs_path)| abs_path.clone());

            self.nav_history
                .0
                .lock()
                .paths_by_item
                .insert(item.item_id(), (path, abs_path));
        } else {
            self.nav_history
                .0
                .lock()
                .paths_by_item
                .remove(&item.item_id());
        }

        if self.zoom_out_on_close && self.item.is_none() && close_pane_if_empty && self.zoomed {
            cx.emit(Event::ZoomOut);
        }

        cx.notify();
    }

    pub async fn save_item(
        project: Entity<Project>,
        pane: &WeakEntity<Pane>,
        item: &dyn ItemHandle,
        save_intent: SaveIntent,
        cx: &mut AsyncWindowContext,
    ) -> Result<bool> {
        const CONFLICT_MESSAGE: &str = "This file has changed on disk since you started editing it. Do you want to overwrite it?";

        const DELETED_MESSAGE: &str = "This file has been deleted on disk since you started editing it. Do you want to recreate it?";

        let path_style = project.read_with(cx, |project, cx| project.path_style(cx));
        if save_intent == SaveIntent::Skip {
            let is_saveable_singleton = cx.update(|_window, cx| {
                item.can_save(cx) && item.buffer_kind(cx) == ItemBufferKind::Singleton
            })?;
            if is_saveable_singleton {
                pane.update_in(cx, |_, window, cx| item.reload(project, window, cx))?
                    .await
                    .log_err();
            }
            return Ok(true);
        };
        let Some(item_ix) = pane
            .read_with(cx, |pane, _| pane.index_for_item(item))
            .ok()
            .flatten()
        else {
            return Ok(true);
        };

        let (
            mut has_conflict,
            mut is_dirty,
            mut can_save,
            can_save_as,
            is_singleton,
            has_deleted_file,
        ) = cx.update(|_window, cx| {
            (
                item.has_conflict(cx),
                item.is_dirty(cx),
                item.can_save(cx),
                item.can_save_as(cx),
                item.buffer_kind(cx) == ItemBufferKind::Singleton,
                item.has_deleted_file(cx),
            )
        })?;

        // when saving a single buffer, we ignore whether or not it's dirty.
        if save_intent == SaveIntent::Save || save_intent == SaveIntent::SaveWithoutFormat {
            is_dirty = true;
        }

        if save_intent == SaveIntent::SaveAs {
            is_dirty = true;
            has_conflict = false;
            can_save = false;
        }

        if save_intent == SaveIntent::Overwrite {
            has_conflict = false;
        }

        let should_format = save_intent != SaveIntent::SaveWithoutFormat;

        if has_conflict && can_save {
            if has_deleted_file && is_singleton {
                let answer = pane.update_in(cx, |pane, window, cx| {
                    pane.activate_item(item_ix, true, true, window, cx);
                    window.prompt(
                        PromptLevel::Warning,
                        DELETED_MESSAGE,
                        None,
                        &["Save", "Close", "Cancel"],
                        cx,
                    )
                })?;
                match answer.await {
                    Ok(0) => {
                        pane.update_in(cx, |_, window, cx| {
                            item.save(
                                SaveOptions {
                                    format: should_format,
                                    autosave: false,
                                },
                                project,
                                window,
                                cx,
                            )
                        })?
                        .await?
                    }
                    Ok(1) => {
                        pane.update_in(cx, |pane, window, cx| {
                            pane.remove_item(item.item_id(), false, true, window, cx)
                        })?;
                    }
                    _ => return Ok(false),
                }
                return Ok(true);
            } else {
                let answer = pane.update_in(cx, |pane, window, cx| {
                    pane.activate_item(item_ix, true, true, window, cx);
                    window.prompt(
                        PromptLevel::Warning,
                        CONFLICT_MESSAGE,
                        None,
                        &["Overwrite", "Discard", "Cancel"],
                        cx,
                    )
                })?;
                match answer.await {
                    Ok(0) => {
                        pane.update_in(cx, |_, window, cx| {
                            item.save(
                                SaveOptions {
                                    format: should_format,
                                    autosave: false,
                                },
                                project,
                                window,
                                cx,
                            )
                        })?
                        .await?
                    }
                    Ok(1) => {
                        pane.update_in(cx, |_, window, cx| item.reload(project, window, cx))?
                            .await?
                    }
                    _ => return Ok(false),
                }
            }
        } else if is_dirty && (can_save || can_save_as) {
            if save_intent == SaveIntent::Close {
                let will_autosave = cx.update(|_window, cx| {
                    item.can_autosave(cx)
                        && item.workspace_settings(cx).autosave.should_save_on_close()
                })?;
                if !will_autosave {
                    let item_id = item.item_id();
                    let answer_task = pane.update_in(cx, |pane, window, cx| {
                        if pane.save_modals_spawned.insert(item_id) {
                            pane.activate_item(item_ix, true, true, window, cx);
                            let prompt = dirty_message_for(item.project_path(cx), path_style);
                            Some(window.prompt(
                                PromptLevel::Warning,
                                &prompt,
                                None,
                                &["Save", "Don't Save", "Cancel"],
                                cx,
                            ))
                        } else {
                            None
                        }
                    })?;
                    if let Some(answer_task) = answer_task {
                        let answer = answer_task.await;
                        pane.update(cx, |pane, _| {
                            if !pane.save_modals_spawned.remove(&item_id) {
                                debug_panic!(
                                    "save modal was not present in spawned modals after awaiting for its answer"
                                )
                            }
                        })?;
                        match answer {
                            Ok(0) => {}
                            Ok(1) => {
                                // Don't save this file - reload from disk to discard changes
                                // Single-item pane has no pinned tabs
                                if can_save && is_singleton {
                                    pane.update_in(cx, |_, window, cx| {
                                        item.reload(project.clone(), window, cx)
                                    })?
                                    .await
                                    .log_err();
                                }
                                return Ok(true);
                            }
                            _ => return Ok(false), // Cancel
                        }
                    } else {
                        return Ok(false);
                    }
                }
            }

            if can_save {
                pane.update_in(cx, |pane, window, cx| {
                    pane.unpreview_item_if_preview(item.item_id());
                    item.save(
                        SaveOptions {
                            format: should_format,
                            autosave: false,
                        },
                        project,
                        window,
                        cx,
                    )
                })?
                .await?;
            } else if can_save_as && is_singleton {
                let suggested_name =
                    cx.update(|_window, cx| item.suggested_filename(cx).to_string())?;
                let new_path = pane.update_in(cx, |pane, window, cx| {
                    pane.activate_item(item_ix, true, true, window, cx);
                    pane.workspace.update(cx, |workspace, cx| {
                        let lister = if workspace.project().read(cx).is_local() {
                            DirectoryLister::Local(
                                workspace.project().clone(),
                                workspace.app_state().fs.clone(),
                            )
                        } else {
                            DirectoryLister::Project(workspace.project().clone())
                        };
                        workspace.prompt_for_new_path(lister, Some(suggested_name), window, cx)
                    })
                })??;
                let Some(new_path) = new_path.await.ok().flatten().into_iter().flatten().next()
                else {
                    return Ok(false);
                };

                let project_path = pane
                    .update(cx, |pane, cx| {
                        pane.project
                            .update(cx, |project, cx| {
                                project.find_or_create_worktree(new_path, true, cx)
                            })
                            .ok()
                    })
                    .ok()
                    .flatten();
                let save_task = if let Some(project_path) = project_path {
                    let (worktree, path) = project_path.await?;
                    let worktree_id = worktree.read_with(cx, |worktree, _| worktree.id());
                    let new_path = ProjectPath { worktree_id, path };

                    pane.update_in(cx, |pane, window, cx| {
                        if let Some(item) = pane.item_for_path(new_path.clone(), cx) {
                            pane.remove_item(item.item_id(), false, false, window, cx);
                        }

                        item.save_as(project, new_path, window, cx)
                    })?
                } else {
                    return Ok(false);
                };

                save_task.await?;
                return Ok(true);
            }
        }

        pane.update(cx, |_, cx| {
            cx.emit(Event::UserSavedItem {
                item: item.downgrade_item(),
                save_intent,
            });
            true
        })
    }

    pub fn autosave_item(
        item: &dyn ItemHandle,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let format = !matches!(
            item.workspace_settings(cx).autosave,
            AutosaveSetting::AfterDelay { .. }
        );
        if item.can_autosave(cx) {
            item.save(
                SaveOptions {
                    format,
                    autosave: true,
                },
                project,
                window,
                cx,
            )
        } else {
            Task::ready(Ok(()))
        }
    }
}

fn dirty_message_for(buffer_path: Option<ProjectPath>, path_style: PathStyle) -> String {
    let path = buffer_path
        .as_ref()
        .and_then(|p| {
            let path = p.path.display(path_style);
            if path.is_empty() { None } else { Some(path) }
        })
        .unwrap_or("This buffer".into());
    let path = truncate_and_remove_front(&path, 80);
    format!("{path} contains unsaved edits. Do you want to save it?")
}
