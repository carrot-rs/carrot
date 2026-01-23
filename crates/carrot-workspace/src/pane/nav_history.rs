use crate::item::WeakItemHandle;
use crate::pane::Pane;
use crate::pane::actions::MAX_NAVIGATION_HISTORY_LEN;
use carrot_project::ProjectPath;
use inazuma::{App, EntityId, WeakEntity};
use inazuma_collections::{HashMap, VecDeque};
use parking_lot::Mutex;
use std::{
    any::Any,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Debug, Default, Copy, Clone)]
pub enum NavigationMode {
    #[default]
    Normal,
    GoingBack,
    GoingForward,
    ClosingItem,
    ReopeningClosedItem,
    Disabled,
}

#[derive(Debug, Default, Copy, Clone)]
pub enum TagNavigationMode {
    #[default]
    Older,
    Newer,
}

#[derive(Clone)]
pub struct NavigationEntry {
    pub item: Arc<dyn WeakItemHandle + Send + Sync>,
    pub data: Option<Arc<dyn Any + Send + Sync>>,
    pub timestamp: usize,
    pub is_preview: bool,
    /// Row position for Neovim-style deduplication. When set, entries with the
    /// same item and row are considered duplicates and deduplicated.
    pub row: Option<u32>,
}

#[derive(Clone)]
pub struct TagStackEntry {
    pub origin: NavigationEntry,
    pub target: NavigationEntry,
}

#[derive(Clone)]
pub(crate) struct NavHistoryState {
    pub(crate) mode: NavigationMode,
    pub(crate) backward_stack: VecDeque<NavigationEntry>,
    pub(crate) forward_stack: VecDeque<NavigationEntry>,
    pub(crate) closed_stack: VecDeque<NavigationEntry>,
    pub(crate) tag_stack: VecDeque<TagStackEntry>,
    pub(crate) tag_stack_pos: usize,
    pub(crate) paths_by_item: HashMap<EntityId, (ProjectPath, Option<PathBuf>)>,
    pub(crate) pane: WeakEntity<Pane>,
    pub(crate) next_timestamp: Arc<AtomicUsize>,
    pub(crate) preview_item_id: Option<EntityId>,
}

#[derive(Clone)]
pub struct NavHistory(pub(crate) Arc<Mutex<NavHistoryState>>);

#[derive(Clone)]
pub struct ItemNavHistory {
    pub(crate) history: NavHistory,
    pub(crate) item: Arc<dyn WeakItemHandle>,
}

impl ItemNavHistory {
    pub fn push<D: 'static + Any + Send + Sync>(
        &mut self,
        data: Option<D>,
        row: Option<u32>,
        cx: &mut App,
    ) {
        if self
            .item
            .upgrade()
            .is_some_and(|item| item.include_in_nav_history())
        {
            let is_preview_item = self.history.0.lock().preview_item_id == Some(self.item.id());
            self.history
                .push(data, self.item.clone(), is_preview_item, row, cx);
        }
    }

    pub fn navigation_entry(&self, data: Option<Arc<dyn Any + Send + Sync>>) -> NavigationEntry {
        let is_preview_item = self.history.0.lock().preview_item_id == Some(self.item.id());
        NavigationEntry {
            item: self.item.clone(),
            data,
            timestamp: 0,
            is_preview: is_preview_item,
            row: None,
        }
    }

    pub fn push_tag(&mut self, origin: Option<NavigationEntry>, target: Option<NavigationEntry>) {
        if let (Some(origin_entry), Some(target_entry)) = (origin, target) {
            self.history.push_tag(origin_entry, target_entry);
        }
    }

    pub fn pop_backward(&mut self, cx: &mut App) -> Option<NavigationEntry> {
        self.history.pop(NavigationMode::GoingBack, cx)
    }

    pub fn pop_forward(&mut self, cx: &mut App) -> Option<NavigationEntry> {
        self.history.pop(NavigationMode::GoingForward, cx)
    }
}

