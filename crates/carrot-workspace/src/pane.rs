use crate::{
    CloseWindow, OpenOptions, OpenVisible, SplitDirection, ToggleZoom, Workspace,
    WorkspaceItemBuilder, ZoomIn, ZoomOut,
    invalid_item_view::InvalidItemView,
    item::{
        Item, ItemBufferKind, ItemHandle, ItemSettings, PreviewTabsSettings, ProjectItemKind,
        SaveOptions, ShowDiagnostics, TabContentParams, WeakItemHandle,
    },
    move_item,
    notifications::NotifyResultExt,
    toolbar::Toolbar,
    workspace_settings::{AutosaveSetting, TabBarSettings, WorkspaceSettings},
};
use anyhow::Result;
use carrot_language::DiagnosticSeverity;
use carrot_project::{DirectoryLister, Project, ProjectEntryId, ProjectPath};
use carrot_theme_settings::ThemeSettings;
use carrot_ui::{ContextMenu, Indicator, PopoverMenuHandle, Tab, prelude::*};
use futures::{StreamExt, stream::FuturesUnordered};
use inazuma::{
    Action, AnyElement, App, AsyncWindowContext, ClickEvent, Context, Corner, Div, DragMoveEvent,
    Entity, EntityId, EventEmitter, ExternalPaths, FocusHandle, FocusOutEvent, Focusable,
    KeyContext, MouseButton, NavigationDirection, Pixels, Point, PromptLevel, Render, Subscription,
    Task, WeakEntity, WeakFocusHandle, Window, anchored, deferred,
};
use inazuma_collections::{BTreeSet, HashMap, HashSet};
use inazuma_settings_framework::{Settings, SettingsStore};
use inazuma_util::{ResultExt, debug_panic, maybe, paths::PathStyle, truncate_and_remove_front};
use itertools::Itertools;
use parking_lot::Mutex;
use std::{
    any::Any,
    sync::{Arc, atomic::AtomicUsize},
    time::Duration,
};

pub mod actions;
pub use actions::*;
mod closing;
mod drag_drop;
mod items;
pub mod nav_history;
pub use nav_history::*;
mod render;
pub use render::{render_item_indicator, tab_details};

/// A single-item container in the workspace.
/// Treats the item uniformly via the [`ItemHandle`] trait, whether it's an editor, search results multibuffer, terminal or something else,
/// responsible for managing item focus and zoom states and drag and drop features.
/// Can be split, see `PaneGroup` for more details.
pub struct Pane {
    alternate_file_items: (
        Option<Box<dyn WeakItemHandle>>,
        Option<Box<dyn WeakItemHandle>>,
    ),
    focus_handle: FocusHandle,
    item: Option<Box<dyn ItemHandle>>,
    zoomed: bool,
    was_focused: bool,
    last_focus_handle_by_item: HashMap<EntityId, WeakFocusHandle>,
    nav_history: NavHistory,
    toolbar: Entity<Toolbar>,
    pub(crate) workspace: WeakEntity<Workspace>,
    project: WeakEntity<Project>,
    pub drag_split_direction: Option<SplitDirection>,
    can_drop_predicate: Option<Arc<dyn Fn(&dyn Any, &mut Window, &mut App) -> bool>>,
    can_split_predicate:
        Option<Arc<dyn Fn(&mut Self, &dyn Any, &mut Window, &mut Context<Self>) -> bool>>,
    can_toggle_zoom: bool,
    should_display_welcome_page: bool,
    _subscriptions: Vec<Subscription>,
    /// Is None if navigation buttons are permanently turned off (and should not react to setting changes).
    /// Otherwise, when `display_nav_history_buttons` is Some, it determines whether nav buttons should be displayed.
    display_nav_history_buttons: Option<bool>,
    double_click_dispatch_action: Box<dyn Action>,
    save_modals_spawned: HashSet<EntityId>,
    close_pane_if_empty: bool,
    pub new_item_context_menu_handle: PopoverMenuHandle<ContextMenu>,
    pub split_item_context_menu_handle: PopoverMenuHandle<ContextMenu>,
    diagnostics: HashMap<ProjectPath, DiagnosticSeverity>,
    zoom_out_on_close: bool,
    diagnostic_summary_update: Task<()>,
    /// If a certain project item wants to get recreated with specific data, it can persist its data before the recreation here.
    pub project_item_restoration_data: HashMap<ProjectItemKind, Box<dyn Any + Send>>,
    welcome_page: Option<Entity<crate::welcome::WelcomePage>>,

    pub in_center_group: bool,
}

