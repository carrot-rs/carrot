//! Carrot Block Render — Layer 4 of the Ultimate Block System.
//!
//! Reads directly from [`carrot_grid::PageList`] / [`carrot_grid::CellStyleAtlas`]
//! (no intermediate snapshot) and produces draw commands for an inazuma
//! render pass. Given a page list + visible row range + viewport cols,
//! emits a `Vec<CellDraw>` ready for GPU upload, with wide-char-aware
//! soft-wrap, damage-aware incremental draw, image compositing, decoration
//! overlays, cursor primitives, LRU shape cache, and an arc-swap shared
//! snapshot for lock-free render reads.
//!
//! The MSDF glyph atlas and the wgpu image texture upload still lift
//! the `Vec<CellDraw>` into a direct `wgpu::Buffer` vertex stream;
//! everything else (damage, soft-wrap, shaping, images, cursor, decoration)
//! is production-complete and covered by the hero end-to-end test and
//! per-concern criterion benches.

pub mod block_element;
pub mod cursor;
pub mod damage;
pub mod decoration;
pub mod diff;
pub mod frame;
pub mod image_pass;
pub mod image_upload;
pub mod msdf_atlas;
pub mod msdf_rasteriser;
pub mod msdf_upload;
pub mod palette;
pub mod shaders;
pub mod shape_cache;
pub mod shaping;
pub mod snapshot;
pub mod soft_wrap;

pub use palette::{DefaultSlot, TerminalPalette};

pub use block_element::{
    BlockElement, BlockPrepaintState, GridOriginStore, GridSelection, RenderSnapshot,
    SearchHighlight,
};
pub use cursor::{CursorDraw, CursorShape, CursorState, render_cursor};
pub use damage::{CellSignature, Damage, FrameState, compute_damage};
pub use decoration::{
    AnimationFlags, DecorationDraw, DecorationKind, FontVariantSelector, apply_reverse_video,
    render_decorations,
};
pub use diff::{DiffEntry, GridDiff, diff_grids};
pub use frame::{Frame, FrameInput, PositionedDecoration, build_frame};
pub use image_pass::{ImageDraw, filter_visible, render_images};
pub use image_upload::{GpuImage, ImageGpuCache, upload_new as upload_new_images};
pub use msdf_atlas::{DirtyRect, GlyphKey, MsdfAtlas, MsdfGlyph, MsdfInsertError};
pub use msdf_rasteriser::{RasterisedGlyph, rasterise_msdf};
pub use msdf_upload::{MsdfGpuAtlas, rgb_to_rgba};
pub use shaders::{MSDF_GLYPH_FRAGMENT, SCROLLBACK_SEARCH_COMPUTE};
pub use shape_cache::{CacheOutcome, ShapeCache, ShapeKey};
pub use shaping::{ShapeOptions, ShapedGlyph, ShapingError, ShapingFont, shape_run};
pub use snapshot::{ActiveBlockSnapshot, FrozenBlockHandle, SharedTerminal, TerminalSnapshot};
pub use soft_wrap::{VisualSegment, data_to_visual, segment as soft_wrap_segment};

use carrot_grid::{CellStyle, CellStyleAtlas, CellTag, PageList};

/// One draw command per cell. Flat struct in CPU-visible memory —
/// the downstream upload stages pack this into a GPU vertex format,
/// but the emit shape stays the same on the producer side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellDraw {
    /// Visual row (after display-only soft-wrap — not a data row).
    pub visual_row: u32,
    /// Visual column.
    pub visual_col: u16,
    /// Raw content word. Interpret with `tag` to decide the rendering path.
    pub content: u32,
    pub tag: CellTag,
    /// Resolved style copied from the atlas so the renderer doesn't need to
    /// look it up a second time per frame.
    pub style: CellStyle,
}

/// What the renderer needs to produce a frame for one block.
pub struct BlockRenderInput<'a> {
    pub pages: &'a PageList,
    pub atlas: &'a CellStyleAtlas,
    pub visible_rows: std::ops::Range<usize>,
    pub viewport_cols: u16,
}

/// Core entry point. Returns per-cell draw commands for the visible rows.
///
/// Algorithm:
/// 1. Walk [`PageList::rows`] over the visible range — O(visible).
/// 2. For each row, apply **display-only soft-wrap** via
///    [`soft_wrap::segment`]: wide-character pairs never split across a
///    wrap boundary. Data is never mutated.
/// 3. For each cell, resolve the style via [`CellStyleAtlas::get`] and emit a
///    [`CellDraw`] with its visual coordinate.
///
/// Wide-character cells emit both halves; [`CellTag::Wide2nd`] carries no
/// content and the GPU renderer stretches the first half across both
/// columns.
pub fn render_block(input: BlockRenderInput<'_>) -> Vec<CellDraw> {
    let (draws, _signatures, _rows) = render_block_with_signatures(input);
    draws
}

/// Result of a damage-aware render pass. Separates the draws the
/// renderer should emit from the signature matrix the caller stores as
/// the next frame's [`FrameState`].
pub struct DamagedRender {
    /// Cells that need redraw — filtered through `damage`.
    pub draws: Vec<CellDraw>,
    /// Signatures of **all** visual cells (even unchanged ones) so the
    /// caller can record them as the new `FrameState`.
    pub signatures: Vec<CellSignature>,
    /// Visual row count actually produced.
    pub visual_rows: u32,
    /// Damage that was applied.
    pub damage: Damage,
}

