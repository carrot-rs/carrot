//! `SearchableItem` impl — Cmd+F search over block scrollback.
//!
//! Extracted from the terminal_pane monolith so the search surface
//! is colocated with the `block_search` helpers it delegates to.
//! Data flow: the `Workspace` calls `find_matches` with a compiled
//! query → we build a per-block text cache → pass to the search
//! helper → return highlights to `BlockListView` via
//! `set_search_highlights`.

use std::sync::Arc;

use carrot_workspace::searchable::{
    Direction, SearchEvent, SearchOptions, SearchToken, SearchableItem,
};
use inazuma::{Context, EventEmitter, Task, Window, prelude::*};

use crate::terminal_pane::TerminalPane;

impl EventEmitter<SearchEvent> for TerminalPane {}

impl SearchableItem for TerminalPane {
    type Match = crate::block_search::BlockMatch;

    fn supported_options(&self) -> SearchOptions {
        SearchOptions {
            case: true,
            word: true,
            regex: true,
            replacement: false, // Terminal output is read-only
            selection: false,   // No selection-scoped search in terminal
            find_in_results: false,
        }
    }

    fn clear_matches(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.search_matches.clear();
        self.active_match_index = None;
        self.block_list.update(cx, |view, _cx| {
            view.set_search_highlights(Vec::new(), None);
        });
        cx.notify();
    }

    fn update_matches(
        &mut self,
        matches: &[Self::Match],
        active_match_index: Option<usize>,
        _token: SearchToken,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_matches = matches.to_vec();
        self.active_match_index = active_match_index;

        self.block_list.update(cx, |view, _cx| {
            view.set_search_highlights(matches.to_vec(), active_match_index);
        });
        cx.notify();
    }

    fn query_suggestion(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> String {
        String::new()
    }

    fn activate_match(
        &mut self,
        index: usize,
        matches: &[Self::Match],
        _token: SearchToken,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(mat) = matches.get(index) {
            self.active_match_index = Some(index);
            let block_index = mat.block_index;
            let match_line = mat.line;
            self.block_list.update(cx, |view, cx| {
                view.scroll_to_match(block_index, match_line, cx);
                view.set_search_highlights(matches.to_vec(), Some(index));
            });
            cx.emit(SearchEvent::ActiveMatchChanged);
            cx.notify();
        }
    }

    fn select_matches(
        &mut self,
        _matches: &[Self::Match],
        _token: SearchToken,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // Terminal output is read-only — select-all-matches is a no-op.
    }

    fn replace(
        &mut self,
        _: &Self::Match,
        _: &carrot_project::search::SearchQuery,
        _token: SearchToken,
        _window: &mut Window,
        _: &mut Context<Self>,
    ) {
        // Terminal output is read-only — replace is a no-op.
    }

    fn find_matches(
        &mut self,
        query: Arc<carrot_project::search::SearchQuery>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Vec<Self::Match>> {
        // Fast path: plain-text (non-regex) queries go through the
        // router's cell-level search (`BlockRouter::search`). It walks
        // the `PageList` once per block without allocating a copy of
        // the block's text — O(cells) vs O(cells × extract × regex).
        //
        // Regex / whole-word / replacement queries still need the
        // text-extracted + regex-automata path below.
        if let carrot_project::search::SearchQuery::Text {
            case_sensitive,
            whole_word: false,
            ..
        } = query.as_ref()
        {
            let needle = query.as_str().to_string();
            let case_sensitive = *case_sensitive;
            let router_results = {
                let handle = self.terminal.handle();
                let term = handle.lock();
                crate::block_search::find_via_router(&term, &needle, case_sensitive)
            };
            return cx.background_spawn(async move { router_results });
        }

        let handle = self.terminal.handle();
        let term = handle.lock();
        let entries = term.block_router().entries();

        let mut current_keys = std::collections::HashSet::new();
        let block_data: Vec<(usize, carrot_term::BlockId, String, String)> = entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let router_id = entry.id;
                let rows = entry.total_rows();
                let legacy_id = carrot_term::BlockId::from(router_id);
                let key = (legacy_id, rows);
                current_keys.insert(key);
                let (text, command) = self.search_text_cache.entry(key).or_insert_with(|| {
                    let text = crate::block_search::extract_entry_text(entry);
                    let command = entry.metadata.command.clone().unwrap_or_default();
                    (text, command)
                });
                (i, legacy_id, text.clone(), command.clone())
            })
            .collect();
        drop(term);

        self.search_text_cache
            .retain(|k, _| current_keys.contains(k));

        cx.background_spawn(async move {
            crate::block_search::find_matches_in_extracted_blocks(&block_data, &query)
        })
    }

    fn active_match_index(
        &mut self,
        _direction: Direction,
        matches: &[Self::Match],
        _token: SearchToken,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        if matches.is_empty() {
            return None;
        }
        self.active_match_index.or(Some(0))
    }
}
