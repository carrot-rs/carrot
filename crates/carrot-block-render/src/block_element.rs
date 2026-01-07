//! `inazuma::Element` wrapper — the surface that plugs carrot-grid
//! into the Inazuma render pass.
//!
//! # Scope
//!
//! Minimal text + background-quad rendering. The element takes a
//! [`RenderSnapshot`] (owned block data), a font + cell dimensions, and
//! emits per-cell draws via the existing `Window::paint_glyph` +
//! `Window::paint_quad` APIs.
//!
//! Out of scope for F.2 (later F.x phases):
//! - MSDF glyph atlas (F.4) — uses Inazuma's CoreText / DirectWrite /
//!   swash rasteriser for now.
//! - Emoji fast path.
//! - Selection / search highlights (consumer adds separately).
//! - Bold / italic / underline (decoration pass).
//! - Wide-char width stretching.
//! - Damage-driven incremental upload.
//!
//! # Ownership
//!
//! This first cut takes an owned [`RenderSnapshot`] because Inazuma's
//! `Element` trait requires the Self type to be `'static`. Callers
//! build the snapshot once per frame from their `ActiveBlock` /
//! `FrozenBlock` — the copy cost is one memcpy per visible row, which
//! is acceptable.

use inazuma::{
    App, Bounds, Element, Font, FontId, GlobalElementId, GlyphId, InspectorElementId, IntoElement,
    LayoutId, Oklch, Pixels, Point, Style, Window, fill, oklcha, point, px, relative, size,
};

use carrot_grid::{Cell, CellStyle, CellTag, PageList};

use crate::soft_wrap;

/// Owned per-frame snapshot of block data the element will render.
///
/// Built by the consumer immediately before constructing the element.
/// Keeps the [`Element`] impl `'static` without requiring the full
/// block to be `Arc`-wrapped (that optimisation lands in F.3).
#[derive(Clone)]
pub struct RenderSnapshot {
    /// Rows of cells in data order (first row at index 0). Each row
    /// should have the same length (= source block cols).
    pub rows: Vec<Vec<Cell>>,
    /// Cell-style atlas snapshot indexed by `CellStyleId.0 as usize`. Index 0
    /// must be the default style.
    pub atlas: Vec<CellStyle>,
}

impl RenderSnapshot {
    /// Empty snapshot — renders as a zero-height empty element.
    pub fn empty() -> Self {
        Self {
            rows: Vec::new(),
            atlas: vec![CellStyle::DEFAULT],
        }
    }

    /// Build a snapshot from already-owned row data. The common
    /// Phase-G caller (`carrot-terminal-view::block_list`) extracts
    /// rows via `block::RenderView` and has nothing to gain from
    /// re-cloning via `from_grid`. Kept generic on purpose —
    /// `carrot-block-render` cannot depend on `carrot-term`, so the
    /// constructor signature stays primitive.
    pub fn from_owned(rows: Vec<Vec<Cell>>, atlas: Vec<CellStyle>) -> Self {
        Self { rows, atlas }
    }

    /// Build a snapshot from the `carrot-grid` data model: walks every
    /// row in the page list and clones into a flat `Vec<Vec<Cell>>`.
    /// The atlas slice is copied once.
    ///
    /// Intended for consumers that hold an `Arc<FrozenBlock>` or a
    /// live `ActiveBlock` and want to feed [`BlockElement`] a single
    /// owned value per frame. Row count and cell width match the
    /// source `PageList` exactly — no soft-wrap here, that's the
    /// element's job.
    pub fn from_grid(pages: &PageList, atlas: &[CellStyle]) -> Self {
        let total = pages.total_rows();
        let mut rows = Vec::with_capacity(total);
        for row in pages.rows(0, total) {
            rows.push(row.to_vec());
        }
        Self {
            rows,
            atlas: atlas.to_vec(),
        }
    }

    fn style(&self, id: carrot_grid::CellStyleId) -> CellStyle {
        self.atlas
            .get(id.0 as usize)
            .copied()
            .unwrap_or(CellStyle::DEFAULT)
    }
}

/// Per-cell glyph resolved during prepaint, drawn during paint.
struct CellGlyph {
    origin: Point<Pixels>,
    font_id: FontId,
    glyph_id: GlyphId,
    color: Oklch,
}

