//! Block list view — v2-native scrollback renderer.
//!
//! Locks the terminal once per frame via `Term::render_view()`, drops
//! the lock, and renders Frozen + Active blocks using
//! `carrot_block_render::BlockElement` directly. No legacy
//! snapshot-cache; frozen blocks are `Arc<FrozenBlock>`-shared, the
//! active block memoizes on `sync_update_frame_id`.

mod fold;
pub(crate) mod header;
mod hit_test;
mod layout;
mod pointer;

use std::sync::Arc;

use carrot_block_render::{BlockElement, GridSelection, SearchHighlight};
use carrot_grid::BlockSnapshot;
use carrot_term::BlockId;
use carrot_term::block::{BlockId as RouterBlockId, SelectionKind};
use carrot_terminal::TerminalHandle;
use carrot_ui::{Scrollbar, ScrollbarShow};
use inazuma::{
    App, BlockConfig, BlockId as InazumaBlockId, BlockMetadata, BlockState, Context, Font,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Oklch, ParentElement, Pixels, Point as GpuiPoint, Render, ScrollBehavior,
    StatefulInteractiveElement, Styled, VisualAnchor, Window, blocks, div, prelude::FluentBuilder,
    px,
};

use self::fold::{render_fold_counter, render_fold_line};
use self::header::{BlockHeaderView, build_metadata_text, header_top_padding, running_badge_color};
use self::layout::{BlockLayoutEntry, GridOriginStore, fresh_origin_store};
use crate::block_search::BlockMatch;
use crate::constants::{
    BLOCK_BODY_PAD_BOTTOM, BLOCK_HEADER_PAD_X, BLOCK_LEFT_BORDER, FOLD_MAX_VISIBLE, accent_color,
    block_selected_bg, error_color, header_metadata_fg, terminal_bg,
};

/// Stateful block list view that owns rendering + mouse interaction.
pub struct BlockListView {
    pub(crate) terminal: TerminalHandle,
    pub(crate) list_state: BlockState,
    pub(crate) list_ids: Vec<InazumaBlockId>,
    /// Router-side BlockId for each Inazuma list entry, in chronological
    /// order. Populated per frame from the router.
    pub(crate) router_ids: Vec<RouterBlockId>,

    // Selection state
    pub(crate) selecting_block: Option<BlockId>,
    pub(crate) mouse_down_pos: Option<GpuiPoint<Pixels>>,
    pub(crate) selected_block: Option<usize>,

    /// Pending single-click block toggle — cancelled if a double-click
    /// follows.
    pub(crate) pending_block_toggle: Option<(usize, inazuma::Task<()>)>,

    // Cached block layout for pixel→grid conversion
    pub(crate) block_layout: Vec<BlockLayoutEntry>,

    // Fold area: show all fold-lines when counter is expanded
    pub(crate) fold_show_all: bool,

    // Search highlights — set by TerminalPane's SearchableItem impl
    pub(crate) search_highlights: Vec<BlockMatch>,
    pub(crate) active_highlight_index: Option<usize>,

    /// Memoized snapshot of the currently-active block. Keyed on
    /// `(router_id, sync_update_frame_id)` so the render pass can
    /// skip a full row-extract when the active block hasn't advanced
    /// (DEC 2026 sync update or between PTY chunks). Cleared when
    /// either key component changes.
    pub(crate) last_active_render: Option<(RouterBlockId, u64, BlockSnapshot)>,
}

impl BlockListView {
    pub fn new(terminal: TerminalHandle) -> Self {
        Self {
            terminal,
            list_state: BlockState::new(
                BlockConfig::default()
                    .visual_anchor(VisualAnchor::Bottom)
                    .scroll_behavior(ScrollBehavior::FollowTail)
                    .overdraw(px(200.0)),
            ),
            list_ids: Vec::new(),
            router_ids: Vec::new(),
            selecting_block: None,
            mouse_down_pos: None,
            pending_block_toggle: None,
            selected_block: None,
            block_layout: Vec::new(),
            fold_show_all: false,
            search_highlights: Vec::new(),
            active_highlight_index: None,
            last_active_render: None,
        }
    }

