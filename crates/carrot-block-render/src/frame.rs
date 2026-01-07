//! Unified frame composition.
//!
//! Glues together the rendering primitives — `render_block_damaged`
//! for cells, `render_cursor` for the cursor, `render_decorations`
//! for per-cell underlines / strikethroughs — into one high-level
//! API that Layer 5 consumers can call once per frame.
//!
//! The primitives each solve one thing well. This module solves
//! the **composition** problem: how do the consumer's draws get
//! ordered, how does damage interact with cursor + decorations,
//! where does the caller put the frame state across paints.
//!
//! # Draw order
//!
//! The frame's draw list is emitted in a fixed z-order so the
//! consumer doesn't have to reason about compositing:
//!
//! 1. **Backgrounds** — per-cell bg quads.
//! 2. **Cell glyphs** — foreground characters.
//! 3. **Decorations** — underlines, strikethroughs (drawn on top of
//!    glyphs because VT conventions expect them over the character).
//! 4. **Cursor** — drawn last so it's always visible.
//!
//! # Damage
//!
//! Cell damage filters the glyph + bg work per the existing
//! `render_block_damaged`. Cursor is **always emitted** when
//! visible (it blinks — always-dirty from the GPU's perspective).
//! Decorations follow their host cell: if the cell is dirty, its
//! decorations are emitted; if clean, they're skipped.

use std::ops::Range;

use carrot_grid::{CellStyleAtlas, CellStyleFlags, ImageStore, PageList};

use crate::cursor::{CursorDraw, CursorState, render_cursor};
use crate::damage::{CellSignature, Damage, FrameState, compute_damage};
use crate::decoration::{DecorationDraw, DecorationKind, apply_reverse_video, render_decorations};
use crate::image_pass::{ImageDraw, render_images};
use crate::{BlockRenderInput, CellDraw, render_block_with_signatures};

/// Everything the consumer needs to paint a frame, minus platform-
/// specific GPU handles. Every field is a plain owned Vec — the
/// consumer consumes it directly.
#[derive(Debug)]
pub struct Frame {
    /// Cell draws to emit — filtered by damage.
    pub cells: Vec<CellDraw>,
    /// Per-cell decoration rects (underlines, strikethroughs) —
    /// aligned with cells in the same frame.
    pub decorations: Vec<PositionedDecoration>,
    /// Image draws (terminal-inline image protocols), composited
    /// after the text pass and before the cursor. Empty when the
    /// caller passes no `images` input.
    pub images: Vec<ImageDraw>,
    /// Cursor rect, or `None` if hidden / in off-blink phase.
    pub cursor: Option<CursorDraw>,
    /// The signature matrix for this frame — the caller stores this
    /// as `prev` for the next call to `build_frame`.
    pub signatures: Vec<CellSignature>,
    /// Number of visible rows actually produced (after soft-wrap).
    pub visual_rows: u32,
    /// Damage result, for consumers that want to log or throttle
    /// based on frame change volume.
    pub damage: Damage,
}

/// A decoration plus the cell it attaches to, so the consumer can
/// compute absolute pixel coords without re-walking the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedDecoration {
    pub row: u32,
    pub col: u16,
    pub draw: DecorationDraw,
}

/// Input bundle for [`build_frame`]. Mirrors the shape of
/// `BlockRenderInput` but adds cursor state and a previous
/// `FrameState` for the damage compare.
pub struct FrameInput<'a> {
    pub pages: &'a PageList,
    pub atlas: &'a CellStyleAtlas,
    pub visible_rows: Range<usize>,
    pub viewport_cols: u16,
    pub cursor: CursorState,
    pub prev: &'a FrameState,
    /// Terminal palette — resolves [`carrot_grid::Color`] tags on
    /// every styled cell to concrete Oklch values.
    pub palette: &'a crate::palette::TerminalPalette,
    /// Underline-color override from the theme. `None` lets per-cell
    /// style (or fg fallback) decide.
    pub underline_color_override: Option<carrot_grid::Color>,
    /// Optional image store — images placed inside this block get
    /// projected into pixel-space `ImageDraw`s and attached to the
    /// frame. `None` means "no images"; pass an empty store for the
    /// same effect.
    pub images: Option<&'a ImageStore>,
    /// Pixel dimensions of one cell, needed to project image
    /// placements from grid coords to pixel rects. Caller supplies
    /// these from their font metrics. Ignored when `images` is None.
    pub cell_pixel_width: f32,
    pub cell_pixel_height: f32,
}