/// Background rectangle in absolute pixel coordinates.
struct BgRect {
    bounds: Bounds<Pixels>,
    color: Oklch,
}

/// One search-match highlight over the block's grid. A row + column
/// span + an `active` flag for the currently-focused match. Consumers
/// pass a `Vec<SearchHighlight>` to
/// [`BlockElement::with_search_highlights`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchHighlight {
    /// Snapshot row index (0-based).
    pub row: usize,
    /// Snapshot column of the first matched cell.
    pub start_col: u16,
    /// Length of the match in cells (wide chars count as 2 cells).
    pub char_len: u16,
    /// `true` for the active (focused) match, `false` for regular
    /// hits. Renderers style the two differently.
    pub active: bool,
}

impl SearchHighlight {
    /// Whether the cell at `(row, col)` falls inside this highlight.
    pub fn contains(&self, row: usize, col: u16) -> bool {
        row == self.row && col >= self.start_col && col < self.start_col + self.char_len
    }
}

/// Selection range over the block's grid — `(row, col)` pairs into
/// the snapshot's `rows` buffer. Consumers translate their domain
/// selection (e.g. `carrot_term::selection::SelectionRange`) into
/// this shape before handing it to [`BlockElement::with_selection`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSelection {
    pub start_row: usize,
    pub start_col: u16,
    pub end_row: usize,
    pub end_col: u16,
    /// `true` for rectangular (column-locked) selection, `false` for
    /// linear "flowing" selection — matches the terminal convention.
    pub block: bool,
}

impl GridSelection {
    /// Whether the cell at `(row, col)` falls inside this selection.
    /// Rectangular selections require the column to fall inside
    /// `[start_col, end_col]` on every included row; linear
    /// selections start/end at column on the first/last row only.
    pub fn contains(&self, row: usize, col: u16) -> bool {
        if row < self.start_row || row > self.end_row {
            return false;
        }
        if self.block {
            return col >= self.start_col && col <= self.end_col;
        }
        // Linear: column bounds only pin the first and last rows.
        let col_ok_left = row != self.start_row || col >= self.start_col;
        let col_ok_right = row != self.end_row || col <= self.end_col;
        col_ok_left && col_ok_right
    }
}

/// Frame state cached between `prepaint` and `paint`.
pub struct BlockPrepaintState {
    glyphs: Vec<CellGlyph>,
    backgrounds: Vec<BgRect>,
    font_size: Pixels,
}

/// Inazuma element that renders one terminal block.
pub struct BlockElement {
    snapshot: RenderSnapshot,
    font: Font,
    font_size: Pixels,
    line_height_multiplier: f32,
    /// Terminal palette — resolves [`carrot_grid::Color`] tags carried
    /// by every styled cell to concrete Oklch values. Swapping the
    /// palette re-colours every scrollback cell on the next frame
    /// without touching stored data.
    palette: crate::palette::TerminalPalette,
    /// Default background colour — cells that match this bg skip the
    /// quad emission (fast path: most cells are on the default bg).
    terminal_bg: Oklch,
    /// Row range inside the snapshot to render.
    visible_rows: std::ops::Range<usize>,
    /// Target viewport cols for display-only soft-wrap.
    viewport_cols: u16,
    /// Active selection range, if any. Painted as a background quad
    /// on top of cell backgrounds, below glyphs.
    selection: Option<GridSelection>,
    /// Selection overlay color. Ignored when `selection == None`.
    selection_color: Oklch,
    /// Search-match highlights layered above cell bg + selection.
    search_highlights: Vec<SearchHighlight>,
    /// Search match color. Separate values for active vs. non-active
    /// keep the focused hit visually distinct without the consumer
    /// having to duplicate the list.
    search_match_color: Oklch,
    search_active_color: Oklch,
    /// Optional slot into which the element's actual paint-time
    /// origin Y is written during prepaint. Consumers install one per
    /// block so hit-testing can map mouse-y → data-row with zero
    /// sub-pixel drift (no re-derivation from layout).
    origin_store: Option<GridOriginStore>,
}

/// Shared cell into which [`BlockElement`] writes its actual paint-
/// time origin Y coordinate. Used by the owning list view for
/// pixel-accurate mouse-to-row hit testing — avoids recomputing
/// positions from the layout tree after the frame paints.
pub type GridOriginStore = std::rc::Rc<std::cell::Cell<Option<Pixels>>>;

