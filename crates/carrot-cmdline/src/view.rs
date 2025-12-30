//! Cmdline view — the GPUI-facing surface of the command entry.
//!
//! Composition, not inheritance: `Cmdline` holds an
//! `Entity<carrot_editor::Editor>` directly and renders it in its
//! own `render()`. Matches the pattern of `CommandPalette`,
//! `FileFinder`, `GoToLine`, `InputField` — no wrapper struct, no
//! trait adapter.
//!
//! # Editor mode
//!
//! `Editor::auto_height(1, 20, window, cx)`:
//!
//! - Starts at 1 visual line, grows up to 20 as the user types.
//! - Soft-wrap enabled (editor default for AutoHeight).
//! - Enter inserts a newline by default.
//!
//! # Smart-Enter
//!
//! The cmdline subscribes to `menu::Confirm` (the action GPUI emits
//! for `Enter` when no popover is open). The handler inspects the
//! current [`CommandAst`]:
//!
//! - If [`CommandAst::is_complete`] is `true` → the command is
//!   submitted (emits a [`CmdlineEvent::Submit`]) and the editor is
//!   not touched.
//! - Otherwise → the handler propagates the action (doesn't call
//!   `cx.propagate()` / doesn't stop it), letting the editor's own
//!   Enter binding insert a newline. This is the "falls through to
//!   editor default" fallback from the spec.

use std::sync::Arc;

use carrot_editor::Editor;
use inazuma::{
    AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, Styled, Window, div,
};
use inazuma_menu::Confirm;

use crate::ast::CommandAst;
use crate::session::CmdlineSession;
use crate::shell::ShellKind;

/// Events emitted upward from the Cmdline view to its host
/// (typically the terminal pane or a block mount site).
#[derive(Debug, Clone)]
pub enum CmdlineEvent {
    /// User pressed Enter on a complete command line — host should
    /// forward the buffer to the PTY (OSC 133 `C` will follow from
    /// the shell).
    Submit {
        /// The text as it stood when the user hit Enter.
        buffer: Arc<str>,
    },
    /// User pressed Ctrl+C. Host should cancel any in-flight agent
    /// work and clear the buffer.
    Cancel,
}

/// The command-entry view.
///
/// Holds a live `Entity<Editor>` for the buffer, a `CmdlineSession`
/// for the semantic AST + completion state, and a `FocusHandle` the
/// host can focus.
pub struct Cmdline {
    editor: Entity<Editor>,
    session: CmdlineSession,
    focus_handle: FocusHandle,
}

impl Cmdline {
    /// Construct a new Cmdline bound to `shell`. Creates the backing
    /// editor in `auto_height(1, 20)` mode.
    pub fn new(shell: ShellKind, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| Editor::auto_height(1, 20, window, cx));
        let focus_handle = editor.focus_handle(cx);
        let session = CmdlineSession::new(shell);
        Self {
            editor,
            session,
            focus_handle,
        }
    }

    /// Read-only access to the backing editor — used by tests and by
    /// higher layers that need to call editor APIs (e.g. the AST
    /// highlighter in `HighlightKey::CommandAst(tier)`).
    pub fn editor(&self) -> &Entity<Editor> {
        &self.editor
    }

    /// Read-only access to the semantic session.
    pub fn session(&self) -> &CmdlineSession {
        &self.session
    }

    /// Rebuild the AST from the editor's current text. The host
    /// calls this after any external mutation (e.g. AI ghost-text
    /// commit, history-recall, agent edit) or after a keystroke
    /// when subscribing to the editor's buffer events.
    pub fn refresh_ast(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).text(cx);
        self.session.set_buffer(&text);
    }

    /// Access the current command AST (stale until `refresh_ast`
    /// has been called after the latest edit).
    pub fn ast(&self) -> &CommandAst {
        self.session.ast()
    }

    /// Smart-Enter handler. Wired as `on_action::<Confirm>` in
    /// `render()`. If the AST is complete, emits
    /// [`CmdlineEvent::Submit`] and stops the action; otherwise
    /// propagates so the editor inserts a newline.
    fn handle_confirm(&mut self, _: &Confirm, _window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_ast(cx);
        if self.session.ast().is_complete() {
            let buffer: Arc<str> = Arc::from(self.editor.read(cx).text(cx));
            cx.emit(CmdlineEvent::Submit { buffer });
        } else {
            // Don't stop the action — let the editor's own Enter
            // binding produce a newline. This is the "falls through
            // to editor default" path from the spec.
            cx.propagate();
        }
    }
}

impl EventEmitter<CmdlineEvent> for Cmdline {}

impl Focusable for Cmdline {
    fn focus_handle(&self, _cx: &inazuma::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Cmdline {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Cmdline")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::handle_confirm))
            .size_full()
            .child(self.editor.clone())
    }
}
