use carrot_workspace::Workspace;
use inazuma::{App, Entity, Window};

use crate::category::SearchCategory;
use crate::source::{SearchAction, SearchResult, SearchSource};

/// Environment-variable copier. Opt-in: hundreds of entries would drown
/// the modal on open, so the source only participates when the user types
/// a query or selects the matching chip.
pub struct EnvVarsSource;

impl SearchSource for EnvVarsSource {
    fn category(&self) -> SearchCategory {
        SearchCategory::EnvironmentVariables
    }

    fn collect(
        &self,
        _workspace: &Entity<Workspace>,
        _query: &str,
        _window: &Window,
        _cx: &mut App,
    ) -> Vec<SearchResult> {
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

    fn default_visible(&self) -> bool {
        false
    }

    fn searchable(&self) -> bool {
        // Hundreds of env vars would fuzzy-match almost every keystroke
        // and swamp real matches. The `env:` prefix and chip remain the
        // explicit entry points.
        false
    }
}
