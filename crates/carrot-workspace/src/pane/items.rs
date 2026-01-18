use super::*;

impl Pane {
    pub fn track_alternate_file_items(&mut self) {
        if let Some(item) = self.active_item().map(|item| item.downgrade_item()) {
            let (current, _) = &self.alternate_file_items;
            match current {
                Some(current) => {
                    if current.id() != item.id() {
                        self.alternate_file_items =
                            (Some(item), self.alternate_file_items.0.take());
                    }
                }
                None => {
                    self.alternate_file_items = (Some(item), None);
                }
            }
        }
    }

    pub fn active_item_index(&self) -> usize {
        0
    }

    pub fn activation_history(&self) -> &[ActivationHistoryEntry] {
        // Single-item pane has no activation history
        &[]
    }

    pub(crate) fn open_item(
        &mut self,
        project_entry_id: Option<ProjectEntryId>,
        project_path: ProjectPath,
        focus_item: bool,
        allow_preview: bool,
        activate: bool,
        suggested_position: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
        build_item: WorkspaceItemBuilder,
    ) -> Box<dyn ItemHandle> {
        let mut existing_item = None;
        if let Some(project_entry_id) = project_entry_id {
            if let Some(item) = &self.item {
                if item.buffer_kind(cx) == ItemBufferKind::Singleton
                    && item.project_entry_ids(cx).as_slice() == [project_entry_id]
                {
                    existing_item = Some((0, item.boxed_clone()));
                }
            }
        } else if let Some(item) = &self.item {
            if item.buffer_kind(cx) == ItemBufferKind::Singleton
                && item.project_path(cx).as_ref() == Some(&project_path)
            {
                existing_item = Some((0, item.boxed_clone()));
            }
        }

        let set_up_existing_item =
            |index: usize, pane: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                if !allow_preview && let Some(item) = &pane.item {
                    pane.unpreview_item_if_preview(item.item_id());
                }
                if activate {
                    pane.activate_item(index, focus_item, focus_item, window, cx);
                }
            };
        let set_up_new_item = |new_item: Box<dyn ItemHandle>,
                               destination_index: Option<usize>,
                               pane: &mut Self,
                               window: &mut Window,
                               cx: &mut Context<Self>| {
            if allow_preview {
                pane.replace_preview_item_id(new_item.item_id(), window, cx);
            }

            if let Some(text) = new_item.telemetry_event_text(cx) {
                carrot_telemetry::event!(text);
            }

            pane.add_item_inner(
                new_item,
                true,
                focus_item,
                activate,
                destination_index,
                window,
                cx,
            );
        };

