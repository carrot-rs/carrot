//! View-wide scrollback display state.
//!
//! The legacy [`crate::grid::Grid`] carries a per-grid `display_offset`
//! because it models scrollback as a single linear buffer. block
//! keeps each block's `PageList` at rest (no scrollback shuffle on
//! scroll-up) and tracks the view's scroll position at the router
//! level instead. One `display_offset` describes how many rows the
//! viewport is scrolled back from the live tail across the whole
//! chain of frozen + active blocks.
//!
//! The offset is in "visual rows" from the bottom — `0` = at the tail
//! (live, un-scrolled), `N` = scrolled N rows back into history.
//!
//! # Scope
//!
//! This module owns only the offset state and the offset-mutation
//! API. Viewport clamping, damage tracking, selection-rotate-on-
//! scroll are Layer 5 / per-block concerns and live elsewhere.

use super::router::BlockRouter;

/// Commands the router accepts from the UI to mutate the display
/// offset. Mirrors `carrot_term::grid::Scroll`; kept here so block
/// doesn't import the legacy grid type.
#[derive(Debug, Clone, Copy)]
pub enum Scroll {
    /// Scroll by a signed delta. Positive = toward newer rows (down,
    /// reducing offset); negative = toward older rows (up).
    Delta(i32),
    /// Scroll one viewport up.
    PageUp,
    /// Scroll one viewport down.
    PageDown,
    /// Jump to the oldest scrollback row (max offset).
    Top,
    /// Jump to the live tail (offset = 0).
    Bottom,
}

/// View-wide scrollback state.
///
/// `display_offset = 0` means the viewport shows the live tail.
/// `display_offset = N` scrolls N rows back into history.
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplayState {
    /// Rows scrolled back from the bottom. Clamped to
    /// `0..=max_offset` at mutation time.
    pub display_offset: usize,
}

impl DisplayState {
    pub fn new() -> Self {
        Self { display_offset: 0 }
    }

    /// Clamp `display_offset` to the [0, max] window. Callers pass
    /// `max` computed from the current total scrollback rows minus
    /// the viewport height; this module stays shape-agnostic.
    pub fn clamp(&mut self, max: usize) {
        if self.display_offset > max {
            self.display_offset = max;
        }
    }

    /// Apply a [`Scroll`] command with the given viewport height and
    /// max-scrollback bound. Returns the old offset so callers can
    /// damage-invalidate on change.
    pub fn apply(&mut self, scroll: Scroll, viewport_rows: usize, max: usize) -> usize {
        let old = self.display_offset;
        let new = match scroll {
            Scroll::Bottom => 0,
            Scroll::Top => max,
            Scroll::PageUp => self.display_offset.saturating_add(viewport_rows).min(max),
            Scroll::PageDown => self.display_offset.saturating_sub(viewport_rows),
            Scroll::Delta(d) if d >= 0 => self.display_offset.saturating_add(d as usize).min(max),
            Scroll::Delta(d) => self.display_offset.saturating_sub((-d) as usize),
        };
        self.display_offset = new;
        old
    }
}

impl BlockRouter {
    /// Read-only access to the router's scrollback state.
    pub fn display_state(&self) -> &DisplayState {
        self.display_state_ref()
    }

    /// Mutable access — primarily for test fixtures.
    pub fn display_state_mut(&mut self) -> &mut DisplayState {
        self.display_state_ref_mut()
    }

    /// Apply a scroll command against the current viewport + max.
    /// The max bound is the total-rows-minus-viewport across all
    /// frozen + active blocks. See the caller for the computation;
    /// block intentionally doesn't own "visible row arithmetic"
    /// because scrollback spans multiple blocks.
    pub fn scroll_display(&mut self, scroll: Scroll, viewport_rows: usize, max: usize) -> usize {
        self.display_state_ref_mut()
            .apply(scroll, viewport_rows, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_state_at_tail() {
        let s = DisplayState::new();
        assert_eq!(s.display_offset, 0);
    }

    #[test]
    fn apply_bottom_snaps_to_zero() {
        let mut s = DisplayState { display_offset: 42 };
        s.apply(Scroll::Bottom, 24, 100);
        assert_eq!(s.display_offset, 0);
    }

    #[test]
    fn apply_top_snaps_to_max() {
        let mut s = DisplayState::new();
        s.apply(Scroll::Top, 24, 100);
        assert_eq!(s.display_offset, 100);
    }

    #[test]
    fn apply_page_up_adds_viewport() {
        let mut s = DisplayState::new();
        s.apply(Scroll::PageUp, 24, 1000);
        assert_eq!(s.display_offset, 24);
    }

    #[test]
    fn apply_page_up_clamps_at_max() {
        let mut s = DisplayState { display_offset: 90 };
        s.apply(Scroll::PageUp, 24, 100);
        assert_eq!(s.display_offset, 100);
    }

    #[test]
    fn apply_page_down_subtracts_viewport() {
        let mut s = DisplayState { display_offset: 50 };
        s.apply(Scroll::PageDown, 24, 100);
        assert_eq!(s.display_offset, 26);
    }

    #[test]
    fn apply_page_down_saturates_at_zero() {
        let mut s = DisplayState { display_offset: 5 };
        s.apply(Scroll::PageDown, 24, 100);
        assert_eq!(s.display_offset, 0);
    }

    #[test]
    fn apply_delta_respects_sign() {
        let mut s = DisplayState::new();
        s.apply(Scroll::Delta(5), 24, 100);
        assert_eq!(s.display_offset, 5);
        s.apply(Scroll::Delta(-3), 24, 100);
        assert_eq!(s.display_offset, 2);
    }

    #[test]
    fn apply_returns_old_offset_for_damage_tracking() {
        let mut s = DisplayState { display_offset: 10 };
        let prev = s.apply(Scroll::PageDown, 5, 100);
        assert_eq!(prev, 10);
        assert_eq!(s.display_offset, 5);
    }

    #[test]
    fn clamp_caps_to_max() {
        let mut s = DisplayState {
            display_offset: 200,
        };
        s.clamp(100);
        assert_eq!(s.display_offset, 100);
    }

    #[test]
    fn router_scroll_display_roundtrips_state() {
        let mut r = BlockRouter::new(40);
        let prev = r.scroll_display(Scroll::Top, 24, 500);
        assert_eq!(prev, 0);
        assert_eq!(r.display_state().display_offset, 500);
        let prev = r.scroll_display(Scroll::Bottom, 24, 500);
        assert_eq!(prev, 500);
        assert_eq!(r.display_state().display_offset, 0);
    }
}