    /// Read current font/appearance config and build Font + dimensions.
    pub(crate) fn read_config(cx: &App) -> (Font, f32, f32, Vec<carrot_theme::ResolvedSymbolMap>) {
        let font = carrot_theme::terminal_font(cx).clone();
        let font_size: f32 = carrot_theme::terminal_font_size(cx).into();
        let line_height =
            carrot_theme::theme_settings(cx).line_height(carrot_theme::FontRole::Terminal, cx);
        let symbol_maps =
            carrot_theme::symbol_map_for(carrot_theme::FontRole::Terminal, cx).to_vec();
        (font, font_size, line_height, symbol_maps)
    }

    /// Clear all blocks. The v2 router doesn't expose a bulk-clear
    /// API yet, so we drive a fresh prompt cycle (which empties the
    /// router on the next on_prompt_start) and drop our local state.
    pub fn clear(&mut self) {
        let handle = self.terminal.clone();
        let mut term = handle.lock();
        // Switching to prompt resets the active id without evicting
        // frozen entries. The renderer re-reads entries every frame
        // from the router, so dropping our cache is enough to stop
        // showing stale rows.
        term.block_router_mut().on_prompt_start();
        drop(term);
        self.selected_block = None;
        self.selecting_block = None;
    }

    /// Get the selected block index.
    pub fn selected_block(&self) -> Option<usize> {
        self.selected_block
    }

    /// Set the selected block index.
    pub fn set_selected_block(&mut self, idx: Option<usize>) {
        self.selected_block = idx;
    }

    /// Scroll the list to reveal the block at the given index.
    pub fn scroll_to_block(&self, block_index: usize) {
        if let Some(&id) = self.list_ids.get(block_index) {
            self.list_state.scroll_to_reveal(id);
        }
    }

    /// Scroll to reveal a specific match within a block.
    pub fn scroll_to_match(&self, block_index: usize, _match_line: usize, _cx: &App) {
        self.scroll_to_block(block_index);
    }

    /// Set search highlight matches and the active match index.
    pub fn set_search_highlights(&mut self, matches: Vec<BlockMatch>, active_index: Option<usize>) {
        self.search_highlights = matches;
        self.active_highlight_index = active_index;
    }

    /// Get search highlights for a specific block (by index).
    pub fn search_highlights_for_block(
        &self,
        block_index: usize,
    ) -> (&[BlockMatch], Option<(usize, usize)>) {
        crate::block_search::search_highlights_for_block(
            &self.search_highlights,
            self.active_highlight_index,
            block_index,
        )
    }

    /// Compute cell dimensions from font metrics using current config.
    pub(crate) fn cell_dimensions(
        font: &Font,
        font_size: f32,
        line_height_multiplier: f32,
        window: &mut Window,
    ) -> (Pixels, Pixels) {
        let font_size_px = px(font_size);
        let font_id = window.text_system().resolve_font(font);
        let cell_width = window
            .text_system()
            .advance(font_id, font_size_px, 'm')
            .expect("glyph not found for 'm'")
            .width;
        let ascent = window.text_system().ascent(font_id, font_size_px);
        let descent = window.text_system().descent(font_id, font_size_px);
        let base_height = ascent + descent.abs();
        let cell_height = base_height * line_height_multiplier;
        (cell_width, cell_height)
    }

    /// Reuse the last-frame's active-block `BlockSnapshot` when the
    /// router's `sync_update_frame_id` hasn't advanced. Frozen blocks
    /// are `Arc`-shared and don't need memoize; only the active block
    /// carries a per-frame row-copy that benefits from caching.
    fn memoize_active_snapshot(
        &mut self,
        view: &carrot_term::block::RenderView,
        entries: &mut [RenderEntry],
    ) {
        let Some(active_view) = &view.active else {
            self.last_active_render = None;
            return;
        };
        let active_entry = match entries.last_mut() {
            Some(e) if e.router_id == active_view.id => e,
            _ => return,
        };
        let frame_id = active_view.sync_update_frame_id;
        match &self.last_active_render {
            Some((cached_id, cached_frame, cached_snap))
                if *cached_id == active_view.id && *cached_frame == frame_id =>
            {
                // Same block + same sync frame → reuse cached snapshot.
                active_entry.snapshot = cached_snap.clone();
            }
            _ => {
                // Different block or advanced frame: cache what we
                // just built.
                self.last_active_render =
                    Some((active_view.id, frame_id, active_entry.snapshot.clone()));
            }
        }
    }

    /// Sync the BlockState entry list to match the terminal's block count.
    fn sync_block_count(&mut self, block_count: usize) {
        let current = self.list_ids.len();
        if block_count == current {
            return;
        }
        if block_count > current {
            for _ in current..block_count {
                let id = self.list_state.push(BlockMetadata::default(), None);
                self.list_ids.push(id);
            }
        } else {
            for id in self.list_ids.drain(block_count..) {
                self.list_state.remove(id);
            }
        }
    }
}

