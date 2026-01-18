//! WorkspaceSession — per-tab session model.
//!
//! Architecture: `Workspace` owns multiple `WorkspaceSession`s, each rendered
//! as a top-level tab in the title bar. A session owns its own `PaneGroup`
//! (split tree) and tracks its own active pane, zoom state, and follower
//! state. Panes within a session are single-item containers.

use std::sync::atomic::{AtomicUsize, Ordering};

use inazuma::{AnyWeakView, App, Context, Entity, EventEmitter, Oklch, SharedString, WeakEntity};
use inazuma_collections::HashMap;

use crate::{
    CollaboratorId, FollowerState, dock::DockPosition, item::PaneRole, pane::Pane,
    pane_group::PaneGroup,
};

/// Stable identifier for a session within a workspace. Generated monotonically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(pub usize);

impl SessionId {
    /// Allocate the next sequential session id (monotonic, never reused).
    pub fn next() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// Events emitted by a `WorkspaceSession` so the parent `Workspace` can react
/// (re-render tab bar, remove empty sessions, persist state, ...).
#[derive(Clone, Debug)]
pub enum SessionEvent {
    /// The session's active pane changed (focus moved between splits).
    ActivePaneChanged,
    /// A pane was added or removed from the session's pane group.
    PanesChanged,
    /// The session's display name changed (rename via context menu).
    NameChanged,
    /// The session's color changed (color picker in context menu).
    ColorChanged,
    /// The last pane in the session was closed — the parent workspace must
    /// remove this session from its session list (and close the window if it
    /// was the last session).
    Empty,
}

/// A single tab in the workspace. Owns its own pane tree and is
/// independent from sibling sessions in the same window.
pub struct WorkspaceSession {
    id: SessionId,
    /// User-provided display name (via context menu rename). `None` falls back
    /// to the active pane's item-derived label.
    name: Option<SharedString>,
    /// User-chosen tab color (via context menu color picker).
    color: Option<Oklch>,
    pane_group: PaneGroup,
    panes: Vec<Entity<Pane>>,
    active_pane: Entity<Pane>,
    last_active_center_pane: Option<WeakEntity<Pane>>,
    /// Last pane (in this session) whose item declared `PaneRole::Editor` and
    /// held focus. Used by `Workspace::add_item_smart()` to route file-open
    /// requests into the appropriate editor pane instead of clobbering a
    /// terminal pane. Wired up via the `PaneRole` trait method.
    last_active_editor_pane: Option<WeakEntity<Pane>>,
    zoomed: Option<AnyWeakView>,
    zoomed_position: Option<DockPosition>,
    follower_states: HashMap<CollaboratorId, FollowerState>,
}

impl EventEmitter<SessionEvent> for WorkspaceSession {}

impl WorkspaceSession {
    /// Create a new session wrapping a single initial pane.
    pub fn new(initial_pane: Entity<Pane>, _cx: &mut Context<Self>) -> Self {
        let mut pane_group = PaneGroup::new(initial_pane.clone());
        pane_group.set_is_center(true);
        Self {
            id: SessionId::next(),
            name: None,
            color: None,
            panes: vec![initial_pane.clone()],
            last_active_center_pane: Some(initial_pane.downgrade()),
            active_pane: initial_pane,
            pane_group,
            last_active_editor_pane: None,
            zoomed: None,
            zoomed_position: None,
            follower_states: HashMap::default(),
        }
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn active_pane(&self) -> &Entity<Pane> {
        &self.active_pane
    }

    pub fn pane_group(&self) -> &PaneGroup {
        &self.pane_group
    }

    pub fn pane_group_mut(&mut self) -> &mut PaneGroup {
        &mut self.pane_group
    }

    pub fn panes(&self) -> &[Entity<Pane>] {
        &self.panes
    }

    pub fn last_active_center_pane(&self) -> Option<&WeakEntity<Pane>> {
        self.last_active_center_pane.as_ref()
    }

    pub fn last_active_editor_pane(&self) -> Option<&WeakEntity<Pane>> {
        self.last_active_editor_pane.as_ref()
    }

    pub fn zoomed(&self) -> Option<&AnyWeakView> {
        self.zoomed.as_ref()
    }

    pub fn zoomed_position(&self) -> Option<DockPosition> {
        self.zoomed_position
    }

    pub fn follower_states(&self) -> &HashMap<CollaboratorId, FollowerState> {
        &self.follower_states
    }

    pub fn follower_states_mut(&mut self) -> &mut HashMap<CollaboratorId, FollowerState> {
        &mut self.follower_states
    }

    /// Display label used in the title-bar tab. Returns the user-set name if
    /// present, otherwise falls back to the active pane's active item label
    /// (e.g. terminal cwd, file path).
    pub fn display_label(&self, cx: &App) -> SharedString {
        if let Some(name) = &self.name {
            return name.clone();
        }
        let pane = self.active_pane.read(cx);
        if let Some(item) = pane.active_item() {
            return item.tab_content_text(0, cx);
        }
        SharedString::new_static("Session")
    }

    pub fn name(&self) -> Option<&SharedString> {
        self.name.as_ref()
    }

    pub fn set_name(&mut self, name: Option<SharedString>, cx: &mut Context<Self>) {
        if self.name != name {
            self.name = name;
            cx.emit(SessionEvent::NameChanged);
            cx.notify();
        }
    }

    pub fn color(&self) -> Option<Oklch> {
        self.color
    }

    pub fn set_color(&mut self, color: Option<Oklch>, cx: &mut Context<Self>) {
        if self.color != color {
            self.color = color;
            cx.emit(SessionEvent::ColorChanged);
            cx.notify();
        }
    }

    /// Update which pane in this session is currently focused. Also tracks
    /// the most recent editor-role pane for smart file-open routing.
    pub fn set_active_pane(&mut self, pane: &Entity<Pane>, cx: &mut Context<Self>) {
        if &self.active_pane == pane {
            return;
        }
        self.active_pane = pane.clone();
        self.last_active_center_pane = Some(pane.downgrade());
        if let Some(item) = pane.read(cx).active_item() {
            if item.pane_role(cx) == PaneRole::Editor {
                self.last_active_editor_pane = Some(pane.downgrade());
            }
        }
        cx.emit(SessionEvent::ActivePaneChanged);
        cx.notify();
    }

    /// Append a freshly created pane to the session's pane list. Caller is
    /// responsible for inserting it into the `PaneGroup` tree first via
    /// `pane_group_mut().split(...)`.
    pub fn add_pane(&mut self, pane: Entity<Pane>, cx: &mut Context<Self>) {
        self.panes.push(pane);
        cx.emit(SessionEvent::PanesChanged);
        cx.notify();
    }

    /// Remove a pane from the session. Emits `Empty` if it was the last pane,
    /// signaling the parent workspace to drop this session.
    pub fn remove_pane(&mut self, pane: &Entity<Pane>, cx: &mut Context<Self>) {
        self.panes.retain(|p| p != pane);
        if self.panes.is_empty() {
            cx.emit(SessionEvent::Empty);
        } else {
            cx.emit(SessionEvent::PanesChanged);
        }
        cx.notify();
    }

    /// Replace the session's pane state in one shot. Used by the
    /// `Workspace::sync_active_session()` mirror bridge during Phase 1 so
    /// that the active session always reflects the workspace's legacy pane
    /// fields. Phase 3's `Workspace::activate_session()` reads this state
    /// back via the getters when switching tabs.
    ///
    /// Note: `zoomed` and `zoomed_position` are not mirrored yet because
    /// `AnyWeakView` is not Clone. Phase 3 will handle that via dedicated
    /// move-semantics during real session switching.
    pub fn replace_pane_state(
        &mut self,
        pane_group: PaneGroup,
        panes: Vec<Entity<Pane>>,
        active_pane: Entity<Pane>,
        last_active_center_pane: Option<WeakEntity<Pane>>,
    ) {
        self.pane_group = pane_group;
        self.panes = panes;
        self.active_pane = active_pane;
        self.last_active_center_pane = last_active_center_pane;
    }
}