/// Damage-aware render. Compares the new frame against `prev` and
/// returns only the [`CellDraw`] commands for changed cells, plus the
/// full signature matrix for the caller to stash as the next frame's
/// prev state.
///
/// `prev.is_empty()` triggers `Damage::Full`, emitting every cell.
/// Use this for the first frame or whenever the viewport has resized.
pub fn render_block_damaged<'a>(input: BlockRenderInput<'a>, prev: &FrameState) -> DamagedRender {
    let (all_draws, signatures, visual_rows) = render_block_with_signatures(input.into_ref());
    let viewport_cols = input.viewport_cols.max(1);
    let damage = compute_damage(prev, &signatures, visual_rows, viewport_cols);

    let draws = match &damage {
        Damage::Full => all_draws,
        Damage::Partial { .. } => all_draws
            .into_iter()
            .filter(|d| damage.contains(d.visual_row, d.visual_col))
            .collect(),
    };

    DamagedRender {
        draws,
        signatures,
        visual_rows,
        damage,
    }
}

/// Core rendering pass. Always emits every visible cell; the damage
/// filter is applied by [`render_block_damaged`] afterwards. Returned
/// tuple: `(draws, signatures, visual_rows)` so callers building their
/// own damage pipeline can reuse the signature matrix.
pub fn render_block_with_signatures(
    input: BlockRenderInput<'_>,
) -> (Vec<CellDraw>, Vec<CellSignature>, u32) {
    let BlockRenderInput {
        pages,
        atlas,
        visible_rows,
        viewport_cols,
    } = input;

    let bounds = carrot_grid::GridBounds::from_pages(pages);
    let data_cols = bounds.columns();
    let effective_cols = viewport_cols.max(1);

    let mut draws = Vec::new();
    let mut signatures = Vec::new();
    let mut visual_row: u32 = 0;

    for (_, row) in bounds.iter_range(pages, visible_rows) {
        // Wide-char-aware segmentation: never splits a Wide1st/Wide2nd
        // pair across a visual-row boundary. See `soft_wrap::segment`.
        let segments = soft_wrap::segment(row, effective_cols);
        for seg in &segments {
            let chunk = &row[seg.start..seg.end];
            for (i, &cell) in chunk.iter().enumerate() {
                draws.push(CellDraw {
                    visual_row,
                    visual_col: i as u16,
                    content: cell.content(),
                    tag: cell.tag(),
                    style: atlas.get(cell.style()),
                });
                signatures.push(CellSignature::from_cell(cell));
            }
            // Pad the signature row out to `effective_cols` so the
            // matrix stays rectangular for damage comparison.
            for _ in chunk.len()..(effective_cols as usize) {
                signatures.push(CellSignature::default());
            }
            visual_row += 1;
        }
        if row.is_empty() || (data_cols as usize) == 0 {
            // `segment` returned a single empty segment for empty
            // rows; skip adding a second empty visual row.
            if !segments.is_empty() && segments[0].is_empty() {
                continue;
            }
            for _ in 0..(effective_cols as usize) {
                signatures.push(CellSignature::default());
            }
            visual_row += 1;
        }
    }

    (draws, signatures, visual_row)
}

impl<'a> BlockRenderInput<'a> {
    /// Produce a same-shape re-borrow of `self`. Cheap — all fields
    /// are already references or `Copy`. Used so call sites can share
    /// the input between the signature pass and damage filter.
    fn into_ref(&self) -> BlockRenderInput<'_> {
        BlockRenderInput {
            pages: self.pages,
            atlas: self.atlas,
            visible_rows: self.visible_rows.clone(),
            viewport_cols: self.viewport_cols,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrot_grid::{Cell, CellStyleAtlas, CellStyleId, PageCapacity};

    fn make_list(cols: u16, rows: u16) -> (PageList, CellStyleAtlas) {
        let cap = PageCapacity::new(cols, 1024);
        let mut list = PageList::new(cap);
        let atlas = CellStyleAtlas::new();
        for r in 0..rows {
            let row: Vec<Cell> = (0..cols)
                .map(|c| Cell::ascii(b'a' + ((r as u8 + c as u8) % 26), CellStyleId(0)))
                .collect();
            list.append_row(&row);
        }
        (list, atlas)
    }

    #[test]
    fn visible_range_only_rows_get_drawn() {
        let (list, atlas) = make_list(4, 10);
        let draws = render_block(BlockRenderInput {
            pages: &list,
            atlas: &atlas,
            visible_rows: 2..5,
            viewport_cols: 4,
        });
        // 3 rows × 4 cols = 12 draws
        assert_eq!(draws.len(), 12);
        // First emitted cell is at visual row 0 — rows are indexed from
        // the visible range start, not absolute.
        assert_eq!(draws[0].visual_row, 0);
        assert_eq!(draws[4].visual_row, 1);
    }

    #[test]
    fn soft_wrap_splits_wide_row_visually() {
        // 8-col data row viewed at 4-col viewport → 2 visual rows per data row.
        let (list, atlas) = make_list(8, 3);
        let draws = render_block(BlockRenderInput {
            pages: &list,
            atlas: &atlas,
            visible_rows: 0..3,
            viewport_cols: 4,
        });
        // 3 data rows × 8 cols = 24 draws across 6 visual rows
        assert_eq!(draws.len(), 24);
        let visual_rows: std::collections::BTreeSet<u32> =
            draws.iter().map(|d| d.visual_row).collect();
        assert_eq!(visual_rows.len(), 6);
    }

    #[test]
    fn narrow_viewport_wide_cols_no_panic_with_viewport_zero() {
        let (list, atlas) = make_list(2, 2);
        // Caller passed 0; we clamp to 1.
        let draws = render_block(BlockRenderInput {
            pages: &list,
            atlas: &atlas,
            visible_rows: 0..2,
            viewport_cols: 0,
        });
        // 2 data rows × 2 cols, shown 1 per line → 4 visual rows.
        assert_eq!(draws.len(), 4);
    }
}
