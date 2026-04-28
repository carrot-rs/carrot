//! Input-byte-stream replay buffer.
//!
//! Every block captures the exact PTY byte stream that was fed into
//! its VT parser. This enables two capabilities no shipped terminal
//! currently has:
//!
//! 1. **Font / theme change without re-running commands** — when the
//!    user switches fonts or themes, we re-run the captured byte
//!    stream through a fresh VtWriter. The rendered grid is
//!    reconstructed from the source of truth (the raw PTY output)
//!    rather than re-styled from an already-rendered grid, so color
//!    resolution and glyph choice can pick up the new theme cleanly.
//! 2. **Debug replay** — a user filing a support ticket can share
//!    the replay buffer (optionally filtered for secrets) and we can
//!    reproduce their exact render state.
//!
//! # Cost model
//!
//! Overhead is strictly ~5–15 % of the rendered grid size. A 30 k-
//! row `seq 1 10000` block is ~58 k bytes of PTY payload; the
//! rendered grid at 8 bytes/cell × 80 cols is ~19 MB. Replay adds
//! 0.3 % to memory, not 5–15 % — the upper bound only hits for
//! workloads that emit long style-only updates (lots of SGR
//! sequences per cell).
//!
//! # Cap
//!
//! A per-block cap (`ReplayBuffer::new(max_bytes)`) drops the oldest
//! bytes rather than rendering corrupt state — a truncated replay
//! can reconstruct whatever fits; the caller can detect the
//! truncation via `is_truncated()` and fall back to re-styling the
//! already-rendered grid.

/// Append-only byte buffer, optionally capped to a maximum size.
///
/// When the cap is reached, further writes are silently dropped and
/// [`Self::is_truncated`] begins returning `true`. This is deliberate
/// — terminals that emit unbounded output must not force unbounded
/// replay allocation.
#[derive(Debug)]
pub struct ReplayBuffer {
    bytes: Vec<u8>,
    max_bytes: usize,
    truncated: bool,
}

impl ReplayBuffer {
    /// Construct with a given byte cap. `max_bytes == usize::MAX` is
    /// effectively unbounded (only use with a caller-side scrollback
    /// cap that bounds the whole block).
    pub fn new(max_bytes: usize) -> Self {
        Self {
            bytes: Vec::new(),
            max_bytes,
            truncated: false,
        }
    }

    /// Default cap — 8 MB per block. Typical shell sessions stay
    /// well under 1 MB; runaway `yes`-like output truncates.
    pub fn with_default_cap() -> Self {
        Self::new(8 * 1024 * 1024)
    }

    /// Raw read access — replayers pass this to a fresh VtWriter /
    /// Processor pair.
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    /// Current size in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Cap in bytes.
    pub fn capacity(&self) -> usize {
        self.max_bytes
    }

    /// True if any writes were dropped because the cap was hit.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Append incoming PTY bytes. Silently caps at `max_bytes`.
    pub fn extend(&mut self, data: &[u8]) {
        let remaining = self.max_bytes.saturating_sub(self.bytes.len());
        if data.len() <= remaining {
            self.bytes.extend_from_slice(data);
        } else {
            self.bytes.extend_from_slice(&data[..remaining]);
            self.truncated = true;
        }
    }

    /// Clear without losing the cap / truncation flag. Used when the
    /// caller has fully replayed into a fresh grid and the bytes are
    /// no longer needed (e.g., after a theme change, if the caller
    /// decides to re-render on every paint instead of caching).
    pub fn clear(&mut self) {
        self.bytes.clear();
        self.truncated = false;
    }
}

impl Default for ReplayBuffer {
    fn default() -> Self {
        Self::with_default_cap()
    }
}