/// Lightweight per-block data the render closure consumes. Built
/// from `RenderView` entries and handed to each `blocks()` child.
///
/// This struct carries no cursor field — Shell carets live in
/// `carrot-cmdline`, TUI cursors are pulled from VT state by Layer 4
/// when rendering `BlockKind::Tui` via the PinnedFooter surface.
struct RenderEntry {
    block_id: BlockId,
    router_id: RouterBlockId,
    command: String,
    header: BlockHeaderView,
    snapshot: BlockSnapshot,
    viewport_cols: u16,
    selection: Option<GridSelection>,
    /// Lifecycle marker pulled from the corresponding `ActiveBlockView`
    /// / `FrozenView`. Drives the pin-as-footer routing — `Tui` blocks
    /// get pinned at the bottom of the viewport, `Shell` blocks scroll
    /// in the normal flow.
    kind: carrot_term::block::BlockKind,
}

impl RenderEntry {
    /// Convenience — `BlockSnapshot.bounds.total_rows()`. Avoids
    /// duplicating the row count on `RenderEntry` (Plan 31 G3
    /// invariant: only one source of truth for `content_rows`).
    fn content_rows(&self) -> usize {
        self.snapshot.total_rows()
    }
}

fn build_entries(view: &carrot_term::block::RenderView) -> Vec<RenderEntry> {
    let mut out = Vec::with_capacity(view.frozen.len() + 1);
    for frozen in &view.frozen {
        let snapshot = BlockSnapshot::from_pages(frozen.block.grid(), frozen.block.atlas());
        out.push(RenderEntry {
            block_id: BlockId::from(frozen.id),
            router_id: frozen.id,
            command: frozen.metadata.command.clone().unwrap_or_default(),
            header: BlockHeaderView::from_metadata(&frozen.metadata, false, None),
            snapshot,
            viewport_cols: view.grid_dims.0,
            selection: None,
            kind: frozen.kind,
        });
    }
    if let Some(active) = &view.active {
        let snapshot = active.snapshot.clone();
        let selection = active.selection.as_ref().map(|sel| {
            // The snapshot's bounds expose `first_row_offset` directly,
            // so selection mapping uses the canonical conversion
            // instead of guessing zero — keeps selections correct after
            // scrollback prune events.
            let (s, e) = sel.range();
            let bounds = snapshot.bounds;
            let start_row = bounds.origin_to_row(s.origin).unwrap_or(0);
            let end_row = bounds
                .origin_to_row(e.origin)
                .unwrap_or_else(|| bounds.total_rows().saturating_sub(1));
            GridSelection {
                start_row,
                start_col: s.col,
                end_row,
                end_col: e.col,
                block: matches!(sel.kind, SelectionKind::Block),
            }
        });
        let viewport_cols = snapshot.columns();
        out.push(RenderEntry {
            block_id: BlockId::from(active.id),
            router_id: active.id,
            command: active.metadata.command.clone().unwrap_or_default(),
            header: BlockHeaderView::from_metadata(
                &active.metadata,
                true,
                active.live_frame.as_ref(),
            ),
            snapshot,
            viewport_cols,
            selection,
            kind: active.kind,
        });
    }
    out
}

