//! History source — recalls the user's shell command history.
//!
//! Loads entries from the shell's history file on first access and caches
//! them for subsequent refreshes so typing stays responsive. Enter inserts
//! the selected command into the active terminal pane's input editor
//! without executing it, giving the user a chance to edit first.

use std::sync::Mutex;

use carrot_actions::command_palette::InsertIntoInput;
use carrot_session::command_history::{
    ActiveCommandHistory, CommandHistory, HistoryEntry, relative_time,
};
use carrot_workspace::Workspace;
use inazuma::{App, Entity, SharedString, Window};

use crate::category::SearchCategory;
use crate::source::{SearchAction, SearchResult, SearchSource};

/// How many entries to surface when the user hasn't typed a query. A few
/// pages of recent commands; the prefix-triggered `history:` view is
/// unbounded.
const DEFAULT_LIMIT: usize = 50;

/// Hit ceiling per `collect()` call — matches the fuzzy-match cutoff used
/// by other sources.
const MAX_RESULTS: usize = 200;

/// Reloads the shell history when it hasn't been refreshed for longer
/// than this. Keeps newly-run commands visible without re-reading every
/// keystroke.
const RELOAD_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(2);

struct Cache {
    entries: Vec<HistoryEntry>,
    loaded_at: Option<std::time::Instant>,
}

pub struct HistorySource {
    cache: Mutex<Cache>,
}

impl HistorySource {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(Cache {
                entries: Vec::new(),
                loaded_at: None,
            }),
        }
    }

    fn shell_name() -> String {
        std::env::var("SHELL")
            .ok()
            .and_then(|s| {
                s.rsplit('/').next().map(|name| match name {
                    "nu" | "nushell" => "nu".to_string(),
                    other => other.to_string(),
                })
            })
            .unwrap_or_else(|| "zsh".to_string())
    }

    fn ensure_loaded(&self) {
        let mut cache = match self.cache.lock() {
            Ok(c) => c,
            Err(_) => return,
        };
        let stale = cache
            .loaded_at
            .is_none_or(|t| t.elapsed() >= RELOAD_COOLDOWN);
        if stale {
            let history = CommandHistory::detect_and_load(&Self::shell_name());
            cache.entries = history.entries().to_vec();
            cache.loaded_at = Some(std::time::Instant::now());
        }
    }
}

impl Default for HistorySource {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchSource for HistorySource {
    fn category(&self) -> SearchCategory {
        SearchCategory::History
    }

    fn default_visible(&self) -> bool {
        true
    }

    fn collect(
        &self,
        _workspace: &Entity<Workspace>,
        query: &str,
        _window: &Window,
        cx: &mut App,
    ) -> Vec<SearchResult> {
        let limit = if query.is_empty() {
            DEFAULT_LIMIT
        } else {
            MAX_RESULTS
        };
        let lower = query.to_lowercase();

        // Prefer the in-memory history of the focused terminal pane
        // (commands typed in this session are visible immediately) and
        // fall back to the shell's on-disk history when no pane has
        // registered a handle yet — e.g. Cmd+R pressed before a terminal
        // was ever focused.
        if let Some(active) = ActiveCommandHistory::try_global(cx)
            && let Ok(history) = active.0.read()
        {
            let entries = history.entries();
            if !entries.is_empty() {
                return collect_from(entries, query, &lower, limit);
            }
        }

        self.ensure_loaded();
        let cache = match self.cache.lock() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        collect_from(&cache.entries, query, &lower, limit)
    }
}

fn collect_from(
    entries: &[HistoryEntry],
    query: &str,
    lower: &str,
    limit: usize,
) -> Vec<SearchResult> {
    if entries.is_empty() {
        return Vec::new();
    }
    let iter = entries.iter().rev();
    let filtered: Vec<&HistoryEntry> = if query.is_empty() {
        iter.take(limit).collect()
    } else {
        iter.filter(|e| e.command.to_lowercase().contains(lower))
            .take(limit)
            .collect()
    };
    filtered
        .into_iter()
        .enumerate()
        .map(|(ix, entry)| build_result(ix, entry))
        .collect()
}

fn build_result(ix: usize, entry: &HistoryEntry) -> SearchResult {
    let subtitle = if entry.timestamp > 0 {
        Some(relative_time(entry.timestamp).into())
    } else {
        None
    };
    SearchResult {
        id: format!("history:{ix}:{}", entry.command).into(),
        category: SearchCategory::History,
        title: SharedString::from(entry.command.clone()),
        subtitle,
        icon: SearchCategory::History.icon(),
        action: SearchAction::DispatchAction(Box::new(InsertIntoInput {
            text: entry.command.clone(),
        })),
    }
}
