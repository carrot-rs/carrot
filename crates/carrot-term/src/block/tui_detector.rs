//! TUI-detection observer — drives [`super::live_frame::LiveFrameRegion`]
//! activation from VT writer events.
//!
//! Three independent signals feed into the detector:
//!
//! 1. **Shell hint**: OSC 7777 `carrot-tui-hint` with `tui_mode = true`.
//!    Highest priority — the shell promises this block is a TUI.
//! 2. **Sync update**: DEC 2026 `CSI ? 2026 h` (BSU) and `CSI ? 2026 l`
//!    (ESU). Each BSU…ESU pair is one reprint. The detector marks the
//!    block as `SyncUpdate` at BSU and bumps `reprint_count` at ESU.
//! 3. **Heuristic**: consecutive cursor-up CSI sequences of N ≥ 2.
//!    Fallback for TUIs that don't wrap frames in DEC 2026.
//!
//! The detector is policy-only. It does not parse VT — the
//! [`crate::block::vt_writer::VtWriter`] calls into the detector at
//! the semantic points where each signal fires. That keeps the writer
//! cheap and detector swappable for testing.
//!
//! # Binding
//!
//! The detector lives on the [`crate::block::BlockRouter`] level,
//! because its signals span block boundaries (shell hint can arrive
//! before the next `on_command_start`). For tests and fixtures, the
//! detector is independent of the router and can be driven directly.

use std::time::Instant;

use super::active::ActiveBlock;
use super::live_frame::LiveFrameSource;

/// TUI-detection policy. Mirrors [`crate::term::TuiAwareness`] on the
/// legacy side, but owned here so block doesn't pull term-level
/// types into the detector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TuiAwareness {
    /// All three signals active.
    #[default]
    Full,
    /// DEC 2026 + shell hints only — heuristic disabled.
    StrictProtocol,
    /// Off. Diagnostic escape hatch.
    Off,
}

impl TuiAwareness {
    pub fn protocol_enabled(self) -> bool {
        matches!(self, Self::Full | Self::StrictProtocol)
    }
    pub fn shell_hint_enabled(self) -> bool {
        matches!(self, Self::Full | Self::StrictProtocol)
    }
    pub fn heuristic_enabled(self) -> bool {
        matches!(self, Self::Full)
    }
}

/// Observer state. Owned by the router / terminal core; consumed by
/// the writer through the `notify_*` methods.
///
/// The detector is effect-free on its own: every `notify_*` returns
/// a [`TuiEffect`] describing the region change. Callers apply the
/// effect to whichever [`ActiveBlock`] is current. Two-phase design
/// keeps the detector unit-testable without borrowing active blocks.
#[derive(Debug, Clone)]
pub struct TuiDetector {
    pub policy: TuiAwareness,
    /// Heuristic minimum cursor-up distance before activation fires.
    /// Legacy uses `2`; kept configurable for tests.
    pub heuristic_threshold: usize,
    /// Idle timeout before the heuristic region is cleared.
    pub heuristic_idle_ms: u64,
}

impl TuiDetector {
    pub fn new(policy: TuiAwareness) -> Self {
        Self {
            policy,
            heuristic_threshold: 2,
            heuristic_idle_ms: 500,
        }
    }

    /// Shell announced this block is a TUI.
    pub fn on_shell_hint(&self, cursor_origin: u64, height: usize) -> TuiEffect {
        if !self.policy.shell_hint_enabled() {
            return TuiEffect::None;
        }
        TuiEffect::Activate {
            start_origin: cursor_origin,
            height,
            source: LiveFrameSource::ShellHint,
        }
    }

    /// DEC 2026 BSU — start of a synchronized update frame.
    pub fn on_sync_update_begin(&self, cursor_origin: u64) -> TuiEffect {
        if !self.policy.protocol_enabled() {
            return TuiEffect::None;
        }
        TuiEffect::Activate {
            start_origin: cursor_origin,
            height: 1,
            source: LiveFrameSource::SyncUpdate,
        }
    }

    /// DEC 2026 ESU — close of a synchronized update frame. The
    /// detector requests a reprint-count bump plus a height snap to
    /// the current cursor distance.
    pub fn on_sync_update_end(&self, cursor_origin: u64) -> TuiEffect {
        if !self.policy.protocol_enabled() {
            return TuiEffect::None;
        }
        TuiEffect::CommitSync { cursor_origin }
    }

    /// Cursor-up CSI with `n` lines moved. Fires the heuristic when
    /// `n ≥ heuristic_threshold` and the block has enough content
    /// above the cursor to overlay.
    pub fn on_cursor_up(&self, cursor_origin: u64, n: usize) -> TuiEffect {
        if !self.policy.heuristic_enabled() || n < self.heuristic_threshold {
            return TuiEffect::None;
        }
        let start = cursor_origin.saturating_sub(n as u64);
        TuiEffect::Activate {
            start_origin: start,
            height: n + 1,
            source: LiveFrameSource::Heuristic,
        }
    }

