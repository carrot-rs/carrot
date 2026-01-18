//! Immutable block after `CommandEnd`.
//!
//! Finished blocks never reflow. Window resize doesn't touch their data.
//! Visual soft-wrap happens at render time based on the current viewport
//! width — the data stays at the columns it was collected at.
//!
//! A `FrozenBlock` is cheap to clone via `Arc`: readers across threads
//! share the same immutable state without any lock.

use std::sync::Arc;
use std::time::Instant;

use carrot_grid::{CellStyle, GraphemeStore, HyperlinkStore, ImageStore, PageList};

use super::active::ActiveMetadata;
use super::replay::ReplayBuffer;

/// Immutable snapshot of a finished block.
///
/// Constructed by [`super::ActiveBlock::finish`]. The inner `PageList`
/// and `ImageStore` are owned (since the active block consumed them);
/// the style atlas becomes an `Arc<[CellStyle]>` to share cheaply with any
/// future GPU upload.
pub struct FrozenBlock {
    grid: PageList,
    atlas: Arc<[CellStyle]>,
    images: ImageStore,
    hyperlinks: HyperlinkStore,
    graphemes: GraphemeStore,
    metadata: ActiveMetadata,
    exit_code: Option<i32>,
    finished_at: Option<Instant>,
    /// PTY byte stream captured while the block was active. Enables
    /// re-rendering on font / theme change without re-running the
    /// command, and debug replay for support tickets.
    replay: ReplayBuffer,
}

impl FrozenBlock {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        grid: PageList,
        atlas: Arc<[CellStyle]>,
        images: ImageStore,
        hyperlinks: HyperlinkStore,
        graphemes: GraphemeStore,
        metadata: ActiveMetadata,
        exit_code: Option<i32>,
        finished_at: Option<Instant>,
        replay: ReplayBuffer,
    ) -> Self {
        Self {
            grid,
            atlas,
            images,
            hyperlinks,
            graphemes,
            metadata,
            exit_code,
            finished_at,
            replay,
        }
    }

    /// Access the frozen page list (immutable).
    pub fn grid(&self) -> &PageList {
        &self.grid
    }

    /// Frozen style atlas. `Arc<[CellStyle]>` is cheap to clone for GPU upload.
    pub fn atlas(&self) -> &Arc<[CellStyle]> {
        &self.atlas
    }

    /// Image entries captured during the command.
    pub fn images(&self) -> &ImageStore {
        &self.images
    }

    /// OSC 8 hyperlink URLs interned during the command. `HyperlinkId`
    /// values on `CellStyle` entries point here.
    pub fn hyperlinks(&self) -> &HyperlinkStore {
        &self.hyperlinks
    }

    /// Grapheme clusters (combiner-extended graphemes, ZWJ sequences)
    /// interned during the command. `GraphemeIndex` values on cells
    /// with `CellTag::Grapheme` point here.
    pub fn graphemes(&self) -> &GraphemeStore {
        &self.graphemes
    }

    /// Metadata captured at command-start.
    pub fn metadata(&self) -> &ActiveMetadata {
        &self.metadata
    }

    /// Exit code, if the block finished successfully.
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Whether the command reported a non-zero exit code.
    pub fn is_error(&self) -> bool {
        matches!(self.exit_code, Some(c) if c != 0)
    }

    /// Wallclock timestamp when `CommandEnd` was received.
    pub fn finished_at(&self) -> Option<Instant> {
        self.finished_at
    }

    /// Running duration, when both timestamps were captured.
    pub fn duration(&self) -> Option<std::time::Duration> {
        match (self.metadata.started_at, self.finished_at) {
            (Some(start), Some(end)) => end.checked_duration_since(start),
            _ => None,
        }
    }

    /// Total rows captured in the frozen grid.
    pub fn total_rows(&self) -> usize {
        self.grid.total_rows()
    }

    /// Access the captured PTY byte stream. Use with a fresh
    /// `VtWriter` + `Processor` to reproduce the block's render state
    /// under a new theme / font / cols setting.
    pub fn replay(&self) -> &ReplayBuffer {
        &self.replay
    }
}
