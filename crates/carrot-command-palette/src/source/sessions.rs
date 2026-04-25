use carrot_workspace::Workspace;
use inazuma::{App, Entity, Window};

use crate::category::SearchCategory;
use crate::source::{SearchAction, SearchResult, SearchSource};

/// Surfaces all open workspace sessions except the active one — switching
/// to the active session would be a no-op and adding it would just look
/// like noise.
pub struct SessionsSource;

impl SearchSource for SessionsSource {
    fn category(&self) -> SearchCategory {
        SearchCategory::Sessions
    }

    fn default_visible(&self) -> bool {
        // Kept out of the empty-query "Suggested" view — session names
        // don't add much before the user types, and Cmd+Shift+P / the
        // Sessions chip is the intended shortcut for switching.
        false
    }

    fn collect(
        &self,
        workspace: &Entity<Workspace>,
        _query: &str,
        _window: &Window,
        cx: &mut App,
    ) -> Vec<SearchResult> {
        let workspace = workspace.read(cx);
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