/// Compose one frame of terminal output.
///
/// Runs cell rendering, damage compare, decoration extraction, and
/// cursor placement in a single pass. The returned [`Frame`] carries
/// the next-frame signature matrix so a caller can do:
///
/// ```ignore
/// let mut prev = FrameState::empty();
/// loop {
///     let frame = build_frame(FrameInput { prev: &prev, ... });
///     // paint frame.cells, frame.decorations, frame.cursor
///     prev.replace_cells(frame.visual_rows, viewport_cols, frame.signatures);
/// }
/// ```
pub fn build_frame(input: FrameInput<'_>) -> Frame {
    let FrameInput {
        pages,
        atlas,
        visible_rows,
        viewport_cols,
        cursor,
        prev,
        palette,
        underline_color_override,
        images,
        cell_pixel_width,
        cell_pixel_height,
    } = input;

    // Cell + signature pass (shared between damage compare and
    // downstream decoration extraction).
    let (all_draws, signatures, visual_rows) = render_block_with_signatures(BlockRenderInput {
        pages,
        atlas,
        visible_rows,
        viewport_cols,
    });

    let cols = viewport_cols.max(1);
    let damage = compute_damage(prev, &signatures, visual_rows, cols);

    // Filter cells by damage.
    let cells: Vec<CellDraw> = match &damage {
        Damage::Full => all_draws.clone(),
        Damage::Partial { .. } => all_draws
            .iter()
            .copied()
            .filter(|d| damage.contains(d.visual_row, d.visual_col))
            .collect(),
    };

    // Decorations for any cell that has UNDERLINE or STRIKETHROUGH.
    // Walk the draw list once; the decoration list is sparse so it
    // stays small even for large viewports.
    let mut decorations = Vec::new();
    for draw in &all_draws {
        let raw = apply_reverse_video(&draw.style);
        if !raw.flags.contains(CellStyleFlags::UNDERLINE)
            && !raw.flags.contains(CellStyleFlags::STRIKETHROUGH)
        {
            continue;
        }
        // If damage is Partial and this cell is clean, skip: the
        // previous frame's decoration is still on screen.
        if !damage.contains(draw.visual_row, draw.visual_col) {
            continue;
        }
        for deco in render_decorations(&raw, palette, underline_color_override) {
            decorations.push(PositionedDecoration {
                row: draw.visual_row,
                col: draw.visual_col,
                draw: deco,
            });
        }
    }

    let cursor_draw = render_cursor(cursor);

    // Image pass — projects the block's ImageStore into pixel-space
    // draws, filtered to the visible row range so we don't upload
    // textures for fully-scrolled-off images. `None` store yields
    // an empty list; empty store same effect.
    let images_out: Vec<ImageDraw> = match images {
        Some(store) if !store.is_empty() => {
            let all = render_images(store, cell_pixel_width, cell_pixel_height);
            // The consumer's viewport is in visual-row coords; draws
            // use data-row coords. They're identical for the no-soft-
            // wrap case which is the only shape we support today.
            let v_start = 0u32;
            let v_end = visual_rows;
            all.into_iter()
                .filter(|d| d.intersects_rows(&(v_start..v_end)))
                .collect()
        }
        _ => Vec::new(),
    };

    Frame {
        cells,
        decorations,
        images: images_out,
        cursor: cursor_draw,
        signatures,
        visual_rows,
        damage,
    }
}

/// Assists identifying what kind of decoration a
/// [`PositionedDecoration`] represents — avoids callers matching
/// on the inner `DecorationKind` directly when they only need a
/// boolean.
impl PositionedDecoration {
    pub fn is_underline(&self) -> bool {
        self.draw.kind == DecorationKind::Underline
    }