impl NavHistory {
    pub fn for_each_entry(
        &self,
        cx: &App,
        f: &mut dyn FnMut(&NavigationEntry, (ProjectPath, Option<PathBuf>)),
    ) {
        let borrowed_history = self.0.lock();
        borrowed_history
            .forward_stack
            .iter()
            .chain(borrowed_history.backward_stack.iter())
            .chain(borrowed_history.closed_stack.iter())
            .for_each(|entry| {
                if let Some(project_and_abs_path) =
                    borrowed_history.paths_by_item.get(&entry.item.id())
                {
                    f(entry, project_and_abs_path.clone());
                } else if let Some(item) = entry.item.upgrade()
                    && let Some(path) = item.project_path(cx)
                {
                    f(entry, (path, None));
                }
            })
    }

    pub fn set_mode(&mut self, mode: NavigationMode) {
        self.0.lock().mode = mode;
    }

    pub fn mode(&self) -> NavigationMode {
        self.0.lock().mode
    }

    pub fn disable(&mut self) {
        self.0.lock().mode = NavigationMode::Disabled;
    }

    pub fn enable(&mut self) {
        self.0.lock().mode = NavigationMode::Normal;
    }

    pub fn clear(&mut self, cx: &mut App) {
        let mut state = self.0.lock();

        if state.backward_stack.is_empty()
            && state.forward_stack.is_empty()
            && state.closed_stack.is_empty()
            && state.paths_by_item.is_empty()
            && state.tag_stack.is_empty()
        {
            return;
        }

        state.mode = NavigationMode::Normal;
        state.backward_stack.clear();
        state.forward_stack.clear();
        state.closed_stack.clear();
        state.paths_by_item.clear();
        state.tag_stack.clear();
        state.tag_stack_pos = 0;
        state.did_update(cx);
    }

    pub fn pop(&mut self, mode: NavigationMode, cx: &mut App) -> Option<NavigationEntry> {
        let mut state = self.0.lock();
        let entry = match mode {
            NavigationMode::Normal | NavigationMode::Disabled | NavigationMode::ClosingItem => {
                return None;
            }
            NavigationMode::GoingBack => &mut state.backward_stack,
            NavigationMode::GoingForward => &mut state.forward_stack,
            NavigationMode::ReopeningClosedItem => &mut state.closed_stack,
        }
        .pop_back();
        if entry.is_some() {
            state.did_update(cx);
        }
        entry
    }

