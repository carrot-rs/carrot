//! Carrot Terminal View — Feature Crate for terminal UI.
//!
//! Contains terminal rendering (grid, blocks, colors, built-in font),
//! the TerminalPane (Workspace Item), shell install modal, and history panel.
//! Imports backend logic from carrot-terminal, carrot-completions, carrot-session, carrot-shell.

// Terminal rendering modules
pub mod block_interaction;
pub mod block_list;
pub mod block_search;
pub mod builtin_font;
pub mod colors;
pub mod constants;

// Terminal UI modules
pub mod branch_picker;
pub mod input;
pub mod shell_install_modal;
pub mod terminal_pane;
pub mod terminal_panel;

use std::rc::Rc;

use carrot_project::Project;
use carrot_terminal::Terminal;
use carrot_workspace::{Workspace, item::ItemEvent};
use inazuma::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Render, Window,
    prelude::*,
};

/// The content mode of a terminal view, describing how content is displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalContentMode {
    /// Normal terminal output.
    Normal,
    /// Terminal has scrollable content (e.g. alternate screen buffer).
    Scrollable,
}

impl TerminalContentMode {
    /// Returns whether this content mode allows scrolling.
    pub fn is_scrollable(&self) -> bool {
        matches!(self, Self::Scrollable)
    }
}

/// Properties for a block rendered below the terminal cursor.
/// Used by carrot-agent-ui, carrot-debugger-ui, carrot-repl.
pub struct BlockProperties {
    pub height: u8,
    pub render: Box<dyn Send + Fn(&mut BlockContext) -> AnyElement>,
}

/// Context passed to block render functions.
pub struct BlockContext<'a, 'b> {
    pub window: &'a mut Window,
    pub context: &'b mut App,
}

/// A terminal view — backward-compatible API for external crates.
/// The real terminal UI lives in `terminal_pane::TerminalPane` which implements
/// the Workspace `Item` trait with our full block system.
pub struct TerminalView {
    terminal: Entity<Terminal>,
    focus_handle: FocusHandle,
}

impl TerminalView {
    pub fn new(
        terminal: Entity<Terminal>,
        _workspace: Entity<Workspace>,
        _custom_title: Option<String>,
        _project: inazuma::WeakEntity<Project>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            terminal,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn terminal(&self) -> &Entity<Terminal> {
        &self.terminal
    }

    pub fn set_block_below_cursor(
        &mut self,
        _block: Rc<BlockProperties>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    /// Set embedded mode with an optional max line height.
    /// In embedded mode, the terminal restricts its output display.
    pub fn set_embedded_mode(&mut self, _max_lines: Option<u32>, _cx: &mut Context<Self>) {}

    /// Returns the content mode of this terminal view.
    pub fn content_mode(&self, _window: &Window, _cx: &App) -> TerminalContentMode {
        TerminalContentMode::Normal
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<ItemEvent> for TerminalView {}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        inazuma::div().size_full()
    }
}

/// Initialize the terminal view system.
///
/// Registers action handlers on every new Workspace:
/// - `workspace::NewTerminal` → creates a new TerminalPane tab
pub fn init(cx: &mut App) {
    carrot_ui::input::erased_editor_impl::register_input_editor_factory();
    cx.observe_new(
        |workspace: &mut Workspace, _window, _cx: &mut inazuma::Context<Workspace>| {
            workspace.register_action(
                |workspace, _action: &carrot_workspace::NewTerminal, window, cx| {
                    let terminal = cx.new(|cx| crate::terminal_pane::TerminalPane::new(window, cx));
                    workspace.add_item_to_active_pane(Box::new(terminal), None, true, window, cx);
                },
            );
            workspace.register_action(
                |workspace, _: &carrot_actions::TrackActiveScope, _window, cx| {
                    let Some(terminal_pane) = workspace
                        .active_item(cx)
                        .and_then(|item| item.downcast::<crate::terminal_pane::TerminalPane>())
                    else {
                        return;
                    };
                    let pane = terminal_pane.read(cx);
                    let Some(git_root) = pane.current_git_root.clone() else {
                        return;
                    };
                    let Some(project) = pane.project.as_ref().and_then(|p| p.upgrade()) else {
                        return;
                    };
                    project.update(cx, |project, cx| {
                        project
                            .ensure_tracked_worktree(&git_root, cx)
                            .detach_and_log_err(cx);
                    });
                },
            );
            workspace.register_action(
                |workspace, _: &carrot_actions::NeverTrackScope, _window, cx| {
                    let Some(terminal_pane) = workspace
                        .active_item(cx)
                        .and_then(|item| item.downcast::<crate::terminal_pane::TerminalPane>())
                    else {
                        return;
                    };
                    let pane = terminal_pane.read(cx);
                    let Some(git_root) = pane.current_git_root.clone() else {
                        return;
                    };
                    let fs = workspace.project().read(cx).fs().clone();
                    inazuma_settings_framework::update_settings_file(fs, cx, move |content, _| {
                        let mut scope = content.worktree_scope.clone().unwrap_or_default();
                        let mut paths = scope.never_track_paths.unwrap_or_default();
                        let new_entry = git_root.display().to_string();
                        if !paths.iter().any(|p| p == &new_entry) {
                            paths.push(new_entry);
                        }
                        scope.never_track_paths = Some(paths);
                        content.worktree_scope = Some(scope);
                    });
                },
            );
            workspace.register_action(
                |_workspace, _: &carrot_actions::TrackAskLater, _window, _cx| {
                    log::debug!("TrackAskLater: notification dismissed");
                },
            );
        },
    )
    .detach();
}
