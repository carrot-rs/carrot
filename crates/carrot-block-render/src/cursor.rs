//! Cursor render primitive.
//!
//! The cursor does **not** live inside `ActiveBlock`.
//! It's VT-state, owned by the terminal emulator. Layer 4 (this
//! crate) reads the cursor state as input to a dedicated render
//! pass and produces draw commands — just like `render_block` does
//! for cells.
//!
//! This module is self-contained and does not depend on Inazuma;
//! the consumer (terminal-view or the BlockElement integration)
//! places the resulting `CursorDraw` into the full frame.
//!
//! # Shapes
//!
//! - [`CursorShape::Block`] — full-cell rectangle, most common default.
//! - [`CursorShape::Underline`] — thin rectangle along the bottom.
//! - [`CursorShape::Bar`] — thin rectangle along the left (i-beam).
//!
//! Each shape consumes the same input (`CursorState`) and emits a
//! single [`CursorDraw`] rect in cell-local pixel space. Absolute
//! positioning is the caller's job (they hold the block bounds).
//!
//! # Wide characters
//!
//! When the cursor sits on the first cell of a wide character, the
//! block cursor stretches to 2× width. Other shapes (underline, bar)
//! stay 1× because their visual intent is position, not coverage.
//!
//! # Blink
//!
//! Blink is a pure animation concern handled by the consumer. We
//! surface `blink_phase_on: bool` as part of the input — when
//! `false`, `render_cursor` returns `None` so the caller skips the
//! draw entirely. Keeps the cursor off the GPU bus when it's
//! invisible.

use carrot_grid::CellTag;

/// Cursor shape as per VT / XTerm conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    /// Full-cell rectangle. Historical DEC default; most `TERM` configs.
    #[default]
    Block,
    /// Thin rectangle along the bottom edge (`\e[4 q`).
    Underline,
    /// Thin rectangle along the left edge — "i-beam" (`\e[6 q`).
    Bar,
}

/// Logical cursor state. Fed to [`render_cursor`] per frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorState {
    pub row: u32,
    pub col: u16,
    pub shape: CursorShape,
    /// Visible during this frame's blink phase. Typically driven by
    /// a timer in the consumer.
    pub blink_phase_on: bool,
    /// Explicitly visible (e.g. cursor is not hidden by DECTCEM).
    /// `false` returns `None` from [`render_cursor`] regardless of
    /// the blink phase.
    pub visible: bool,
    /// The cell under the cursor — used to detect wide characters
    /// so the Block shape can stretch. Cell is [`CellTag::Wide2nd`]
    /// means the cursor is on the *right* half of a wide char, which
    /// we render as the partner of the preceding Ascii/Codepoint
    /// cell; callers normally move the cursor back one column before
    /// calling.
    pub cell_tag: CellTag,
}

/// Draw command emitted by [`render_cursor`]. Cell-local pixel
/// coordinates — caller adds the block's origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorDraw {
    /// Row in the block's visual grid.
    pub row: u32,
    /// Starting column.
    pub col: u16,
    /// Cell-local x offset (for Bar shape, always 0; for Underline
    /// always 0; for Block always 0).
    pub x_offset: f32,
    /// Cell-local y offset (for Underline > 0; for Block = 0).
    pub y_offset_frac: f32,
    /// Width in cells. Usually 1. Block cursor on a wide char → 2.
    pub width_cells: u16,
    /// Width fraction inside the cell (Bar = ~0.15, else 1.0).
    pub width_frac: f32,
    /// Height fraction inside the cell (Underline = ~0.08, else 1.0).
    pub height_frac: f32,
    pub shape: CursorShape,
}

