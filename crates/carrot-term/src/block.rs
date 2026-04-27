//! Phase-C block state on top of `carrot-grid`.
//!
//! Two-phase lifecycle:
//! - [`ActiveBlock`] — mutable, owns a `PageList` + per-block `CellStyleAtlas`
//!   + `ImageStore`. The VT state machine writes into it on every PTY byte.
//! - [`FrozenBlock`] — immutable `Arc`-wrapped snapshot after `CommandEnd`.
//!   Cheap to clone, never reflowed.
//!
//! This module is the **new** block state. The legacy [`crate::block`]
//! module stays in place until all consumers have migrated — see Phase
//! C migration plan for the cut-over. Both modules can coexist on the
//! branch; they render to different `BlockState` types.
//!
//! [`crate::block`]: crate::block

pub mod active;
pub mod display;
pub mod frozen;
pub mod kind;
pub mod live_frame;
pub mod mode;
pub mod render_view;
pub mod replay;
pub mod router;
pub mod search;
pub mod selection;
pub mod state;
pub mod text;
pub mod tui_detector;
pub mod vt_color;
pub mod vt_report;
pub mod vt_writer;

#[cfg(test)]
pub mod tests;

pub use active::ActiveBlock;
pub use display::{DisplayState, Scroll};
pub use frozen::FrozenBlock;
pub use kind::BlockKind;
pub use live_frame::{LiveFrameRegion, LiveFrameSource};
pub use mode::TermMode;
pub use render_view::{ActiveBlockView, FrozenView, RenderView};
pub use replay::ReplayBuffer;
pub use router::{
    ActiveTarget, BlockId, BlockRouter, RouterBlockMetadata, RouterEntry, RouterLimits,
};
pub use search::BlockSearchMatch;
pub use selection::{BlockSelection, SelectionKind, Side};
pub use state::{BlockState, BlockVariant};
pub use text::{append_cell, append_row, append_row_range, extract_block_lines, extract_block_text};
pub use tui_detector::{TuiAwareness, TuiDetector, TuiEffect};
pub use vt_report::{ModeReportState, REPORT_CHANNEL_CAPACITY, VtReport, VtReportSink};
pub use vt_writer::{VtWriter, VtWriterState};