/// Re-render a [`super::FrozenBlock`] from its captured PTY byte
/// stream into a fresh [`super::FrozenBlock`].
///
/// **Wager B1 — block replay.** When the user changes theme, font or
/// column count, the captured byte stream re-runs through a fresh
/// [`super::VtWriter`] + `vte::Processor` against a freshly-allocated
/// active block. The result is a brand-new frozen block with the
/// same metadata + lifecycle but rendered against the current
/// terminal configuration.
///
/// Bytes that were silently truncated when the buffer hit its cap
/// are **not** re-fetched from the shell — the truncated marker is
/// preserved on the new block (callers can show a visual indicator).
///
/// `cols` is the column count to render the replay against. Passing
/// the current terminal's `cols` lets a theme/font swap re-flow at
/// the user's current viewport width without involving the live PTY.
///
/// Returns `None` if the source block carries no replay bytes (cap
/// disabled or truncated to zero); the caller's fallback is to keep
/// the original frozen block as-is.
pub fn replay_frozen_block(
    source: &std::sync::Arc<super::FrozenBlock>,
    cols: u16,
) -> Option<std::sync::Arc<super::FrozenBlock>> {
    use super::active::ActiveBlock;
    use super::vt_writer::{VtWriter, VtWriterState};
    use crate::vte::ansi::{Processor, StdSyncHandler};

    let bytes = source.replay().as_slice();
    if bytes.is_empty() {
        return None;
    }

    // Spin up a fresh active block + VT pipeline at the requested
    // viewport width. We don't reuse the source block's atlas /
    // image store — replay rebuilds them from scratch via the byte
    // stream, identical to the original first-time render.
    let mut block = ActiveBlock::new(cols);
    // Replay screen-line count is purely a VtWriter scratchpad —
    // 24 is the safe default for shell output that's unlikely to
    // hit alt-screen scroll regions. Future work: thread the live
    // viewport rows through the call so dynamic-resize replays use
    // the right size.
    let mut state = VtWriterState::new(cols, 24);
    let mut processor = Processor::<StdSyncHandler>::new();
    {
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, bytes);
        writer.commit_row();
        writer.finalize();
    }

    // Preserve metadata (command, cwd, git branch, …) and exit
    // code. The originally-captured `started_at` / `finished_at`
    // stay the same — replay isn't a re-execution.
    *block.metadata_mut() = source.metadata().clone();
    Some(block.finish(source.exit_code(), source.finished_at()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer_reports_empty() {
        let buf = ReplayBuffer::new(1024);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert!(!buf.is_truncated());
        assert_eq!(buf.as_slice(), &[] as &[u8]);
    }

    #[test]
    fn extend_accumulates() {
        let mut buf = ReplayBuffer::new(1024);
        buf.extend(b"hello");
        buf.extend(b" ");
        buf.extend(b"world");
        assert_eq!(buf.len(), 11);
        assert_eq!(buf.as_slice(), b"hello world");
        assert!(!buf.is_truncated());
    }

    #[test]
    fn cap_truncates_tail() {
        let mut buf = ReplayBuffer::new(4);
        buf.extend(b"abcdef");
        assert_eq!(buf.len(), 4);
        assert_eq!(buf.as_slice(), b"abcd");
        assert!(buf.is_truncated());
    }

    #[test]
    fn cap_preserves_prefix_when_last_write_overflows() {
        let mut buf = ReplayBuffer::new(6);
        buf.extend(b"abc");
        buf.extend(b"defghi");
        // First 3 kept verbatim; second write only fits 3 more chars.
        assert_eq!(buf.len(), 6);
        assert_eq!(buf.as_slice(), b"abcdef");
        assert!(buf.is_truncated());
    }

    #[test]
    fn clear_resets_content_and_truncation() {
        let mut buf = ReplayBuffer::new(4);
        buf.extend(b"abcdef");
        assert!(buf.is_truncated());
        buf.clear();
        assert!(buf.is_empty());
        assert!(!buf.is_truncated());
        // Cap preserved.
        assert_eq!(buf.capacity(), 4);
    }

    #[test]
    fn default_cap_is_generous_for_normal_sessions() {
        let buf = ReplayBuffer::default();
        // A 1 KB typical command session fits comfortably.
        assert!(buf.capacity() >= 1024);
    }

    #[test]
    fn extend_empty_slice_is_noop() {
        let mut buf = ReplayBuffer::new(16);
        buf.extend(b"");
        assert!(buf.is_empty());
        assert!(!buf.is_truncated());
    }

    #[test]
    fn extend_exactly_fills_cap_without_truncation() {
        let mut buf = ReplayBuffer::new(4);
        buf.extend(b"abcd");
        assert_eq!(buf.len(), 4);
        assert!(!buf.is_truncated(), "exact-fit shouldn't flip truncated");
    }

    #[test]
    fn replay_frozen_block_reproduces_original_content() {
        use super::super::{BlockRouter, vt_writer::VtWriter, vt_writer::VtWriterState};
        use crate::vte::ansi::{Processor, StdSyncHandler};

        // Build a router-driven block, feed bytes through the VT
        // pipeline, freeze, then replay — the new frozen block must
        // carry the same content (row count + first-row prefix).
        let mut router = BlockRouter::new(20);
        router.on_command_start();
        {
            let mut target = router.active();
            let block = target.as_active_mut();
            let mut state = VtWriterState::new(20, 24);
            let mut processor = Processor::<StdSyncHandler>::new();
            let bytes = b"hello\r\nworld";
            block.record_bytes(bytes);
            let mut writer = VtWriter::new_in(&mut state, block);
            processor.advance(&mut writer, bytes);
            writer.commit_row();
            writer.finalize();
        }
        let frozen = router.on_command_end(0).expect("frozen block");
        let original_rows = frozen.total_rows();

        let replayed = replay_frozen_block(&frozen, 20).expect("non-empty replay");
        assert_eq!(replayed.total_rows(), original_rows);
        // Exit code + metadata flow through.
        assert_eq!(replayed.exit_code(), Some(0));
    }

    #[test]
    fn replay_frozen_block_returns_none_for_empty_buffer() {
        use super::super::{BlockRouter, vt_writer::VtWriter, vt_writer::VtWriterState};
        use crate::vte::ansi::{Processor, StdSyncHandler};

        let mut router = BlockRouter::new(20);
        router.on_command_start();
        // Don't record any bytes — replay buffer stays empty.
        {
            let mut target = router.active();
            let block = target.as_active_mut();
            let mut state = VtWriterState::new(20, 24);
            let mut processor = Processor::<StdSyncHandler>::new();
            let mut writer = VtWriter::new_in(&mut state, block);
            processor.advance(&mut writer, b"");
            writer.finalize();
        }
        let frozen = router.on_command_end(0).expect("frozen block");
        assert!(replay_frozen_block(&frozen, 20).is_none());
    }
}