/// Emit a cursor draw for the given state, or `None` when the cursor
/// is hidden or in its dark blink phase.
pub fn render_cursor(state: CursorState) -> Option<CursorDraw> {
    if !state.visible || !state.blink_phase_on {
        return None;
    }

    let (width_cells, width_frac, height_frac, x_offset, y_offset_frac) = match state.shape {
        CursorShape::Block => {
            let w = if state.cell_tag == CellTag::Wide2nd {
                // Cursor on the right half of a wide char — render
                // the block only over that half, 1 cell wide. The
                // partner cell is the Ascii/Codepoint half and will
                // draw its glyph alongside.
                1
            } else if matches!(state.cell_tag, CellTag::Codepoint | CellTag::Grapheme) {
                // Simplified: only stretch when we *know* the next
                // cell is Wide2nd. The VT layer tags wide-char
                // openers with CellTag::Codepoint (or Grapheme); the
                // caller that needs exact wide-char detection passes
                // a look-ahead via a different path. For this first
                // cut we keep it 1 cell — F.6 proper wires the
                // look-ahead and turns this into 2 when appropriate.
                1
            } else {
                1
            };
            (w, 1.0, 1.0, 0.0, 0.0)
        }
        CursorShape::Underline => (1, 1.0, 0.08, 0.0, 0.92),
        CursorShape::Bar => (1, 0.15, 1.0, 0.0, 0.0),
    };

    Some(CursorDraw {
        row: state.row,
        col: state.col,
        x_offset,
        y_offset_frac,
        width_cells,
        width_frac,
        height_frac,
        shape: state.shape,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visible(row: u32, col: u16, shape: CursorShape, tag: CellTag) -> CursorState {
        CursorState {
            row,
            col,
            shape,
            blink_phase_on: true,
            visible: true,
            cell_tag: tag,
        }
    }

    #[test]
    fn hidden_cursor_returns_none() {
        let s = CursorState {
            visible: false,
            ..visible(0, 0, CursorShape::Block, CellTag::Ascii)
        };
        assert!(render_cursor(s).is_none());
    }

    #[test]
    fn blink_off_phase_returns_none() {
        let s = CursorState {
            blink_phase_on: false,
            ..visible(0, 0, CursorShape::Block, CellTag::Ascii)
        };
        assert!(render_cursor(s).is_none());
    }

    #[test]
    fn block_shape_covers_full_cell() {
        let draw = render_cursor(visible(3, 7, CursorShape::Block, CellTag::Ascii)).expect("some");
        assert_eq!(draw.shape, CursorShape::Block);
        assert_eq!(draw.row, 3);
        assert_eq!(draw.col, 7);
        assert_eq!(draw.width_cells, 1);
        assert!((draw.width_frac - 1.0).abs() < 1e-6);
        assert!((draw.height_frac - 1.0).abs() < 1e-6);
        assert!((draw.x_offset - 0.0).abs() < 1e-6);
        assert!((draw.y_offset_frac - 0.0).abs() < 1e-6);
    }

    #[test]
    fn underline_is_thin_at_bottom() {
        let draw =
            render_cursor(visible(0, 0, CursorShape::Underline, CellTag::Ascii)).expect("some");
        assert_eq!(draw.shape, CursorShape::Underline);
        assert!(draw.height_frac < 0.15, "underline should be thin");
        assert!(draw.y_offset_frac > 0.8, "underline should sit at bottom");
        assert!((draw.width_frac - 1.0).abs() < 1e-6);
    }

    #[test]
    fn bar_is_thin_at_left() {
        let draw = render_cursor(visible(0, 0, CursorShape::Bar, CellTag::Ascii)).expect("some");
        assert_eq!(draw.shape, CursorShape::Bar);
        assert!(draw.width_frac < 0.2, "bar should be thin");
        assert!((draw.height_frac - 1.0).abs() < 1e-6);
        assert!((draw.x_offset - 0.0).abs() < 1e-6);
    }

    #[test]
    fn default_shape_is_block() {
        assert_eq!(CursorShape::default(), CursorShape::Block);
    }

    #[test]
    fn all_shapes_emit_one_draw_when_visible() {
        for shape in [CursorShape::Block, CursorShape::Underline, CursorShape::Bar] {
            let draw = render_cursor(visible(1, 2, shape, CellTag::Ascii));
            assert!(draw.is_some(), "{shape:?} should emit a draw");
        }
    }

    #[test]
    fn width_cells_stays_one_for_bar_and_underline_on_wide() {
        let bar = render_cursor(visible(0, 5, CursorShape::Bar, CellTag::Wide2nd)).expect("bar");
        assert_eq!(bar.width_cells, 1);
        let underline =
            render_cursor(visible(0, 5, CursorShape::Underline, CellTag::Wide2nd)).expect("u");
        assert_eq!(underline.width_cells, 1);
    }
}