    pub fn push<D: 'static + Any + Send + Sync>(
        &mut self,
        data: Option<D>,
        item: Arc<dyn WeakItemHandle + Send + Sync>,
        is_preview: bool,
        row: Option<u32>,
        cx: &mut App,
    ) {
        let state = &mut *self.0.lock();
        let new_item_id = item.id();

        let is_same_location =
            |entry: &NavigationEntry| entry.item.id() == new_item_id && entry.row == row;

        match state.mode {
            NavigationMode::Disabled => {}
            NavigationMode::Normal | NavigationMode::ReopeningClosedItem => {
                state
                    .backward_stack
                    .retain(|entry| !is_same_location(entry));

                if state.backward_stack.len() >= MAX_NAVIGATION_HISTORY_LEN {
                    state.backward_stack.pop_front();
                }
                state.backward_stack.push_back(NavigationEntry {
                    item,
                    data: data.map(|data| Arc::new(data) as Arc<dyn Any + Send + Sync>),
                    timestamp: state.next_timestamp.fetch_add(1, Ordering::SeqCst),
                    is_preview,
                    row,
                });
                state.forward_stack.clear();
            }
            NavigationMode::GoingBack => {
                state.forward_stack.retain(|entry| !is_same_location(entry));

                if state.forward_stack.len() >= MAX_NAVIGATION_HISTORY_LEN {
                    state.forward_stack.pop_front();
                }
                state.forward_stack.push_back(NavigationEntry {
                    item,
                    data: data.map(|data| Arc::new(data) as Arc<dyn Any + Send + Sync>),
                    timestamp: state.next_timestamp.fetch_add(1, Ordering::SeqCst),
                    is_preview,
                    row,
                });
            }
            NavigationMode::GoingForward => {
                state
                    .backward_stack
                    .retain(|entry| !is_same_location(entry));

                if state.backward_stack.len() >= MAX_NAVIGATION_HISTORY_LEN {
                    state.backward_stack.pop_front();
                }
                state.backward_stack.push_back(NavigationEntry {
                    item,
                    data: data.map(|data| Arc::new(data) as Arc<dyn Any + Send + Sync>),
                    timestamp: state.next_timestamp.fetch_add(1, Ordering::SeqCst),
                    is_preview,
                    row,
                });
            }
            NavigationMode::ClosingItem if is_preview => return,
            NavigationMode::ClosingItem => {
                if state.closed_stack.len() >= MAX_NAVIGATION_HISTORY_LEN {
                    state.closed_stack.pop_front();
                }
                state.closed_stack.push_back(NavigationEntry {
                    item,
                    data: data.map(|data| Arc::new(data) as Arc<dyn Any + Send + Sync>),
                    timestamp: state.next_timestamp.fetch_add(1, Ordering::SeqCst),
                    is_preview,
                    row,
                });
            }
        }
        state.did_update(cx);
    }

    pub fn remove_item(&mut self, item_id: EntityId) {
        let mut state = self.0.lock();
        state.paths_by_item.remove(&item_id);
        state
            .backward_stack
            .retain(|entry| entry.item.id() != item_id);
        state
            .forward_stack
            .retain(|entry| entry.item.id() != item_id);
        state
            .closed_stack
            .retain(|entry| entry.item.id() != item_id);
        state
            .tag_stack
            .retain(|entry| entry.origin.item.id() != item_id && entry.target.item.id() != item_id);
    }

    pub fn rename_item(
        &mut self,
        item_id: EntityId,
        project_path: ProjectPath,
        abs_path: Option<PathBuf>,
    ) {
        let mut state = self.0.lock();
        let path_for_item = state.paths_by_item.get_mut(&item_id);
        if let Some(path_for_item) = path_for_item {
            path_for_item.0 = project_path;
            path_for_item.1 = abs_path;
        }
    }

    pub fn path_for_item(&self, item_id: EntityId) -> Option<(ProjectPath, Option<PathBuf>)> {
        self.0.lock().paths_by_item.get(&item_id).cloned()
    }

    pub fn push_tag(&mut self, origin: NavigationEntry, target: NavigationEntry) {
        let mut state = self.0.lock();
        let truncate_to = state.tag_stack_pos;
        state.tag_stack.truncate(truncate_to);
        state.tag_stack.push_back(TagStackEntry { origin, target });
        state.tag_stack_pos = state.tag_stack.len();
    }

    pub fn pop_tag(&mut self, mode: TagNavigationMode) -> Option<NavigationEntry> {
        let mut state = self.0.lock();
        match mode {
            TagNavigationMode::Older => {
                if state.tag_stack_pos > 0 {
                    state.tag_stack_pos -= 1;
                    state
                        .tag_stack
                        .get(state.tag_stack_pos)
                        .map(|e| e.origin.clone())
                } else {
                    None
                }
            }
            TagNavigationMode::Newer => {
                let entry = state
                    .tag_stack
                    .get(state.tag_stack_pos)
                    .map(|e| e.target.clone());
                if state.tag_stack_pos < state.tag_stack.len() {
                    state.tag_stack_pos += 1;
                }
                entry
            }
        }
    }
}

impl NavHistoryState {
    pub fn did_update(&self, cx: &mut App) {
        if let Some(pane) = self.pane.upgrade() {
            cx.defer(move |cx| {
                pane.update(cx, |pane, cx| pane.history_updated(cx));
            });
        }
    }
}
