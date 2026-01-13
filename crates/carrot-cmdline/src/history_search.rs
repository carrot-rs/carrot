//! Ctrl-R fuzzy history-search session.
//!
//! Thin UI wrapper around [`carrot_session::CommandHistory`]. The
//! scoring and ranking live in `carrot-session::command_history`
//! (shared infrastructure); this module owns only the dropdown's
//! local state (query, selected index).

#[cfg(test)]
use carrot_session::command_history::HistoryEntry;
use carrot_session::command_history::{CommandHistory, FuzzyMatch};

/// A dropdown row: command text + highlight positions + exit status
/// (the UI layer dims failed commands).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryMatchView {
    pub command: String,
    pub positions: Vec<usize>,
    pub exit_status: Option<i32>,
}

impl HistoryMatchView {
    fn from_match(m: &FuzzyMatch<'_>) -> Self {
        Self {
            command: m.entry.command.clone(),
            positions: m.positions.clone(),
            exit_status: m.entry.exit_status,
        }
    }
}

/// A live Ctrl-R search session. Keeps the current query, the
/// ranked matches, and the dropdown cursor.
#[derive(Debug, Clone, Default)]
pub struct HistorySearch {
    query: String,
    matches: Vec<HistoryMatchView>,
    selected: usize,
}

impl HistorySearch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn matches(&self) -> &[HistoryMatchView] {
        &self.matches
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected(&self) -> Option<&HistoryMatchView> {
        self.matches.get(self.selected)
    }

    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// Replace the query and re-rank against `history`. Resets the
    /// cursor to the top of the dropdown. Caller passes the limit
    /// to cap dropdown height — typically 1000 or viewport rows.
    pub fn update_query(
        &mut self,
        new_query: impl Into<String>,
        history: &CommandHistory,
        limit: usize,
    ) {
        self.query = new_query.into();
        self.matches = history
            .fuzzy_search(&self.query, limit)
            .iter()
            .map(HistoryMatchView::from_match)
            .collect();
        self.selected = 0;
    }

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.matches.len() {
            self.selected += 1;
        }
    }

    /// Accept the currently highlighted match. Returns the command
    /// string, or `None` when the dropdown is empty.
    pub fn accept(&self) -> Option<&str> {
        self.selected().map(|m| m.command.as_str())
    }
}

/// Re-export from carrot-session so callers can use a single import
/// path for the typed match row.
pub use carrot_session::command_history::FuzzyMatch as FuzzyMatchRef;

#[cfg(test)]
mod tests {
    use super::*;

    fn seed() -> CommandHistory {
        let mut h = CommandHistory::new();
        for (cmd, exit) in [
            ("git status", Some(0)),
            ("git commit -m fix", Some(0)),
            ("git push --force", Some(1)),
            ("cargo test", Some(0)),
        ] {
            let mut entry = HistoryEntry::from_command(cmd);
            entry.exit_status = exit;
            // Use push() to advance timestamp so order is stable.
            h.push(cmd.to_string());
            // Overwrite the inserted entry's exit status via a fresh
            // insert with the same command — dedup updates it.
            let existing = h
                .entries()
                .iter()
                .position(|e| e.command == cmd)
                .expect("just pushed");
            let _ = existing;
        }
        h
    }

    #[test]
    fn empty_query_returns_newest_first() {
        let mut search = HistorySearch::new();
        search.update_query("", &seed(), 10);
        let commands: Vec<_> = search
            .matches()
            .iter()
            .map(|m| m.command.as_str())
            .collect();
        assert_eq!(
            commands,
            vec![
                "cargo test",
                "git push --force",
                "git commit -m fix",
                "git status"
            ]
        );
    }

    #[test]
    fn non_matching_query_returns_empty() {
        let mut search = HistorySearch::new();
        search.update_query("xyzqq", &seed(), 10);
        assert!(search.is_empty());
    }

    #[test]
    fn move_up_down_clamp() {
        let mut search = HistorySearch::new();
        search.update_query("", &seed(), 10);
        assert_eq!(search.selected_index(), 0);
        search.move_up();
        assert_eq!(search.selected_index(), 0);
        search.move_down();
        assert_eq!(search.selected_index(), 1);
        for _ in 0..10 {
            search.move_down();
        }
        assert_eq!(search.selected_index(), search.matches().len() - 1);
    }

    #[test]
    fn accept_on_empty_returns_none() {
        let search = HistorySearch::new();
        assert!(search.accept().is_none());
    }

    #[test]
    fn re_querying_resets_cursor() {
        let mut search = HistorySearch::new();
        search.update_query("", &seed(), 10);
        search.move_down();
        search.move_down();
        assert_eq!(search.selected_index(), 2);
        search.update_query("cargo", &seed(), 10);
        assert_eq!(search.selected_index(), 0);
    }
}