#[derive(Clone)]
pub struct DraggedTab {
    pub pane: Entity<Pane>,
    pub item: Box<dyn ItemHandle>,
    pub ix: usize,
    pub detail: usize,
    pub is_active: bool,
}

pub enum Side {
    Left,
    Right,
}

#[derive(Copy, Clone)]
pub struct ActivationHistoryEntry {
    pub entity_id: EntityId,
    pub timestamp: usize,
}

impl Pane {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        next_timestamp: Arc<AtomicUsize>,
        can_drop_predicate: Option<Arc<dyn Fn(&dyn Any, &mut Window, &mut App) -> bool + 'static>>,
        double_click_dispatch_action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        let subscriptions = vec![
            cx.on_focus(&focus_handle, window, Pane::focus_in),
            cx.on_focus_in(&focus_handle, window, Pane::focus_in),
            cx.on_focus_out(&focus_handle, window, Pane::focus_out),
            cx.observe_global_in::<SettingsStore>(window, Self::settings_changed),
            cx.subscribe(&project, Self::project_events),
        ];

        let handle = cx.entity().downgrade();

        Self {
            alternate_file_items: (None, None),
            focus_handle,
            item: None,
            was_focused: false,
            zoomed: false,
            last_focus_handle_by_item: Default::default(),
            nav_history: NavHistory(Arc::new(Mutex::new(NavHistoryState {
                mode: NavigationMode::Normal,
                backward_stack: Default::default(),
                forward_stack: Default::default(),
                closed_stack: Default::default(),
                tag_stack: Default::default(),
                tag_stack_pos: Default::default(),
                paths_by_item: Default::default(),
                pane: handle,
                next_timestamp,
                preview_item_id: None,
            }))),
            toolbar: cx.new(|_| Toolbar::new()),
            drag_split_direction: None,
            workspace,
            project: project.downgrade(),
            can_drop_predicate,
            can_split_predicate: None,
            can_toggle_zoom: true,
            should_display_welcome_page: false,
            display_nav_history_buttons: Some(
                TabBarSettings::get_global(cx).show_nav_history_buttons,
            ),
            _subscriptions: subscriptions,
            double_click_dispatch_action,
            save_modals_spawned: HashSet::default(),
            close_pane_if_empty: true,
            split_item_context_menu_handle: Default::default(),
            new_item_context_menu_handle: Default::default(),
            diagnostics: Default::default(),
            zoom_out_on_close: true,
            diagnostic_summary_update: Task::ready(()),
            project_item_restoration_data: HashMap::default(),
            welcome_page: None,
            in_center_group: false,
        }
    }

    fn alternate_file(&mut self, _: &AlternateFile, window: &mut Window, cx: &mut Context<Pane>) {
        let (_, alternative) = &self.alternate_file_items;
        if let Some(alternative) = alternative {
            let existing = self
                .items()
                .find_position(|item| item.item_id() == alternative.id());
            if let Some((ix, _)) = existing {
                self.activate_item(ix, true, true, window, cx);
            } else if let Some(upgraded) = alternative.upgrade() {
                self.add_item(upgraded, true, true, None, window, cx);
            }
        }
    }

    pub fn has_focus(&self, window: &Window, cx: &App) -> bool {
        // We not only check whether our focus handle contains focus, but also
        // whether the active item might have focus, because we might have just activated an item
        // that hasn't rendered yet.
        // Before the next render, we might transfer focus
        // to the item, and `focus_handle.contains_focus` returns false because the `active_item`
        // is not hooked up to us in the dispatch tree.
        self.focus_handle.contains_focused(window, cx)
            || self
                .active_item()
                .is_some_and(|item| item.item_focus_handle(cx).contains_focused(window, cx))
    }

    fn focus_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.was_focused {
            self.was_focused = true;
            self.update_history(0);
            if self.item.is_some() {
                self.update_active_tab(0);
            }
            cx.emit(Event::Focus);
            cx.notify();
        }

        self.toolbar.update(cx, |toolbar, cx| {
            toolbar.focus_changed(true, window, cx);
        });

        if let Some(active_item) = self.active_item() {
            if self.focus_handle.is_focused(window) {
                // Schedule a redraw next frame, so that the focus changes below take effect
                cx.on_next_frame(window, |_, _, cx| {
                    cx.notify();
                });

                // Pane was focused directly. We need to either focus a view inside the active item,
                // or focus the active item itself
                if let Some(weak_last_focus_handle) =
                    self.last_focus_handle_by_item.get(&active_item.item_id())
                    && let Some(focus_handle) = weak_last_focus_handle.upgrade()
                {
                    focus_handle.focus(window, cx);
                    return;
                }

                active_item.item_focus_handle(cx).focus(window, cx);
            } else if let Some(focused) = window.focused(cx)
                && !self.context_menu_focused(window, cx)
            {
                self.last_focus_handle_by_item
                    .insert(active_item.item_id(), focused.downgrade());
            }
        } else if self.should_display_welcome_page
            && let Some(welcome_page) = self.welcome_page.as_ref()
        {
            if self.focus_handle.is_focused(window) {
                welcome_page.read(cx).focus_handle(cx).focus(window, cx);
            }
        }
    }

    pub fn context_menu_focused(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.new_item_context_menu_handle.is_focused(window, cx)
            || self.split_item_context_menu_handle.is_focused(window, cx)
    }

    fn focus_out(&mut self, _event: FocusOutEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.was_focused = false;
        self.toolbar.update(cx, |toolbar, cx| {
            toolbar.focus_changed(false, window, cx);
        });

        cx.notify();
    }

    fn project_events(
        &mut self,
        _project: Entity<Project>,
        event: &carrot_project::Event,
        cx: &mut Context<Self>,
    ) {
        match event {
            carrot_project::Event::DiskBasedDiagnosticsFinished { .. }
            | carrot_project::Event::DiagnosticsUpdated { .. } => {
                if ItemSettings::get_global(cx).show_diagnostics != ShowDiagnostics::Off {
                    self.diagnostic_summary_update = cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(30))
                            .await;
                        this.update(cx, |this, cx| {
                            this.update_diagnostics(cx);
                            cx.notify();
                        })
                        .log_err();
                    });
                }
            }
            _ => {}
        }
    }

    fn update_diagnostics(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project.upgrade() else {
            return;
        };
        let show_diagnostics = ItemSettings::get_global(cx).show_diagnostics;
        self.diagnostics = if show_diagnostics != ShowDiagnostics::Off {
            project
                .read(cx)
                .diagnostic_summaries(false, cx)
                .filter_map(|(project_path, _, diagnostic_summary)| {
                    if diagnostic_summary.error_count > 0 {
                        Some((project_path, DiagnosticSeverity::ERROR))
                    } else if diagnostic_summary.warning_count > 0
                        && show_diagnostics != ShowDiagnostics::Errors
                    {
                        Some((project_path, DiagnosticSeverity::WARNING))
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            HashMap::default()
        }
    }

    fn settings_changed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let tab_bar_settings = TabBarSettings::get_global(cx);

        if let Some(display_nav_history_buttons) = self.display_nav_history_buttons.as_mut() {
            *display_nav_history_buttons = tab_bar_settings.show_nav_history_buttons;
        }

        if !PreviewTabsSettings::get_global(cx).enabled {
            self.nav_history.0.lock().preview_item_id = None;
        }

        self.update_diagnostics(cx);
        cx.notify();
    }

    pub fn set_should_display_tab_bar<F>(&mut self, _should_display_tab_bar: F)
    where
        F: 'static + Fn(&Window, &mut Context<Pane>) -> bool,
    {
        // No-op: single-item pane has no tab bar
    }

    pub fn set_should_display_welcome_page(&mut self, should_display_welcome_page: bool) {
        self.should_display_welcome_page = should_display_welcome_page;
    }

    pub fn set_can_split(
        &mut self,
        can_split_predicate: Option<
            Arc<dyn Fn(&mut Self, &dyn Any, &mut Window, &mut Context<Self>) -> bool + 'static>,
        >,
    ) {
        self.can_split_predicate = can_split_predicate;
    }

    pub fn set_can_toggle_zoom(&mut self, can_toggle_zoom: bool, cx: &mut Context<Self>) {
        self.can_toggle_zoom = can_toggle_zoom;
        cx.notify();
    }

    pub fn set_close_pane_if_empty(&mut self, close_pane_if_empty: bool, cx: &mut Context<Self>) {
        self.close_pane_if_empty = close_pane_if_empty;
        cx.notify();
    }

    pub fn set_can_navigate(&mut self, can_navigate: bool, cx: &mut Context<Self>) {
        self.toolbar.update(cx, |toolbar, cx| {
            toolbar.set_can_navigate(can_navigate, cx);
        });
        cx.notify();
    }

    pub fn set_render_tab_bar<F>(&mut self, cx: &mut Context<Self>, _render: F)
    where
        F: 'static + Fn(&mut Pane, &mut Window, &mut Context<Pane>) -> AnyElement,
    {
        // No-op: single-item pane has no tab bar
        cx.notify();
    }

    pub fn set_render_tab_bar_buttons<F>(&mut self, cx: &mut Context<Self>, _render: F)
    where
        F: 'static
            + Fn(
                &mut Pane,
                &mut Window,
                &mut Context<Pane>,
            ) -> (Option<AnyElement>, Option<AnyElement>),
    {
        // No-op: single-item pane has no tab bar
        cx.notify();
    }

    pub fn nav_history_for_item<T: Item>(&self, item: &Entity<T>) -> ItemNavHistory {
        ItemNavHistory {
            history: self.nav_history.clone(),
            item: Arc::new(item.downgrade()),
        }
    }

    pub fn nav_history(&self) -> &NavHistory {
        &self.nav_history
    }

    pub fn nav_history_mut(&mut self) -> &mut NavHistory {
        &mut self.nav_history
    }

    pub fn fork_nav_history(&self) -> NavHistory {
        let history = self.nav_history.0.lock().clone();
        NavHistory(Arc::new(Mutex::new(history)))
    }

    pub fn set_nav_history(&mut self, history: NavHistory, cx: &Context<Self>) {
        self.nav_history = history;
        self.nav_history().0.lock().pane = cx.entity().downgrade();
    }

    pub fn disable_history(&mut self) {
        self.nav_history.disable();
    }

    pub fn enable_history(&mut self) {
        self.nav_history.enable();
    }

    pub fn can_navigate_backward(&self) -> bool {
        !self.nav_history.0.lock().backward_stack.is_empty()
    }

    pub fn can_navigate_forward(&self) -> bool {
        !self.nav_history.0.lock().forward_stack.is_empty()
    }

    pub fn navigate_backward(&mut self, _: &GoBack, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            let pane = cx.entity().downgrade();
            window.defer(cx, move |window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.go_back(pane, window, cx).detach_and_log_err(cx)
                })
            })
        }
    }

    fn navigate_forward(&mut self, _: &GoForward, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            let pane = cx.entity().downgrade();
            window.defer(cx, move |window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace
                        .go_forward(pane, window, cx)
                        .detach_and_log_err(cx)
                })
            })
        }
    }

    pub fn go_to_older_tag(
        &mut self,
        _: &GoToOlderTag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(workspace) = self.workspace.upgrade() {
            let pane = cx.entity().downgrade();
            window.defer(cx, move |window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace
                        .navigate_tag_history(pane, TagNavigationMode::Older, window, cx)
                        .detach_and_log_err(cx)
                })
            })
        }
    }

    pub fn go_to_newer_tag(
        &mut self,
        _: &GoToNewerTag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(workspace) = self.workspace.upgrade() {
            let pane = cx.entity().downgrade();
            window.defer(cx, move |window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace
                        .navigate_tag_history(pane, TagNavigationMode::Newer, window, cx)
                        .detach_and_log_err(cx)
                })
            })
        }
    }

    fn history_updated(&mut self, cx: &mut Context<Self>) {
        self.toolbar.update(cx, |_, cx| cx.notify());
    }

    pub fn preview_item_id(&self) -> Option<EntityId> {
        None
    }

    pub fn preview_item(&self) -> Option<Box<dyn ItemHandle>> {
        None
    }

    pub fn preview_item_idx(&self) -> Option<usize> {
        None
    }

    pub fn is_active_preview_item(&self, _item_id: EntityId) -> bool {
        false
    }

    /// No-op: single-item pane has no preview items.
    pub fn unpreview_item_if_preview(&mut self, _item_id: EntityId) {}

    /// No-op: single-item pane has no preview items.
    pub fn replace_preview_item_id(
        &mut self,
        _item_id: EntityId,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    /// No-op: single-item pane has no preview items.
    pub(crate) fn set_preview_item_id(&mut self, _item_id: Option<EntityId>, _cx: &App) {}

    pub fn handle_item_edit(&mut self, _item_id: EntityId, _cx: &App) {
        // No-op: single-item pane has no preview items
    }

    pub fn close_current_preview_item(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        // No-op: single-item pane has no preview items
        None
    }

    pub fn index_for_item(&self, item: &dyn ItemHandle) -> Option<usize> {
        self.index_for_item_id(item.item_id())
    }

    fn index_for_item_id(&self, item_id: EntityId) -> Option<usize> {
        match &self.item {
            Some(item) if item.item_id() == item_id => Some(0),
            _ => None,
        }
    }

    pub fn item_for_index(&self, ix: usize) -> Option<&dyn ItemHandle> {
        if ix == 0 {
            self.item.as_ref().map(|i| i.as_ref())
        } else {
            None
        }
    }

    pub fn toggle_zoom(&mut self, _: &ToggleZoom, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_toggle_zoom {
            cx.propagate();
        } else if self.zoomed {
            cx.emit(Event::ZoomOut);
        } else if self.item.is_some() {
            if !self.focus_handle.contains_focused(window, cx) {
                cx.focus_self(window);
            }
            cx.emit(Event::ZoomIn);
        }
    }

    pub fn zoom_in(&mut self, _: &ZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_toggle_zoom {
            cx.propagate();
        } else if !self.zoomed && self.item.is_some() {
            if !self.focus_handle.contains_focused(window, cx) {
                cx.focus_self(window);
            }
            cx.emit(Event::ZoomIn);
        }
    }

    pub fn zoom_out(&mut self, _: &ZoomOut, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_toggle_zoom {
            cx.propagate();
        } else if self.zoomed {
            cx.emit(Event::ZoomOut);
        }
    }

    fn update_history(&mut self, _index: usize) {
        // No-op: single-item pane has no activation history
    }

    pub fn split(
        &mut self,
        direction: SplitDirection,
        mode: SplitMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.items_len() <= 1 && mode == SplitMode::MovePane {
            // MovePane with only one pane present behaves like a SplitEmpty in the opposite direction
            let active_item = self.active_item();
            cx.emit(Event::Split {
                direction: direction.opposite(),
                mode: SplitMode::EmptyPane,
            });
            // ensure that we focus the moved pane
            // in this case we know that the window is the same as the active_item
            if let Some(active_item) = active_item {
                cx.defer_in(window, move |_, window, cx| {
                    let focus_handle = active_item.item_focus_handle(cx);
                    window.focus(&focus_handle, cx);
                });
            }
        } else {
            cx.emit(Event::Split { direction, mode });
        }
    }

    pub fn toolbar(&self) -> &Entity<Toolbar> {
        &self.toolbar
    }

    pub fn handle_deleted_project_item(
        &mut self,
        entry_id: ProjectEntryId,
        window: &mut Window,
        cx: &mut Context<Pane>,
    ) -> Option<()> {
        let item_id = self.items().find_map(|item| {
            if item.buffer_kind(cx) == ItemBufferKind::Singleton
                && item.project_entry_ids(cx).as_slice() == [entry_id]
            {
                Some(item.item_id())
            } else {
                None
            }
        })?;

        self.remove_item(item_id, false, true, window, cx);
        self.nav_history.remove_item(item_id);

        Some(())
    }

    fn update_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_item = self.item.as_ref().map(|item| item.as_ref());
        self.toolbar.update(cx, |toolbar, cx| {
            toolbar.set_active_item(active_item, window, cx);
        });
    }

    fn update_status_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let pane = cx.entity();

        window.defer(cx, move |window, cx| {
            let Ok(status_bar) =
                workspace.read_with(cx, |workspace, _| workspace.status_bar.clone())
            else {
                return;
            };

            status_bar.update(cx, move |status_bar, cx| {
                status_bar.set_active_pane(&pane, window, cx);
            });
        });
    }

    pub fn icon_color(selected: bool) -> Color {
        if selected {
            Color::Default
        } else {
            Color::Muted
        }
    }

    pub fn set_zoomed(&mut self, zoomed: bool, cx: &mut Context<Self>) {
        self.zoomed = zoomed;
        cx.notify();
    }

    pub fn is_zoomed(&self) -> bool {
        self.zoomed
    }

    pub fn display_nav_history_buttons(&mut self, display: Option<bool>) {
        self.display_nav_history_buttons = display;
    }

    fn clean_item_ids(&self, cx: &mut Context<Pane>) -> Vec<EntityId> {
        self.items()
            .filter_map(|item| {
                if !item.is_dirty(cx) {
                    return Some(item.item_id());
                }

                None
            })
            .collect()
    }

    fn to_the_side_item_ids(&self, item_id: EntityId, side: Side) -> Vec<EntityId> {
        match side {
            Side::Left => self
                .items()
                .take_while(|item| item.item_id() != item_id)
                .map(|item| item.item_id())
                .collect(),
            Side::Right => self
                .items()
                .rev()
                .take_while(|item| item.item_id() != item_id)
                .map(|item| item.item_id())
                .collect(),
        }
    }

    fn multibuffer_item_ids(&self, cx: &mut Context<Pane>) -> Vec<EntityId> {
        self.items()
            .filter(|item| item.buffer_kind(cx) == ItemBufferKind::Multibuffer)
            .map(|item| item.item_id())
            .collect()
    }

    pub fn set_zoom_out_on_close(&mut self, zoom_out_on_close: bool) {
        self.zoom_out_on_close = zoom_out_on_close;
    }
}

#[cfg(test)]
mod tests;
