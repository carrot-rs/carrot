//! Session + pane handler methods (rename, close, activate, new).
//!
//! These are the leaves of the user interactions the vertical tab
//! surfaces: click a row → `activate_session` / `activate_pane`; `×` →
//! `close_session` / `close_pane`; double-click → `start_rename` /
//! `finish_rename`; `+` → `new_session`. Each one bounces off the
//! workspace or session entity directly and has no render concerns,
//! so they group naturally into one "actions" module and keep the
//! render path from reaching into entity-update territory inline.

use carrot_ui::input::{InputEvent, InputState};
use inazuma::{App, AppContext, Context, Focusable, SharedString, Window};

use crate::VerticalTabsPanel;

impl VerticalTabsPanel {
    /// Enter inline-rename mode for the session at `index`. The
    /// `InputState` is seeded with the current label and fully
    /// selected so typing immediately replaces the text.
    pub(crate) fn start_rename(
        &mut self,
        index: usize,
        label: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| {
            state.insert(&label, window, cx);
            state.select_all(&carrot_ui::input::SelectAll, window, cx);
        });
        window.focus(&input.focus_handle(cx), cx);

        cx.subscribe_in(
            &input,
            window,
            move |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    this.finish_rename(true, window, cx);
                }
                InputEvent::Escape => {
                    this.finish_rename(false, window, cx);
                }
                _ => {}
            },
        )
        .detach();

        self.rename_in_progress = Some((index, input));
        cx.notify();
    }

    /// Close the inline rename. When `commit` is true, persist the
    /// typed text via `WorkspaceSession::set_name`; when false,
    /// discard.
    pub(crate) fn finish_rename(
        &mut self,
        commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((index, input)) = self.rename_in_progress.take() else {
            return;
        };

        if commit {
            let text = input.read(cx).value();
            let trimmed = text.trim();
            let new_name: Option<SharedString> = if trimmed.is_empty() {
                None
            } else {
                Some(SharedString::from(trimmed.to_string()))
            };
            if let Some(session) = self.cached_sessions.get(index) {
                session.update(cx, |s, cx| s.set_name(new_name, cx));
            }
        }

        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    pub(crate) fn close_session(&self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |ws, cx| ws.close_session(index, window, cx));
        }
    }

    /// Close one specific pane inside a session (Panes view mode). If
    /// the pane being removed was the session's last pane the session
    /// itself closes — `WorkspaceSession::remove_pane` emits `Empty`
    /// and the workspace reacts accordingly.
    pub(crate) fn close_pane(
        &self,
        session_index: usize,
        pane_id: inazuma::EntityId,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.cached_sessions.get(session_index).cloned() else {
            return;
        };
        session.update(cx, |session, cx| {
            if let Some(pane) = session
                .panes()
                .iter()
                .find(|p| p.entity_id() == pane_id)
                .cloned()
            {
                session.remove_pane(&pane, cx);
            }
        });
    }

    /// Activate a specific pane inside a session (Panes view mode).
    /// Activates the session first so the workspace focus lands
    /// there, then promotes the chosen pane to the session's active
    /// pane.
    pub(crate) fn activate_pane(
        &self,
        session_index: usize,
        pane_id: inazuma::EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        workspace.update(cx, |workspace, cx| {
            workspace.activate_session(session_index, window, cx);
        });
        let Some(session) = self.cached_sessions.get(session_index).cloned() else {
            return;
        };
        session.update(cx, |session, cx| {
            if let Some(pane) = session
                .panes()
                .iter()
                .find(|p| p.entity_id() == pane_id)
                .cloned()
            {
                session.set_active_pane(&pane, cx);
            }
        });
    }

    pub(crate) fn new_session(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                workspace.new_session(window, cx);
            });
        }
    }

    pub(crate) fn search_query(&self, cx: &App) -> String {
        self.search_input.read(cx).value().to_string()
    }

    pub(crate) fn activate_session(
        &self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                workspace.activate_session(index, window, cx);
            });
        }
    }
}
