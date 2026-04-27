//! Carrot Grid — pure terminal-cell data structure.
//!
//! Layer 1 of the Ultimate Block System. Zero dependencies on UI, GPU, or VT.
//! Provides three primitives:
//!
//! - [`Cell`] — 8-byte packed struct, 3-bit tag, u16 style id, flag bits.
//! - [`CellStyleAtlas`] — interned cell styles, cells carry only a `u16` id.
//! - [`PageList`] — doubly-linked 4 KB mmap-aligned pages, O(1) append,
//!   O(1) scrollback prune via page recycling.
//!
//! See plan section A1–A4 for the full contract. This crate is consumed by
//! `carrot-term` (Layer 2) for VT state, and by `carrot-block-render`
//! (Layer 4) for direct GPU reads.

pub mod cell;
pub mod cell_id;
pub mod color;
pub mod compress;
pub mod coordinates;
pub mod custom_render;
pub mod grapheme;
pub mod hyperlink;
pub mod image;
pub mod image_protocols;
pub mod page;
pub mod page_list;
pub mod reflow;
pub mod search;
pub mod snapshot;
pub mod style;
pub mod tabstops;

pub use cell::{Cell, CellFlags, CellStyleId, CellTag, GraphemeIndex, ImageIndex, ShapedRunIndex};
pub use cell_id::{CellId, CellIdRow, CellIdSpan};
pub use color::{Color, NamedColor, rgb_to_oklch};
pub use compress::{CompressError, CompressedCells, compress, decompress};
pub use coordinates::{GridBounds, RowAddr};
pub use custom_render::{
    CustomDraw, CustomRenderIndex, CustomRenderRect, CustomRenderRegistry, CustomRenderer,
};
pub use grapheme::GraphemeStore;
pub use hyperlink::{HyperlinkId, HyperlinkStore};
pub use image::{DecodedImage, ImageEntry, ImageFormat, ImageStore, Placement};
pub use image_protocols::{
    ITerm2Image, decode_image_bytes, decode_sixel, parse_iterm2_payload, parse_kitty_payload,
    placement_from_iterm2,
};
pub use page::{Page, PageCapacity};
pub use page_list::{PageList, RowIter};
pub use reflow::{
    ReflowDriver, ReflowProgress, ReflowReason, ReflowRequest, SyncReflowDriver,
    ThreadedReflowDriver,
};
pub use search::{SearchMatch, SearchOptions, search_cells};
pub use snapshot::BlockSnapshot;
pub use style::{CellStyle, CellStyleAtlas, CellStyleFlags};
pub use tabstops::{INITIAL_TABSTOPS, TabStops};