impl BlockElement {
    pub fn new(
        snapshot: RenderSnapshot,
        font: Font,
        font_size: Pixels,
        line_height_multiplier: f32,
        palette: crate::palette::TerminalPalette,
        terminal_bg: Oklch,
        visible_rows: std::ops::Range<usize>,
        viewport_cols: u16,
    ) -> Self {
        Self {
            snapshot,
            font,
            font_size,
            line_height_multiplier,
            palette,
            terminal_bg,
            visible_rows,
            viewport_cols,
            selection: None,
            selection_color: oklcha(0.75, 0.15, 85.0, 0.45),
            search_highlights: Vec::new(),
            search_match_color: oklcha(0.75, 0.15, 85.0, 0.45),
            search_active_color: oklcha(0.80, 0.18, 55.0, 0.65),
            origin_store: None,
        }
    }

    /// Install a [`GridOriginStore`] the element writes into on every
    /// prepaint. Enables pixel-accurate hit testing in the owning
    /// view — the stored Y reflects the real painted origin, no
    /// re-derivation from the layout tree.
    pub fn with_origin_store(mut self, store: GridOriginStore) -> Self {
        self.origin_store = Some(store);
        self
    }

    /// Attach a selection overlay. The range is in snapshot-row /
    /// snapshot-col coordinates — consumers translate from their
    /// domain representation (terminal SelectionRange, editor
    /// buffer range, …) before calling.
    pub fn with_selection(mut self, selection: GridSelection, color: Oklch) -> Self {
        self.selection = Some(selection);
        self.selection_color = color;
        self
    }

    /// Attach search-match highlights. `match_color` is painted on
    /// every non-active hit; `active_color` on the focused match.
    /// Pass an empty slice to clear previously-set highlights.
    pub fn with_search_highlights(
        mut self,
        highlights: Vec<SearchHighlight>,
        match_color: Oklch,
        active_color: Oklch,
    ) -> Self {
        self.search_highlights = highlights;
        self.search_match_color = match_color;
        self.search_active_color = active_color;
        self
    }

    fn cell_dimensions(&self, window: &mut Window) -> (Pixels, Pixels, Pixels) {
        let font_id = window.text_system().resolve_font(&self.font);
        let advance = window.text_system().advance(font_id, self.font_size, 'm');
        let cell_width = advance.map(|m| m.width).unwrap_or(px(8.0));
        let ascent = window.text_system().ascent(font_id, self.font_size);
        let descent = window.text_system().descent(font_id, self.font_size);
        let cell_height = (ascent + descent.abs()) * self.line_height_multiplier;
        (cell_width, cell_height, ascent)
    }
}

impl Element for BlockElement {
    type RequestLayoutState = ();
    type PrepaintState = BlockPrepaintState;

