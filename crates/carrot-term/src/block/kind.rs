//! `BlockKind` — typed lifecycle marker, **not** content type.
//!
//! Every block in the router is one of two kinds:
//!
//! - `Shell` — append-only output stream from a regular shell command.
//!   The grid grows monotonically, never overwritten in place. Cursor
//!   lives in `carrot-cmdline` (the input editor) for the prompt phase
//!   and stays detached during command execution.
//!
//! - `Tui` — alt-screen / live-redraw session (vim, htop, claude inside
//!   carrot, …). The grid is rewritten in place from a [`LiveFrameRegion`].
//!   VT-cursor state lives in `carrot-term` and is painted by Layer 4
//!   from the term state at render time.
//!
//! `BlockKind` is **sticky**: a block starts as `Shell`, and the first
//! TUI signal (DEC 2026 sync update, OSC 7777 shell hint, cursor-up
//! heuristic) promotes it to `Tui` for the rest of its lifetime. The
//! kind never reverts — even if the heuristic resets and the
//! `live_frame` slot is cleared, a block that ever rendered TUI
//! frames is still a TUI block. That's what "lifecycle semantics, not
//! content type" means.
//!
//! `BlockKind` is **not** a content discriminator. Images
//! (Kitty / Sixel / iTerm2) and custom-render plugins live as Cell
//! tags 4 / 6 inside *any* block — a Shell block can hold images, a
//! TUI block can hold images. Adding `BlockKind::Image` /
//! `BlockKind::AltScreen` etc. is explicitly off the menu (CLAUDE.md
//! Hard Rule).
//!
//! [`LiveFrameRegion`]: crate::block::live_frame::LiveFrameRegion

use serde::{Deserialize, Serialize};

/// Lifecycle semantics of a block.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlockKind {
    /// Append-only shell-output stream. Default for newly-spawned
    /// active blocks.
    #[default]
    Shell,
    /// Alt-screen / live-redraw session. Promoted from `Shell` once
    /// TUI activity is detected; sticky for the block's lifetime.
    Tui,
}

impl BlockKind {
    /// Promote `Shell` → `Tui` (sticky). Calling this on an already-Tui
    /// block is a no-op. Called by the TUI detector when the first
    /// promotion signal fires.
    pub fn promote_to_tui(&mut self) {
        *self = BlockKind::Tui;
    }

    /// `true` for `Tui` blocks. Convenience for renderers that route
    /// shell output inline and TUI frames through the PinnedFooter.
    #[inline]
    pub fn is_tui(self) -> bool {
        matches!(self, BlockKind::Tui)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_shell() {
        assert_eq!(BlockKind::default(), BlockKind::Shell);
    }

    #[test]
    fn promote_is_sticky_and_idempotent() {
        let mut k = BlockKind::default();
        assert!(!k.is_tui());
        k.promote_to_tui();
        assert_eq!(k, BlockKind::Tui);
        // Idempotent — calling again is a no-op.
        k.promote_to_tui();
        assert_eq!(k, BlockKind::Tui);
    }
}
