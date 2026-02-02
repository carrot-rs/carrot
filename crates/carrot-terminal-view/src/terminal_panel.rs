//! Stub terminal panel — will be replaced in Phase 20.

use carrot_workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};
use inazuma::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels, Render, Window, prelude::*,
    px,
};

/// Stub terminal panel.
pub struct TerminalPanel {
    focus_handle: FocusHandle,
}

impl TerminalPanel {
    pub fn new(_workspace: &Workspace, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
        }
    }

    /// Add a fresh terminal pane as the active item in the workspace.
    ///
    /// Handles the `terminal::NewTerminal` action. Mirrors the startup path
    /// in `carrot-app/src/main.rs` so that new sessions (spawned via
    /// `Workspace::new_session`) get a live terminal instead of remaining
    /// blank. Terminal-first ADE contract: every session starts with a
    /// terminal pane.
    pub fn new_terminal(
        workspace: &mut Workspace,
        _action: &NewTerminal,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let terminal = cx.new(|cx| crate::terminal_pane::TerminalPane::new(window, cx));
        workspace.add_item_to_active_pane(Box::new(terminal), None, true, window, cx);
    }

    /// Handle `terminal::OpenTerminal` — alias for `NewTerminal` in the
    /// current single-terminal-per-pane world. Kept as a separate action
    /// so menus / keybindings can distinguish "new terminal" (explicit
    /// tab / split) from "open terminal" (jump to an existing one).
    pub fn open_terminal(
        workspace: &mut Workspace,
        _action: &OpenTerminal,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        Self::new_terminal(workspace, &NewTerminal, window, cx);
    }

    pub fn terminal_selections(&self, _cx: &App) -> Vec<String> {
        Vec::new()
    }

    /// Whether the assistant integration is enabled for this terminal panel.
    pub fn assistant_enabled(&self) -> bool {
        false
    }

    /// Enable or disable assistant integration for this terminal panel.
    pub fn set_assistant_enabled(&mut self, _enabled: bool, _cx: &mut Context<Self>) {}

    /// Spawn a task in a new terminal tab, returning a handle to the terminal.
    pub fn spawn_task(
        &mut self,
        _task: &carrot_task::ResolvedTask,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> inazuma::Task<anyhow::Result<Entity<carrot_terminal::Terminal>>> {
        cx.spawn(async move |_this, _cx| {
            anyhow::bail!("TerminalPanel task spawning — not yet implemented")
        })
    }

    pub async fn load(
        _workspace: inazuma::WeakEntity<Workspace>,
        _cx: inazuma::AsyncWindowContext,
    ) -> anyhow::Result<Entity<Self>> {
        anyhow::bail!("TerminalPanel stub — Phase 20 not yet implemented")
    }
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for TerminalPanel {}

impl Panel for TerminalPanel {
    fn persistent_name() -> &'static str {
        "TerminalPanel"
    }

    fn panel_key() -> &'static str {
        "TerminalPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Bottom
    }

    fn position_is_valid(&self, _position: DockPosition) -> bool {
        true
    }

    fn set_position(
        &mut self,
        _position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(300.0)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<carrot_ui::IconName> {
        None
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Terminal")
    }

    fn toggle_action(&self) -> Box<dyn inazuma::Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        5
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        inazuma::div().size_full()
    }
}

/// Initialize the terminal panel.
///
/// Registers workspace-level action handlers so that `terminal::NewTerminal`
/// (dispatched e.g. from `Workspace::new_session`) actually spawns a fresh
/// terminal pane, and `terminal::OpenTerminal` opens one in the active pane.
pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _window, _cx| {
        workspace.register_action(TerminalPanel::new_terminal);
        workspace.register_action(TerminalPanel::open_terminal);
    })
    .detach();
}

use inazuma::actions;
actions!(
    terminal,
    [
        NewTerminal,
        OpenTerminal,
        ToggleFocus,
        Toggle,
        SendInterrupt
    ]
);