    fn id(&self) -> Option<inazuma::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let (_cell_w, cell_h, _ascent) = self.cell_dimensions(window);
        let visible_row_count = self
            .visible_rows
            .end
            .saturating_sub(self.visible_rows.start);
        let total_height = cell_h * visible_row_count as f32;
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = total_height.into();
        let layout_id = window.request_layout(style, None, cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let (cell_w, cell_h, ascent) = self.cell_dimensions(window);
        let font_id = window.text_system().resolve_font(&self.font);
        let effective_cols = self.viewport_cols.max(1);

        // Stash the actual paint origin Y for the owning view's hit
        // tester. Writing it here (in prepaint) captures the
        // post-layout coordinate — no sub-pixel drift later.
        if let Some(ref store) = self.origin_store {
            store.set(Some(bounds.origin.y));
        }

        let mut glyphs = Vec::new();
        let mut backgrounds = Vec::new();

        if bounds.size.height <= px(0.0) {
            return BlockPrepaintState {
                glyphs,
                backgrounds,
                font_size: self.font_size,
            };
        }

        let end = self.visible_rows.end.min(self.snapshot.rows.len());
        let start = self.visible_rows.start.min(end);
        let rows_slice = &self.snapshot.rows[start..end];

        let mut visual_row: u32 = 0;
        for (row_offset, row) in rows_slice.iter().enumerate() {
            let data_row = start + row_offset;
            // Wide-char-aware segmentation — never splits a Wide2nd
            // pair across the wrap point. See `soft_wrap::segment`.
            let segments = soft_wrap::segment(row, effective_cols);
            for seg in &segments {
                let chunk = &row[seg.start..seg.end];
                for (i, &cell) in chunk.iter().enumerate() {
                    let col = i as u16;
                    let x = bounds.origin.x + cell_w * col as f32;
                    let y = bounds.origin.y + cell_h * visual_row as f32;
                    let style = self.snapshot.style(cell.style());
                    let bg = oklch_from_arr(
                        self.palette
                            .resolve(style.bg, crate::palette::DefaultSlot::Background),
                    );

                    // Background quad when not the terminal default.
                    if !same_color(bg, self.terminal_bg) {
                        backgrounds.push(BgRect {
                            bounds: Bounds::new(point(x, y), size(cell_w, cell_h)),
                            color: bg,
                        });
                    }

                    // Selection overlay — painted on top of the cell
                    // background, under the glyph. Data-row coords so
                    // soft-wrap doesn't confuse the selection range.
                    let data_col = seg.start as u16 + col;
                    if let Some(sel) = self.selection
                        && sel.contains(data_row, data_col)
                    {
                        backgrounds.push(BgRect {
                            bounds: Bounds::new(point(x, y), size(cell_w, cell_h)),
                            color: self.selection_color,
                        });
                    }

                    // Search highlights — linear scan, typically <10
                    // matches per block on screen so the cost is flat.
                    for hit in &self.search_highlights {
                        if hit.contains(data_row, data_col) {
                            let color = if hit.active {
                                self.search_active_color
                            } else {
                                self.search_match_color
                            };
                            backgrounds.push(BgRect {
                                bounds: Bounds::new(point(x, y), size(cell_w, cell_h)),
                                color,
                            });
                        }
                    }

                    // Resolve glyph for printable cells.
                    match cell.tag() {
                        CellTag::Ascii | CellTag::Codepoint => {
                            let c = char::from_u32(cell.content()).unwrap_or('?');
                            if c != ' '
                                && c != '\0'
                                && let Some(glyph_id) =
                                    window.text_system().glyph_for_char(font_id, c)
                            {
                                glyphs.push(CellGlyph {
                                    origin: point(x, y + ascent),
                                    font_id,
                                    glyph_id,
                                    color: oklch_from_arr(self.palette.resolve(
                                        style.fg,
                                        crate::palette::DefaultSlot::Foreground,
                                    )),
                                });
                            }
                        }
                        _ => {}
                    }
                }
                visual_row += 1;
            }
            // Empty data rows still occupy one visual row; soft_wrap
            // returns a single empty segment for them, which already
            // advanced `visual_row`. Nothing more to do here.
        }

        BlockPrepaintState {
            glyphs,
            backgrounds,
            font_size: self.font_size,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        for bg in &prepaint.backgrounds {
            window.paint_quad(fill(bg.bounds, bg.color));
        }
        for glyph in &prepaint.glyphs {
            let _ = window.paint_glyph(
                glyph.origin,
                glyph.font_id,
                glyph.glyph_id,
                prepaint.font_size,
                glyph.color,
            );
        }
    }
}

impl IntoElement for BlockElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

/// Convert an Oklch `[f32; 4]` quadruplet to an inazuma Oklch.
fn oklch_from_arr(c: [f32; 4]) -> Oklch {
    oklcha(c[0], c[1], c[2], c[3])
}

/// Approximate equality for colour comparison — carrot-grid styles
/// and theme colours should match exactly on the common default
/// path; a 1e-5 tolerance guards against f32 round-trip noise.
fn same_color(a: Oklch, b: Oklch) -> bool {
    const EPS: f32 = 1.0e-5;
    (a.l - b.l).abs() < EPS
        && (a.c - b.c).abs() < EPS
        && (a.h - b.h).abs() < EPS
        && (a.a - b.a).abs() < EPS
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrot_grid::{CellStyleAtlas, CellStyleId, PageCapacity};

    fn row(cols: u16, byte: u8) -> Vec<Cell> {
        (0..cols)
            .map(|_| Cell::ascii(byte, CellStyleId(0)))
            .collect()
    }

    #[test]
    fn empty_snapshot_has_one_atlas_entry_and_zero_rows() {
        let snap = RenderSnapshot::empty();
        assert_eq!(snap.rows.len(), 0);
        assert_eq!(snap.atlas.len(), 1);
        assert_eq!(snap.atlas[0], CellStyle::DEFAULT);
    }

    #[test]
    fn from_grid_copies_all_rows_and_atlas() {
        let cap = PageCapacity::new(4, 128);
        let mut pages = PageList::new(cap);
        for b in [b'a', b'b', b'c'] {
            pages.append_row(&row(4, b));
        }
        let atlas = CellStyleAtlas::new();
        let snap = RenderSnapshot::from_grid(&pages, atlas.as_slice());
        assert_eq!(snap.rows.len(), 3);
        assert_eq!(snap.rows[0][0].content(), b'a' as u32);
        assert_eq!(snap.rows[1][0].content(), b'b' as u32);
        assert_eq!(snap.rows[2][0].content(), b'c' as u32);
        // Atlas copied from the single-default-entry CellStyleAtlas.
        assert_eq!(snap.atlas.len(), 1);
    }

    #[test]
    fn from_grid_on_empty_page_list_returns_empty_rows() {
        let cap = PageCapacity::new(4, 128);
        let pages = PageList::new(cap);
        let atlas = CellStyleAtlas::new();
        let snap = RenderSnapshot::from_grid(&pages, atlas.as_slice());
        assert!(snap.rows.is_empty());
        assert_eq!(snap.atlas.len(), 1);
    }

    #[test]
    fn linear_selection_spans_rows_with_edge_pinning() {
        // Linear selection from row=2 col=3 to row=5 col=7.
        let sel = GridSelection {
            start_row: 2,
            start_col: 3,
            end_row: 5,
            end_col: 7,
            block: false,
        };
        // Start row: only cells at col >= 3 are inside.
        assert!(!sel.contains(2, 2));
        assert!(sel.contains(2, 3));
        assert!(sel.contains(2, 100));
        // Middle rows: every col is inside.
        assert!(sel.contains(3, 0));
        assert!(sel.contains(4, 50));
        // End row: only cells at col <= 7.
        assert!(sel.contains(5, 7));
        assert!(!sel.contains(5, 8));
        // Outside row range: never inside.
        assert!(!sel.contains(1, 3));
        assert!(!sel.contains(6, 0));
    }

    #[test]
    fn rectangular_selection_column_locked() {
        let sel = GridSelection {
            start_row: 2,
            start_col: 3,
            end_row: 5,
            end_col: 7,
            block: true,
        };
        // Inside the rectangle on every included row.
        assert!(sel.contains(2, 3));
        assert!(sel.contains(3, 5));
        assert!(sel.contains(5, 7));
        // Outside the column window on any row.
        assert!(!sel.contains(3, 2));
        assert!(!sel.contains(3, 8));
        // Outside the row window.
        assert!(!sel.contains(1, 5));
        assert!(!sel.contains(6, 5));
    }

    #[test]
    fn search_highlight_contains_is_half_open_on_the_right() {
        let hit = SearchHighlight {
            row: 3,
            start_col: 5,
            char_len: 4,
            active: false,
        };
        // Inclusive on the left, exclusive at start_col + char_len.
        assert!(!hit.contains(3, 4));
        assert!(hit.contains(3, 5));
        assert!(hit.contains(3, 6));
        assert!(hit.contains(3, 7));
        assert!(hit.contains(3, 8));
        assert!(!hit.contains(3, 9));
        // Different row never matches.
        assert!(!hit.contains(2, 6));
        assert!(!hit.contains(4, 6));
    }

    #[test]
    fn search_highlight_zero_length_matches_nothing() {
        let hit = SearchHighlight {
            row: 0,
            start_col: 0,
            char_len: 0,
            active: true,
        };
        assert!(!hit.contains(0, 0));
    }

    #[test]
    fn single_row_linear_selection_bounds_on_both_edges() {
        // When start_row == end_row, both edge-pin conditions apply.
        let sel = GridSelection {
            start_row: 4,
            start_col: 5,
            end_row: 4,
            end_col: 9,
            block: false,
        };
        assert!(!sel.contains(4, 4));
        assert!(sel.contains(4, 5));
        assert!(sel.contains(4, 9));
        assert!(!sel.contains(4, 10));
    }
}
