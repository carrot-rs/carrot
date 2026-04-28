//! Live-frame tracking for TUI-aware block rendering.
//!
//! A "live frame" is a rectangular region inside an [`crate::block::
//! ActiveBlock`] where a TUI is doing in-place redraws — the classic
//! log-update pattern: cursor-up-N → overwrite → repeat. Rows inside a
//! live frame must NOT be pushed to scrollback when they scroll off
//! the top, because the TUI relies on cursor-up reaching them again.
//!
//! This is the block port of the legacy `carrot_term::block::grid::
//! LiveFrameRegion`. The shape matches field-for-field; the only
//! change is the anchor type — v2 uses the prune-safe
//! [`carrot_grid::CellId::origin`] (a monotonic row sequence number)
//! instead of a viewport-relative `Line`. That keeps the region valid
//! across scrollback pruning — dropping the head of the PageList
//! renames row indices, but origin numbers are stable forever.
//!
//! # Priority rules
//!
//! Multiple detection paths may fire for the same block (explicit
//! shell hint, DEC 2026 sync-update, cursor-up heuristic). Only the
//! highest-priority source holds the region at a time:
//!
//! 1. `ShellHint` (priority 3): strongest — the shell guarantees the
//!    whole block is a TUI.
//! 2. `SyncUpdate` (priority 2): DEC 2026 BSU/ESU frames.
//! 3. `Heuristic` (priority 1): cursor-up-N bump, weakest.
//!
//! Lower priority never overwrites higher — the old region is kept
//! and its `last_activity` timer is refreshed.

use std::time::Instant;

use super::active::ActiveBlock;

/// How a [`LiveFrameRegion`] was activated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveFrameSource {
    /// Shell-emitted OSC 7777 `carrot-tui-hint` with `tui_mode = true`.
    ShellHint,
    /// DEC 2026 synchronized update. Each BSU…ESU cycle maps to one
    /// reprint; the region spans from the BSU cursor row down to the
    /// row at ESU close.
    SyncUpdate,
    /// Cursor-up-N motion heuristic (N ≥ 2). Fallback for TUIs that
    /// don't wrap frames in DEC 2026.
    Heuristic,
}

impl LiveFrameSource {
    /// Higher numeric priority wins when multiple sources would
    /// activate. Public so callers can order their own fallback
    /// strategies; matches the legacy layout.
    pub fn priority(self) -> u8 {
        match self {
            Self::ShellHint => 3,
            Self::SyncUpdate => 2,
            Self::Heuristic => 1,
        }
    }
}

/// A contiguous region inside an active block that the TUI is
/// redrawing in place.
///
/// `start_origin` is a [`carrot_grid::CellId::origin`] — a monotonic
/// row-append sequence number. Combined with `height` it spans the
/// origins `[start_origin, start_origin + height)`. Pruning the head
/// of the block's PageList leaves the origins unchanged, so the
/// region stays valid across memory evictions.
#[derive(Debug, Clone)]
pub struct LiveFrameRegion {
    /// First row's append-origin (inclusive).
    pub start_origin: u64,
    /// Row count in the region. Grown by the detector as the TUI
    /// expands.
    pub height: usize,
    /// Completed reprints (ESU→ESU cycles). Surfaced in the block
    /// header by Layer 5.
    pub reprint_count: u32,
    /// Which path activated this region.
    pub source: LiveFrameSource,
    /// Last time the region saw activity. Heuristic source resets
    /// after an idle timeout; higher-priority sources ignore this.
    pub last_activity: Instant,
}

impl LiveFrameRegion {
    /// Convenience factory — most callers go through
    /// [`ActiveBlock::activate_live_frame`].
    pub fn new(start_origin: u64, height: usize, source: LiveFrameSource) -> Self {
        Self {
            start_origin,
            height,
            reprint_count: 0,
            source,
            last_activity: Instant::now(),
        }
    }

    /// Bump the reprint counter (ESU commit path).
    pub fn bump_reprint_count(&mut self) {
        self.reprint_count = self.reprint_count.saturating_add(1);
        self.last_activity = Instant::now();
    }

    /// Is `origin` inside the `[start_origin, start_origin + height)`
    /// window? Rows outside the window are normal scrollback output.
    pub fn contains(&self, origin: u64) -> bool {
        origin >= self.start_origin && origin < self.start_origin.saturating_add(self.height as u64)
    }
}

impl ActiveBlock {
    /// Activate or refresh the live-frame region. Respects the
    /// priority ordering in [`LiveFrameSource`]:
    /// - Higher-priority source already active → refresh `last_activity`
    ///   and return, leaving the region shape alone.
    /// - Same-or-lower priority already active → overwrite `source`,
    ///   keep height at `max(current, initial_height)`.
    /// - No region yet → install a fresh [`LiveFrameRegion`].
    pub fn activate_live_frame(
        &mut self,
        start_origin: u64,
        initial_height: usize,
        source: LiveFrameSource,
    ) {
        match self.live_frame_slot_mut().as_mut() {
            Some(lf) if lf.source.priority() > source.priority() => {
                lf.last_activity = Instant::now();
            }
            Some(lf) => {
                lf.source = source;
                lf.last_activity = Instant::now();
                if lf.height < initial_height {
                    lf.height = initial_height;
                }
                if lf.start_origin > start_origin {
                    lf.start_origin = start_origin;
                }
            }
            None => {
                *self.live_frame_slot_mut() =
                    Some(LiveFrameRegion::new(start_origin, initial_height, source));
            }
        }
        // Sticky promotion: any successful live-frame activation marks
        // this block as a TUI block for the rest of its lifetime, even
        // if the region is later cleared (heuristic reset, alt-screen
        // exit). See `super::kind::BlockKind` doc-comment.
        self.promote_kind_to_tui();
    }

