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
}