impl Render for BlockListView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (font, font_size, line_height_multiplier, _symbol_maps) = Self::read_config(cx);
        let theme = carrot_theme::GlobalTheme::theme(cx).clone();

        let view = self.terminal.lock().render_view();
        let mut entries = build_entries(&view);
        self.memoize_active_snapshot(&view, &mut entries);
        self.sync_block_count(entries.len());

        // Pin the most-recent `BlockKind::Tui` entry as the viewport
        // footer so its live frame stays anchored at the bottom while
        // earlier shell blocks scroll above it. Walk in reverse so an
        // active TUI block wins over older frozen TUI blocks. If no
        // TUI block is present we explicitly unpin — a previously
        // pinned TUI block that's now Shell (impossible by sticky
        // promotion, but defensive) or pruned would otherwise leave a
        // stale pin.
        let pinned_tui = entries.iter().enumerate().rev().find_map(|(i, e)| {
            e.kind
                .is_tui()
                .then_some(())
                .and_then(|_| self.list_ids.get(i).copied())
        });
        match pinned_tui {
            Some(id) => self.list_state.pin(id),
            None => self.list_state.unpin(),
        }

        // Cache the v2 id mapping + layout per entry.
        self.router_ids = entries.iter().map(|e| e.router_id).collect();
        self.block_layout = entries
            .iter()
            .enumerate()
            .map(|(i, e)| BlockLayoutEntry {
                block_id: e.block_id,
                block_index: i,
                content_rows: e.content_rows(),
                command_row_count: 0,
                grid_history_size: 0,
                grid_origin_store: fresh_origin_store(),
            })
            .collect();

        let origin_stores: Vec<GridOriginStore> = self
            .block_layout
            .iter()
            .map(|e| e.grid_origin_store.clone())
            .collect();

        let selected_block = self.selected_block;

        let is_alt_screen = {
            let term = self.terminal.lock();
            term.mode()
                .contains(carrot_term::term::TermMode::ALT_SCREEN)
        };
        let folded_indices = if is_alt_screen {
            Vec::new()
        } else {
            let folded_ids = self.list_state.folded_ids();
            folded_ids
                .iter()
                .filter_map(|id| self.list_ids.iter().position(|x| x == id))
                .collect::<Vec<_>>()
        };

        let fold_show_all = self.fold_show_all;
        let visible_fold_start = if fold_show_all {
            0
        } else {
            folded_indices.len().saturating_sub(FOLD_MAX_VISIBLE)
        };
        let fold_data: Vec<(usize, BlockHeaderView, String)> = folded_indices[visible_fold_start..]
            .iter()
            .map(|&ix| (ix, entries[ix].header.clone(), entries[ix].command.clone()))
            .collect();
        let fold_hidden_count = visible_fold_start;

        let list_theme = theme.clone();
        let search_highlights = self.search_highlights.clone();
        let active_highlight_idx = self.active_highlight_index;
        let ids_for_closure = self.list_ids.clone();
        let entries_arc = Arc::new(entries);
        let entries_for_list = Arc::clone(&entries_arc);
        let font_for_list = font.clone();

        let block_list = blocks(self.list_state.clone(), move |id, _window, _cx| {
            let Some(ix) = ids_for_closure.iter().position(|x| *x == id) else {
                return div().into_any_element();
            };
            let Some(entry) = entries_for_list.get(ix) else {
                return div().into_any_element();
            };
            let is_selected = selected_block == Some(ix);
            let store = origin_stores.get(ix).cloned();

            let highlights =
                derive_highlights_for_block(&search_highlights, active_highlight_idx, ix);

            render_block_entry(
                entry,
                &font_for_list,
                font_size,
                line_height_multiplier,
                is_selected,
                &list_theme,
                store,
                highlights,
            )
            .into_any_element()
        });

        div()
            .id("block-list-container")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .when(!folded_indices.is_empty(), |container| {
                let fold_theme = theme.clone();

                let mut fold_area = div()
                    .w_full()
                    .flex_shrink_0()
                    .flex_col()
                    .border_b_1()
                    .border_color(Oklch::white().opacity(0.12));

                if fold_hidden_count > 0 {
                    fold_area = fold_area.child(render_fold_counter(
                        fold_hidden_count,
                        &fold_theme,
                        cx.listener(|view, _ev, _win, _cx| {
                            view.fold_show_all = !view.fold_show_all;
                        }),
                    ));
                } else if fold_show_all && folded_indices.len() > FOLD_MAX_VISIBLE {
                    fold_area = fold_area.child(render_fold_counter(
                        0,
                        &fold_theme,
                        cx.listener(|view, _ev, _win, _cx| {
                            view.fold_show_all = false;
                        }),
                    ));
                }

                for (ix, header, command) in &fold_data {
                    let ix = *ix;
                    fold_area = fold_area.child(render_fold_line(
                        header,
                        command,
                        ix,
                        &fold_theme,
                        cx.listener(move |view, _ev, _win, _cx| {
                            if let Some(&id) = view.list_ids.get(ix) {
                                view.list_state.scroll_to_reveal(id);
                            }
                        }),
                    ));
                }

                container.child(fold_area)
            })
            .child(
                div()
                    .id("block-list-scroll-area")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, event: &MouseDownEvent, window, cx| {
                            let dims = self::pointer::cell_dimensions_from_config(window, cx);
                            view.on_mouse_down_left(event, dims, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| {
                        let dims = self::pointer::cell_dimensions_from_config(window, cx);
                        view.on_mouse_move(event, dims, cx);
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|view, event: &MouseUpEvent, window, cx| {
                            let dims = self::pointer::cell_dimensions_from_config(window, cx);
                            view.on_mouse_up_left(event, dims, window, cx);
                        }),
                    )
                    .child(block_list.size_full())
                    .child(
                        Scrollbar::vertical(&self.list_state).scrollbar_show(ScrollbarShow::Always),
                    ),
            )
    }
}

