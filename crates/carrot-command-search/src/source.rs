//! Search sources — providers of searchable items.
//!
//! A `SearchSource` owns one category and produces `SearchResult`s for the
//! command search modal. Results carry a `SearchAction` so the modal can
//! dispatch them without knowing source-specific types.

use std::sync::Arc;

use carrot_actions::theme_selector;
use carrot_actions::{OpenKeymap, OpenSettings};
use carrot_ui::IconName;
use carrot_workspace::Workspace;
use inazuma::{Action, App, ClipboardItem, Entity, SharedString, Window};

use crate::category::SearchCategory;

/// One row in the result list. The `id` is stable within a single source so
/// the list can preserve selection across re-renders.
pub struct SearchResult {
    pub id: SharedString,
    pub category: SearchCategory,
    pub title: SharedString,
    pub subtitle: Option<SharedString>,
    pub icon: IconName,
    pub action: SearchAction,
}

/// Runnable invocation bound to a result. Kept as a closed enum so the modal
/// can dispatch without needing a trait object that captures the workspace.
pub enum SearchAction {
    /// Switch to the session at this index.
    ActivateSession { index: usize },
    /// Copy the given text to the clipboard (used for env vars).
    CopyToClipboard(String),
    /// Dispatch a global action through the window — the workspace (or
    /// whichever layer registered the handler) runs the action.
    DispatchAction(Box<dyn Action>),
}

impl SearchAction {
    pub fn run(self, workspace: Entity<Workspace>, window: &mut Window, cx: &mut App) {
        match self {
            SearchAction::ActivateSession { index } => {
                workspace.update(cx, |ws, cx| {
                    ws.activate_session(index, window, cx);
                });
            }
            SearchAction::CopyToClipboard(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            SearchAction::DispatchAction(action) => {
                window.dispatch_action(action, cx);
            }
        }
    }
}

/// Provider of searchable items for one category. `collect()` is called with
/// the current workspace snapshot each time results are refreshed; sources
/// should be cheap to call and produce plain data.
pub trait SearchSource: Send + Sync {
    fn category(&self) -> SearchCategory;
    fn collect(&self, workspace: &Workspace, cx: &App) -> Vec<SearchResult>;

    /// Whether this source contributes to the default (no-filter) view.
    /// Bulk sources like env vars opt out so the modal doesn't flood the
    /// result list with hundreds of items on open — they still participate
    /// when the user types a query or selects the matching chip.
    fn default_visible(&self) -> bool {
        true
    }
}

/// Built-in sources registered by the command search modal. Order matters —
/// `Actions` first so curated commands are the most prominent row when the
/// modal opens, followed by `Sessions`, then bulk sources.
pub fn default_sources() -> Vec<Arc<dyn SearchSource>> {
    vec![
        Arc::new(ActionsSource),
        Arc::new(SessionsSource),
        Arc::new(EnvVarsSource),
    ]
}

struct SessionsSource;

impl SearchSource for SessionsSource {
    fn category(&self) -> SearchCategory {
        SearchCategory::Sessions
    }

    fn collect(&self, workspace: &Workspace, cx: &App) -> Vec<SearchResult> {
        // The active session is skipped — the user is already in it, so
        // "switching" would be a no-op and the row just looks like noise.
        let active = workspace.active_session_index();
        workspace
            .sessions()
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != active)
            .map(|(index, session)| {
                let session = session.read(cx);
                let title = session.display_label(cx);
                SearchResult {
                    id: format!("session:{index}").into(),
                    category: SearchCategory::Sessions,
                    title,
                    subtitle: None,
                    icon: SearchCategory::Sessions.icon(),
                    action: SearchAction::ActivateSession { index },
                }
            })
            .collect()
    }
}

struct EnvVarsSource;

impl SearchSource for EnvVarsSource {
    fn category(&self) -> SearchCategory {
        SearchCategory::EnvironmentVariables
    }

    fn collect(&self, _workspace: &Workspace, _cx: &App) -> Vec<SearchResult> {
        let mut entries: Vec<(String, String)> = std::env::vars().collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
            .into_iter()
            .map(|(name, value)| {
                let clipboard_payload = format!("{name}={value}");
                SearchResult {
                    id: format!("env:{name}").into(),
                    category: SearchCategory::EnvironmentVariables,
                    title: name.into(),
                    subtitle: Some(value.into()),
                    icon: SearchCategory::EnvironmentVariables.icon(),
                    action: SearchAction::CopyToClipboard(clipboard_payload),
                }
            })
            .collect()
    }

    /// Env vars are opt-in: hundreds of entries would drown the modal on
    /// open. They appear only when the user types or clicks the chip.
    fn default_visible(&self) -> bool {
        false
    }
}

struct ActionsSource;

impl SearchSource for ActionsSource {
    fn category(&self) -> SearchCategory {
        SearchCategory::Actions
    }

    fn collect(&self, _workspace: &Workspace, _cx: &App) -> Vec<SearchResult> {
        fn entry(
            id: &'static str,
            title: &'static str,
            subtitle: &'static str,
            icon: IconName,
            action: Box<dyn Action>,
        ) -> SearchResult {
            SearchResult {
                id: SharedString::new_static(id),
                category: SearchCategory::Actions,
                title: SharedString::new_static(title),
                subtitle: Some(SharedString::new_static(subtitle)),
                icon,
                action: SearchAction::DispatchAction(action),
            }
        }

        // Session/tab lifecycle actions (new/rename/close) live in the
        // Vertical Tabs right-click menu — this panel is for unified
        // content search (commands, prompts, conversations, workflows),
        // plus a handful of app-level navigation shortcuts.
        vec![
            entry(
                "action:open-theme-picker",
                "Switch theme",
                "Open the theme picker",
                IconName::Palette,
                Box::new(theme_selector::Toggle::default()),
            ),
            entry(
                "action:open-settings",
                "Open settings",
                "Open the settings editor",
                IconName::Settings,
                Box::new(OpenSettings),
            ),
            entry(
                "action:open-keymap",
                "Open keymap editor",
                "Edit keyboard shortcuts",
                IconName::Keyboard,
                Box::new(OpenKeymap),
            ),
        ]
    }
}