    /// Idle tick — called periodically by the event loop. Returns
    /// [`TuiEffect::ResetHeuristic`] when the heuristic region should
    /// be evaluated for idle-drop.
    pub fn on_idle_tick(&self, now: Instant) -> TuiEffect {
        if !self.policy.heuristic_enabled() {
            return TuiEffect::None;
        }
        TuiEffect::ResetHeuristic {
            now,
            idle_ms: self.heuristic_idle_ms,
        }
    }
}

impl Default for TuiDetector {
    fn default() -> Self {
        Self::new(TuiAwareness::default())
    }
}

/// Instruction the detector returns; the caller applies it to an
/// [`ActiveBlock`].
#[derive(Debug, Clone)]
pub enum TuiEffect {
    /// No-op for this signal under the active policy.
    None,
    /// Install / upgrade the live-frame region.
    Activate {
        start_origin: u64,
        height: usize,
        source: LiveFrameSource,
    },
    /// ESU close — bump reprint count and snap height to
    /// `cursor_origin - region.start_origin + 1` if that's larger
    /// than the current height.
    CommitSync { cursor_origin: u64 },
    /// Maybe-clear the heuristic region if idle.
    ResetHeuristic { now: Instant, idle_ms: u64 },
}

impl TuiEffect {
    /// Apply the effect to `block`. Safe to call unconditionally —
    /// `None` is a no-op.
    pub fn apply(self, block: &mut ActiveBlock) {
        match self {
            TuiEffect::None => {}
            TuiEffect::Activate {
                start_origin,
                height,
                source,
            } => {
                block.activate_live_frame(start_origin, height, source);
            }
            TuiEffect::CommitSync { cursor_origin } => {
                if let Some(lf) = block.live_frame_mut() {
                    let span = cursor_origin.saturating_sub(lf.start_origin) as usize + 1;
                    if span > lf.height {
                        lf.height = span;
                    }
                    lf.bump_reprint_count();
                }
            }
            TuiEffect::ResetHeuristic { now, idle_ms } => {
                block.maybe_reset_heuristic_live_frame(now, idle_ms);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_hint_activates_with_shellhint_source() {
        let d = TuiDetector::default();
        let e = d.on_shell_hint(7, 12);
        match e {
            TuiEffect::Activate {
                start_origin,
                height,
                source,
            } => {
                assert_eq!(start_origin, 7);
                assert_eq!(height, 12);
                assert_eq!(source, LiveFrameSource::ShellHint);
            }
            _ => panic!("expected Activate"),
        }
    }

    #[test]
    fn heuristic_ignored_below_threshold() {
        let d = TuiDetector::default();
        let e = d.on_cursor_up(10, 1);
        assert!(matches!(e, TuiEffect::None));
    }

    #[test]
    fn heuristic_fires_at_threshold() {
        let d = TuiDetector::default();
        let e = d.on_cursor_up(10, 3);
        match e {
            TuiEffect::Activate {
                start_origin,
                height,
                source,
            } => {
                assert_eq!(start_origin, 7);
                assert_eq!(height, 4);
                assert_eq!(source, LiveFrameSource::Heuristic);
            }
            _ => panic!("expected Activate"),
        }
    }

    #[test]
    fn heuristic_disabled_by_strict_protocol() {
        let d = TuiDetector::new(TuiAwareness::StrictProtocol);
        let e = d.on_cursor_up(10, 5);
        assert!(matches!(e, TuiEffect::None));
    }

    #[test]
    fn off_policy_drops_everything() {
        let d = TuiDetector::new(TuiAwareness::Off);
        assert!(matches!(d.on_shell_hint(0, 1), TuiEffect::None));
        assert!(matches!(d.on_sync_update_begin(0), TuiEffect::None));
        assert!(matches!(d.on_cursor_up(10, 5), TuiEffect::None));
    }

    #[test]
    fn effect_apply_installs_region_on_block() {
        let mut block = ActiveBlock::new(10);
        let d = TuiDetector::default();
        d.on_sync_update_begin(3).apply(&mut block);
        assert_eq!(
            block.live_frame().unwrap().source,
            LiveFrameSource::SyncUpdate
        );
    }

    #[test]
    fn sync_update_commit_snaps_height_and_bumps_reprint() {
        let mut block = ActiveBlock::new(10);
        let d = TuiDetector::default();
        d.on_sync_update_begin(5).apply(&mut block);
        assert_eq!(block.live_frame().unwrap().height, 1);
        d.on_sync_update_end(9).apply(&mut block);
        let lf = block.live_frame().unwrap();
        assert_eq!(lf.height, 5); // 9 - 5 + 1
        assert_eq!(lf.reprint_count, 1);
    }

    #[test]
    fn idle_tick_on_heuristic_region_clears_after_timeout() {
        let mut block = ActiveBlock::new(10);
        let d = TuiDetector::default();
        d.on_cursor_up(10, 3).apply(&mut block);
        assert!(block.live_frame().is_some());
        let after = Instant::now() + std::time::Duration::from_millis(1_000);
        d.on_idle_tick(after).apply(&mut block);
        assert!(block.live_frame().is_none());
    }
}