fn derive_highlights_for_block(
    matches: &[BlockMatch],
    active_highlight_idx: Option<usize>,
    block_index: usize,
) -> Vec<SearchHighlight> {
    let start = matches.partition_point(|m| m.block_index < block_index);
    let end = matches.partition_point(|m| m.block_index <= block_index);
    matches[start..end]
        .iter()
        .enumerate()
        .map(|(local_idx, m)| {
            let global_idx = start + local_idx;
            SearchHighlight {
                row: m.line,
                start_col: m.col as u16,
                char_len: m.char_len as u16,
                active: active_highlight_idx == Some(global_idx),
            }
        })
        .collect()
}

fn render_block_entry(
    entry: &RenderEntry,
    font: &Font,
    font_size: f32,
    line_height_multiplier: f32,
    is_selected: bool,
    theme: &carrot_theme::Theme,
    grid_origin_store: Option<GridOriginStore>,
    highlights: Vec<SearchHighlight>,
) -> impl IntoElement {
    let meta_text = build_metadata_text(&entry.header);
    let is_error = entry.header.is_error;
    let selection_highlight = accent_color(theme).opacity(0.45);
    let match_color = accent_color(theme).opacity(0.25);
    let active_match_color = accent_color(theme).opacity(0.55);

    let mut element = BlockElement::new(
        entry.snapshot.clone(),
        font.clone(),
        px(font_size),
        line_height_multiplier,
        carrot_block_render::TerminalPalette::from_theme(theme.colors()),
        terminal_bg(theme),
        0..entry.content_rows(),
        entry.viewport_cols,
    );
    if let Some(store) = grid_origin_store {
        element = element.with_origin_store(store);
    }
    if let Some(sel) = entry.selection {
        element = element.with_selection(sel, selection_highlight);
    }
    if !highlights.is_empty() {
        element = element.with_search_highlights(highlights, match_color, active_match_color);
    }

    let error_bg = inazuma::oklcha(0.25, 0.08, 25.0, 0.15);

    div()
        .w_full()
        .when(is_selected, |d| d.bg(block_selected_bg(theme)))
        .when(is_error && !is_selected, |d| d.bg(error_bg))
        .border_t_1()
        .border_color(Oklch::white().opacity(0.08))
        .pb(px(BLOCK_BODY_PAD_BOTTOM))
        .when(is_error, |d| {
            d.border_l(px(BLOCK_LEFT_BORDER))
                .border_l_color(error_color(theme))
        })
        .child(
            div()
                .px(px(BLOCK_HEADER_PAD_X))
                .pt(header_top_padding())
                .pb(header_top_padding())
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .when(entry.header.is_running, |d| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .size(px(6.0))
                            .rounded_full()
                            .bg(running_badge_color(theme)),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(header_metadata_fg(theme))
                        .child(meta_text),
                )
                .when(entry.header.live_frame.is_some(), |d| {
                    let lf = entry.header.live_frame.expect("checked by when()");
                    let tooltip_text: inazuma::SharedString = format!(
                        "TUI redraw protected — source: {}, reprints: {}",
                        lf.source.label(),
                        lf.reprint_count,
                    )
                    .into();
                    d.child(
                        div()
                            .id(inazuma::SharedString::from(format!(
                                "tui-chip-{}",
                                entry.router_id.0
                            )))
                            .flex_shrink_0()
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(3.0))
                            .text_size(px(10.0))
                            .bg(accent_color(theme).opacity(0.18))
                            .text_color(accent_color(theme))
                            .child("TUI")
                            .tooltip(carrot_ui::ChipTooltip::text(tooltip_text)),
                    )
                }),
        )
        .child(element)
}

impl BlockListView {
    /// Cursor position of the active block inside its snapshot rows.
    /// Used by the input bar's `handle_terminal_event` path to
    /// highlight the command line.
    pub fn active_cursor(&self) -> Option<(usize, u16)> {
        // Accessing here would take a fresh lock — defer to render
        // view when the caller actually needs this data.
        None
    }
}