        if let Some((index, existing_item)) = existing_item {
            set_up_existing_item(index, self, window, cx);
            existing_item
        } else {
            // If the item is being opened as preview and we have an existing preview tab,
            // open the new item in the position of the existing preview tab.
            let destination_index = if allow_preview {
                self.close_current_preview_item(window, cx)
            } else {
                suggested_position
            };

            let new_item = build_item(self, window, cx);
            // A special case that won't ever get a `project_entry_id` but has to be deduplicated nonetheless.
            if let Some(invalid_buffer_view) = new_item.downcast::<InvalidItemView>() {
                let mut already_open_view = None;
                let mut views_to_close = HashSet::default();
                for existing_error_view in self
                    .items_of_type::<InvalidItemView>()
                    .filter(|item| item.read(cx).abs_path == invalid_buffer_view.read(cx).abs_path)
                {
                    if already_open_view.is_none()
                        && existing_error_view.read(cx).error == invalid_buffer_view.read(cx).error
                    {
                        already_open_view = Some(existing_error_view);
                    } else {
                        views_to_close.insert(existing_error_view.item_id());
                    }
                }

                let resulting_item = match already_open_view {
                    Some(already_open_view) => {
                        if let Some(index) = self.index_for_item_id(already_open_view.item_id()) {
                            set_up_existing_item(index, self, window, cx);
                        }
                        Box::new(already_open_view) as Box<_>
                    }
                    None => {
                        set_up_new_item(new_item.clone(), destination_index, self, window, cx);
                        new_item
                    }
                };

                self.close_items(window, cx, SaveIntent::Skip, &|existing_item| {
                    views_to_close.contains(&existing_item)
                })
                .detach();

                resulting_item
            } else {
                set_up_new_item(new_item.clone(), destination_index, self, window, cx);
                new_item
            }
        }
    }

    pub fn add_item_inner(
        &mut self,
        item: Box<dyn ItemHandle>,
        activate_pane: bool,
        focus_item: bool,
        activate: bool,
        _destination_index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if item.buffer_kind(cx) == ItemBufferKind::Singleton
            && let Some(&entry_id) = item.project_entry_ids(cx).first()
        {
            let Some(project) = self.project.upgrade() else {
                return;
            };

            let project = project.read(cx);
            if let Some(project_path) = project.path_for_entry(entry_id, cx) {
                let abs_path = project.absolute_path(&project_path, cx);
                self.nav_history
                    .0
                    .lock()
                    .paths_by_item
                    .insert(item.item_id(), (project_path, abs_path));
            }
        }

        // Single-item pane: replace the current item
        self.item = Some(item.clone());
        cx.notify();

        if activate {
            self.activate_item(0, activate_pane, focus_item, window, cx);
        }

        cx.emit(Event::AddItem { item });
    }

    pub fn add_item(
        &mut self,
        item: Box<dyn ItemHandle>,
        activate_pane: bool,
        focus_item: bool,
        destination_index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = item.telemetry_event_text(cx) {
            carrot_telemetry::event!(text);
        }

        self.add_item_inner(
            item,
            activate_pane,
            focus_item,
            true,
            destination_index,
            window,
            cx,
        )
    }

    pub fn items_len(&self) -> usize {
        if self.item.is_some() { 1 } else { 0 }
    }

    pub fn items(&self) -> impl DoubleEndedIterator<Item = &Box<dyn ItemHandle>> {
        self.item.iter()
    }

    pub fn items_of_type<T: Render>(&self) -> impl '_ + Iterator<Item = Entity<T>> {
        self.item
            .iter()
            .filter_map(|item| item.to_any_view().downcast().ok())
    }

    pub fn active_item(&self) -> Option<Box<dyn ItemHandle>> {
        self.item.clone()
    }

    pub(super) fn active_item_id(&self) -> EntityId {
        self.item.as_ref().expect("pane has no item").item_id()
    }

    pub fn pixel_position_of_cursor(&self, cx: &App) -> Option<Point<Pixels>> {
        self.item.as_ref()?.pixel_position_of_cursor(cx)
    }

    pub fn item_for_entry(
        &self,
        entry_id: ProjectEntryId,
        cx: &App,
    ) -> Option<Box<dyn ItemHandle>> {
        self.item.iter().find_map(|item| {
            if item.buffer_kind(cx) == ItemBufferKind::Singleton
                && (item.project_entry_ids(cx).as_slice() == [entry_id])
            {
                Some(item.boxed_clone())
            } else {
                None
            }
        })
    }

    pub fn item_for_path(
        &self,
        project_path: ProjectPath,
        cx: &App,
    ) -> Option<Box<dyn ItemHandle>> {
        self.item.iter().find_map(move |item| {
            if item.buffer_kind(cx) == ItemBufferKind::Singleton
                && (item.project_path(cx).as_slice() == [project_path.clone()])
            {
                Some(item.boxed_clone())
            } else {
                None
            }
        })
    }

    pub fn activate_item(
        &mut self,
        _index: usize,
        activate_pane: bool,
        focus_item: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.item.is_some() {
            self.update_toolbar(window, cx);
            self.update_status_bar(window, cx);

            if focus_item {
                self.focus_active_item(window, cx);
            }

            cx.emit(Event::ActivateItem {
                local: activate_pane,
                focus_changed: focus_item,
            });

            cx.notify();
        }
    }

    pub(super) fn update_active_tab(&mut self, _index: usize) {
        // No-op: single-item pane has no tab bar
    }

    pub fn activate_previous_item(
        &mut self,
        _: &ActivatePreviousItem,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // No-op: single-item pane
    }

    pub fn activate_next_item(
        &mut self,
        _: &ActivateNextItem,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // No-op: single-item pane
    }

    pub fn swap_item_left(
        &mut self,
        _: &SwapItemLeft,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // No-op: single-item pane
    }

    pub fn swap_item_right(
        &mut self,
        _: &SwapItemRight,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // No-op: single-item pane
    }

    pub fn activate_last_item(
        &mut self,
        _: &ActivateLastItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_item(0, true, true, window, cx);
    }

    pub fn focus_active_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(active_item) = self.active_item() {
            let focus_handle = active_item.item_focus_handle(cx);
            window.focus(&focus_handle, cx);
        }
    }
}