    /// Drop the region only if it was activated by the `Heuristic`
    /// source AND the last activity was more than `idle_ms` ago.
    /// Higher-priority sources stick until their own lifecycle end
    /// (shell-integration block-finalize, DEC 2026 ESU close).
    pub fn maybe_reset_heuristic_live_frame(&mut self, now: Instant, idle_ms: u64) {
        let clear = match self.live_frame_slot().as_ref() {
            Some(lf) if lf.source == LiveFrameSource::Heuristic => {
                now.saturating_duration_since(lf.last_activity).as_millis() as u64 >= idle_ms
            }
            _ => false,
        };
        if clear {
            *self.live_frame_slot_mut() = None;
        }
    }

    /// Read-only access to the region, if any.
    pub fn live_frame(&self) -> Option<&LiveFrameRegion> {
        self.live_frame_slot().as_ref()
    }

    /// Mutable access — for the ESU commit path that bumps the
    /// reprint counter + extends height.
    pub fn live_frame_mut(&mut self) -> Option<&mut LiveFrameRegion> {
        self.live_frame_slot_mut().as_mut()
    }

    /// Explicitly clear the region regardless of source. Called when
    /// the block freezes at `CommandEnd`.
    pub fn clear_live_frame(&mut self) {
        *self.live_frame_slot_mut() = None;
    }

    /// Increment the reprint counter on the active region, if any.
    pub fn bump_live_frame_reprint(&mut self) {
        if let Some(lf) = self.live_frame_slot_mut().as_mut() {
            lf.bump_reprint_count();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_order_is_shellhint_then_sync_then_heuristic() {
        assert!(LiveFrameSource::ShellHint.priority() > LiveFrameSource::SyncUpdate.priority());
        assert!(LiveFrameSource::SyncUpdate.priority() > LiveFrameSource::Heuristic.priority());
    }

    #[test]
    fn region_contains_window_is_half_open() {
        let lf = LiveFrameRegion::new(10, 3, LiveFrameSource::Heuristic);
        assert!(lf.contains(10));
        assert!(lf.contains(11));
        assert!(lf.contains(12));
        assert!(!lf.contains(13));
        assert!(!lf.contains(9));
    }

    #[test]
    fn fresh_activation_installs_region() {
        let mut block = ActiveBlock::new(20);
        assert!(block.live_frame().is_none());
        block.activate_live_frame(5, 2, LiveFrameSource::SyncUpdate);
        let lf = block.live_frame().expect("installed");
        assert_eq!(lf.start_origin, 5);
        assert_eq!(lf.height, 2);
        assert_eq!(lf.source, LiveFrameSource::SyncUpdate);
        assert_eq!(lf.reprint_count, 0);
    }

    #[test]
    fn shellhint_replaces_sync_update() {
        let mut block = ActiveBlock::new(20);
        block.activate_live_frame(5, 2, LiveFrameSource::SyncUpdate);
        block.activate_live_frame(0, 5, LiveFrameSource::ShellHint);
        let lf = block.live_frame().expect("still installed");
        assert_eq!(lf.source, LiveFrameSource::ShellHint);
        // Height grows when replacement wants more rows.
        assert_eq!(lf.height, 5);
        // Lower-numbered start_origin wins to keep the window inclusive.
        assert_eq!(lf.start_origin, 0);
    }

    #[test]
    fn heuristic_never_overrides_shell_hint() {
        let mut block = ActiveBlock::new(20);
        block.activate_live_frame(3, 4, LiveFrameSource::ShellHint);
        block.activate_live_frame(10, 2, LiveFrameSource::Heuristic);
        let lf = block.live_frame().expect("kept");
        assert_eq!(lf.source, LiveFrameSource::ShellHint);
        assert_eq!(lf.start_origin, 3);
        assert_eq!(lf.height, 4);
    }

    #[test]
    fn heuristic_idle_reset_drops_region() {
        let mut block = ActiveBlock::new(20);
        block.activate_live_frame(5, 2, LiveFrameSource::Heuristic);
        let after = Instant::now() + std::time::Duration::from_millis(1_000);
        block.maybe_reset_heuristic_live_frame(after, 500);
        assert!(block.live_frame().is_none());
    }

    #[test]
    fn idle_reset_ignores_non_heuristic_sources() {
        let mut block = ActiveBlock::new(20);
        block.activate_live_frame(5, 2, LiveFrameSource::SyncUpdate);
        let after = Instant::now() + std::time::Duration::from_millis(10_000);
        block.maybe_reset_heuristic_live_frame(after, 500);
        assert!(block.live_frame().is_some());
    }

    #[test]
    fn bump_reprint_increments_counter() {
        let mut block = ActiveBlock::new(20);
        block.activate_live_frame(0, 1, LiveFrameSource::SyncUpdate);
        block.bump_live_frame_reprint();
        block.bump_live_frame_reprint();
        assert_eq!(block.live_frame().unwrap().reprint_count, 2);
    }

    #[test]
    fn clear_live_frame_drops_unconditionally() {
        let mut block = ActiveBlock::new(20);
        block.activate_live_frame(0, 1, LiveFrameSource::ShellHint);
        block.clear_live_frame();
        assert!(block.live_frame().is_none());
    }
}
