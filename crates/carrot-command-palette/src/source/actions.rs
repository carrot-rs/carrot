use std::cmp::Reverse;
use std::collections::HashMap;

use carrot_command_palette_hooks::CommandPaletteFilter;
use carrot_ui::IconName;
use carrot_workspace::Workspace;
use inazuma::{App, Entity, Window};

use crate::action_name::humanize_action_name;
use crate::category::SearchCategory;
use crate::persistence::CommandPaletteDB;
use crate::source::{SearchAction, SearchResult, SearchSource};

/// Curated suggestion list for the empty-query "Suggested" section.
/// Deliberately tiny — just a pair of shortcuts, matching Warp's
/// "Create Workflow" / "Open Theme Picker" pattern. Actions the focused
/// element hasn't registered are silently skipped. Order here is the
/// render order.
const CURATED_SUGGESTIONS: &[&str] = &["theme_selector::Toggle", "carrot::OpenSettings"];

/// Dynamic workspace action discovery. Every action currently dispatchable
/// from the focused element is surfaced here, filtered through the global
/// `CommandPaletteFilter` and ranked by the usage history persisted in
/// `CommandPaletteDB`.
pub struct ActionsSource;

impl SearchSource for ActionsSource {
    fn category(&self) -> SearchCategory {
        SearchCategory::Actions
    }

    fn collect(
        &self,
        _workspace: &Entity<Workspace>,
        query: &str,
        window: &Window,
        cx: &mut App,
    ) -> Vec<SearchResult> {
        let filter = CommandPaletteFilter::try_global(cx);
        let hit_counts = load_hit_counts(cx);

        let mut entries: Vec<(String, Box<dyn inazuma::Action>)> = window
            .available_actions(cx)
            .into_iter()
            .filter_map(|action| {
                if filter.is_some_and(|f| f.is_hidden(&*action)) {
                    return None;
                }
                let name = humanize_action_name(action.name());
                Some((name, action))
            })
            .collect();

        // Empty-query path is the "Suggested" section. Return only the
        // hand-picked entries from `CURATED_SUGGESTIONS`, skipping any
        // that aren't currently registered. Keeps the opening view small
        // instead of flooding it with every workspace action.
        if query.is_empty() {
            let mut curated: Vec<SearchResult> = Vec::new();
            for wanted in CURATED_SUGGESTIONS {
                if let Some(pos) = entries.iter().position(|(_, a)| a.name() == *wanted) {
                    let (humanized, action) = entries.swap_remove(pos);
                    let raw_name = action.name().to_string();
                    curated.push(SearchResult {
                        id: format!("action:{raw_name}").into(),
                        category: SearchCategory::Actions,
                        title: humanized.into(),
                        subtitle: Some(raw_name.into()),
                        icon: IconName::BoltFilled,
                        action: SearchAction::DispatchAction(action),
                    });
                }
            }
            return curated;
        }

        // Frecency: entries with higher invocation counts float to the top
        // so repeated use keeps the most-used commands in the first rows,
        // matching the behavior of the previous Zed-style palette.
        entries.sort_by_key(|(name, _)| (Reverse(hit_counts.get(name).copied()), name.clone()));

        entries
            .into_iter()
            .map(|(humanized, action)| {
                let raw_name = action.name().to_string();
                SearchResult {
                    id: format!("action:{raw_name}").into(),
                    category: SearchCategory::Actions,
                    title: humanized.into(),
                    subtitle: Some(raw_name.into()),
                    icon: IconName::BoltFilled,
                    action: SearchAction::DispatchAction(action),
                }
            })
            .collect()
    }
}

fn load_hit_counts(cx: &App) -> HashMap<String, u16> {
    match CommandPaletteDB::global(cx).list_commands_used() {
        Ok(commands) => commands
            .into_iter()
            .map(|c| (c.command_name, c.invocations))
            .collect(),
        Err(_) => HashMap::new(),
    }
}