    pub fn is_strikethrough(&self) -> bool {
        self.draw.kind == DecorationKind::Strikethrough
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrot_grid::{Cell, CellStyle, CellStyleId, CellTag, PageCapacity};

    fn populate(
        cols: u16,
        rows: usize,
        styler: impl Fn(usize, u16) -> CellStyleId,
    ) -> (PageList, CellStyleAtlas) {
        let cap = PageCapacity::new(cols, 1024);
        let mut list = PageList::new(cap);
        let atlas = CellStyleAtlas::new();
        for r in 0..rows {
            let row: Vec<Cell> = (0..cols)
                .map(|c| Cell::ascii(b'a' + ((r as u8 + c as u8) % 26), styler(r, c)))
                .collect();
            list.append_row(&row);
        }
        (list, atlas)
    }

    fn default_cursor() -> CursorState {
        CursorState {
            row: 0,
            col: 0,
            shape: crate::cursor::CursorShape::Block,
            blink_phase_on: true,
            visible: true,
            cell_tag: CellTag::Ascii,
        }
    }

    #[test]
    fn fresh_frame_fills_cells_and_emits_cursor() {
        let (pages, atlas) = populate(8, 4, |_, _| CellStyleId(0));
        let frame = build_frame(FrameInput {
            pages: &pages,
            atlas: &atlas,
            visible_rows: 0..4,
            viewport_cols: 8,
            cursor: default_cursor(),
            prev: &FrameState::empty(),
            palette: &crate::palette::TerminalPalette::CARROT_DARK,
            underline_color_override: None,
            images: None,
            cell_pixel_width: 8.0,
            cell_pixel_height: 16.0,
        });
        assert_eq!(frame.damage, Damage::Full);
        assert_eq!(frame.cells.len(), 4 * 8);
        assert_eq!(frame.visual_rows, 4);
        assert!(frame.cursor.is_some());
        assert!(frame.decorations.is_empty()); // no styled cells
    }

    #[test]
    fn steady_state_frame_emits_no_cell_draws() {
        let (pages, atlas) = populate(8, 4, |_, _| CellStyleId(0));
        let first = build_frame(FrameInput {
            pages: &pages,
            atlas: &atlas,
            visible_rows: 0..4,
            viewport_cols: 8,
            cursor: default_cursor(),
            prev: &FrameState::empty(),
            palette: &crate::palette::TerminalPalette::CARROT_DARK,
            underline_color_override: None,
            images: None,
            cell_pixel_width: 8.0,
            cell_pixel_height: 16.0,
        });
        let mut prev = FrameState::with_viewport(first.visual_rows, 8);
        prev.replace_cells(first.visual_rows, 8, first.signatures);

        let second = build_frame(FrameInput {
            pages: &pages,
            atlas: &atlas,
            visible_rows: 0..4,
            viewport_cols: 8,
            cursor: default_cursor(),
            prev: &prev,
            palette: &crate::palette::TerminalPalette::CARROT_DARK,
            underline_color_override: None,
            images: None,
            cell_pixel_width: 8.0,
            cell_pixel_height: 16.0,
        });
        assert!(second.damage.is_clean());
        assert_eq!(second.cells.len(), 0);
        // Cursor is still emitted — it blinks, so it's always
        // considered "dirty" from the consumer's perspective.
        assert!(second.cursor.is_some());
    }

    #[test]
    fn hidden_cursor_keeps_cells_but_drops_cursor() {
        let (pages, atlas) = populate(4, 2, |_, _| CellStyleId(0));
        let frame = build_frame(FrameInput {
            pages: &pages,
            atlas: &atlas,
            visible_rows: 0..2,
            viewport_cols: 4,
            cursor: CursorState {
                visible: false,
                ..default_cursor()
            },
            prev: &FrameState::empty(),
            palette: &crate::palette::TerminalPalette::CARROT_DARK,
            underline_color_override: None,
            images: None,
            cell_pixel_width: 8.0,
            cell_pixel_height: 16.0,
        });
        assert_eq!(frame.cells.len(), 2 * 4);
        assert!(frame.cursor.is_none());
    }

    #[test]
    fn styled_cell_emits_decoration() {
        let cap = PageCapacity::new(2, 1024);
        let mut list = PageList::new(cap);
        let mut atlas = CellStyleAtlas::new();
        let underlined = atlas.intern(CellStyle {
            flags: CellStyleFlags::UNDERLINE,
            ..CellStyle::DEFAULT
        });
        list.append_row(&[
            Cell::ascii(b'x', underlined),
            Cell::ascii(b'y', CellStyleId(0)),
        ]);

        let frame = build_frame(FrameInput {
            pages: &list,
            atlas: &atlas,
            visible_rows: 0..1,
            viewport_cols: 2,
            cursor: default_cursor(),
            prev: &FrameState::empty(),
            palette: &crate::palette::TerminalPalette::CARROT_DARK,
            underline_color_override: None,
            images: None,
            cell_pixel_width: 8.0,
            cell_pixel_height: 16.0,
        });
        assert_eq!(frame.decorations.len(), 1);
        assert_eq!(frame.decorations[0].col, 0);
        assert!(frame.decorations[0].is_underline());
    }

    #[test]
    fn reverse_video_cell_decorates_correctly() {
        let cap = PageCapacity::new(1, 1024);
        let mut list = PageList::new(cap);
        let mut atlas = CellStyleAtlas::new();
        let reverse_underline = atlas.intern(CellStyle {
            flags: CellStyleFlags::REVERSE.insert(CellStyleFlags::UNDERLINE),
            fg: carrot_grid::Color::Named(carrot_grid::NamedColor::Green),
            bg: carrot_grid::Color::Named(carrot_grid::NamedColor::Black),
            ..CellStyle::DEFAULT
        });
        list.append_row(&[Cell::ascii(b'r', reverse_underline)]);

        let frame = build_frame(FrameInput {
            pages: &list,
            atlas: &atlas,
            visible_rows: 0..1,
            viewport_cols: 1,
            cursor: CursorState {
                visible: false,
                ..default_cursor()
            },
            prev: &FrameState::empty(),
            palette: &crate::palette::TerminalPalette::CARROT_DARK,
            underline_color_override: None,
            images: None,
            cell_pixel_width: 8.0,
            cell_pixel_height: 16.0,
        });
        // Reverse-video swapped the colors before decoration color
        // resolution, so the underline color resolves to what was the
        // background (here: Named::Black in the Carrot Dark palette).
        assert_eq!(frame.decorations.len(), 1);
        assert_eq!(
            frame.decorations[0].draw.color,
            crate::palette::TerminalPalette::CARROT_DARK.black,
        );
    }

    #[test]
    fn partial_damage_keeps_matching_decorations_only() {
        // Two cells both underlined. First frame full render; second
        // frame with only cell (0, 1) changed — decoration on cell
        // (0, 0) should NOT be emitted again, decoration on cell
        // (0, 1) SHOULD be (because the cell is dirty).
        let cap = PageCapacity::new(2, 1024);
        let mut list = PageList::new(cap);
        let mut atlas = CellStyleAtlas::new();
        let underlined = atlas.intern(CellStyle {
            flags: CellStyleFlags::UNDERLINE,
            ..CellStyle::DEFAULT
        });
        list.append_row(&[Cell::ascii(b'a', underlined), Cell::ascii(b'b', underlined)]);

        let first = build_frame(FrameInput {
            pages: &list,
            atlas: &atlas,
            visible_rows: 0..1,
            viewport_cols: 2,
            cursor: CursorState {
                visible: false,
                ..default_cursor()
            },
            prev: &FrameState::empty(),
            palette: &crate::palette::TerminalPalette::CARROT_DARK,
            underline_color_override: None,
            images: None,
            cell_pixel_width: 8.0,
            cell_pixel_height: 16.0,
        });
        assert_eq!(first.decorations.len(), 2);

        let mut prev = FrameState::with_viewport(first.visual_rows, 2);
        prev.replace_cells(first.visual_rows, 2, first.signatures);

        // Swap cell (0, 1) to 'B'.
        let cap2 = PageCapacity::new(2, 1024);
        let mut list2 = PageList::new(cap2);
        let mut atlas2 = CellStyleAtlas::new();
        let underlined2 = atlas2.intern(CellStyle {
            flags: CellStyleFlags::UNDERLINE,
            ..CellStyle::DEFAULT
        });
        list2.append_row(&[
            Cell::ascii(b'a', underlined2),
            Cell::ascii(b'B', underlined2),
        ]);

        let second = build_frame(FrameInput {
            pages: &list2,
            atlas: &atlas2,
            visible_rows: 0..1,
            viewport_cols: 2,
            cursor: CursorState {
                visible: false,
                ..default_cursor()
            },
            prev: &prev,
            palette: &crate::palette::TerminalPalette::CARROT_DARK,
            underline_color_override: None,
            images: None,
            cell_pixel_width: 8.0,
            cell_pixel_height: 16.0,
        });
        assert_eq!(second.cells.len(), 1);
        assert_eq!(second.decorations.len(), 1);
        assert_eq!(second.decorations[0].col, 1);
    }

    #[test]
    fn frame_includes_images_when_store_present() {
        use carrot_grid::{DecodedImage, ImageFormat, ImageStore, Placement};
        use std::sync::Arc;

        let (pages, atlas) = populate(10, 4, |_, _| CellStyleId(0));
        let mut images = ImageStore::new();
        let img = Arc::new(DecodedImage::new(
            16,
            16,
            ImageFormat::Rgba8,
            vec![0u8; 16 * 16 * 4],
        ));
        images.push(img, Placement::at(1, 2, 2, 3));

        let frame = build_frame(FrameInput {
            pages: &pages,
            atlas: &atlas,
            visible_rows: 0..4,
            viewport_cols: 10,
            cursor: CursorState {
                visible: false,
                ..default_cursor()
            },
            prev: &FrameState::empty(),
            palette: &crate::palette::TerminalPalette::CARROT_DARK,
            underline_color_override: None,
            images: Some(&images),
            cell_pixel_width: 8.0,
            cell_pixel_height: 16.0,
        });
        assert_eq!(frame.images.len(), 1);
        // col 2 × 8 = 16, row 1 × 16 = 16
        assert_eq!(frame.images[0].pixel_x, 16.0);
        assert_eq!(frame.images[0].pixel_y, 16.0);
    }

    #[test]
    fn frame_skips_offscreen_images() {
        use carrot_grid::{DecodedImage, ImageFormat, ImageStore, Placement};
        use std::sync::Arc;

        let (pages, atlas) = populate(10, 4, |_, _| CellStyleId(0));
        let mut images = ImageStore::new();
        let img = Arc::new(DecodedImage::new(
            16,
            16,
            ImageFormat::Rgba8,
            vec![0u8; 16 * 16 * 4],
        ));
        // Image at row 100 — way outside our 4-row visible range.
        images.push(img, Placement::at(100, 0, 1, 1));

        let frame = build_frame(FrameInput {
            pages: &pages,
            atlas: &atlas,
            visible_rows: 0..4,
            viewport_cols: 10,
            cursor: CursorState {
                visible: false,
                ..default_cursor()
            },
            prev: &FrameState::empty(),
            palette: &crate::palette::TerminalPalette::CARROT_DARK,
            underline_color_override: None,
            images: Some(&images),
            cell_pixel_width: 8.0,
            cell_pixel_height: 16.0,
        });
        assert!(
            frame.images.is_empty(),
            "offscreen image should be filtered"
        );
    }

    #[test]
    fn frame_images_none_produces_empty_vec() {
        let (pages, atlas) = populate(4, 2, |_, _| CellStyleId(0));
        let frame = build_frame(FrameInput {
            pages: &pages,
            atlas: &atlas,
            visible_rows: 0..2,
            viewport_cols: 4,
            cursor: CursorState {
                visible: false,
                ..default_cursor()
            },
            prev: &FrameState::empty(),
            palette: &crate::palette::TerminalPalette::CARROT_DARK,
            underline_color_override: None,
            images: None,
            cell_pixel_width: 8.0,
            cell_pixel_height: 16.0,
        });
        assert!(frame.images.is_empty());
    }
}
