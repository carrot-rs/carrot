//! Carrot (キャロット) — Terminal emulation core.
//!
//! After the Phase-G migration the crate is v2-only. The legacy
//! `BlockGridRouter` + `Grid<Cell>` + `Handler` stack is gone; the
//! live data model is `carrot-grid::PageList` + the block
//! [`ActiveBlock`] / [`FrozenBlock`] pair.
//!
//! ## Layout
//!
//! | Module | Role |
//! |--------|------|
//! | [`block`] | Block lifecycle on top of `carrot-grid`. Includes the VT writer, parity probe, render view, search, selection, live-frame tracking. |
//! | [`term`] | Thin `Term<T>` handle — router + VT state + colours + config. No more per-grid state. |
//! | [`event`], [`event_loop`], [`thread`], [`tty`] | PTY lifecycle + event dispatch. |
//! | [`simd_scan`] | AVX2 / NEON / scalar control-byte scanner. |
//! | [`index`] | `Line` / `Column` / `Point` / `Side` newtypes consumed by hit-testing + UI glue. |
//!
//! External consumers reach the render view via [`Term::render_view`]
//! and the search path via [`block::BlockRouter::search`]. Frozen
//! blocks travel as `Arc<FrozenBlock>` — zero-copy across threads.

#![warn(rust_2018_idioms, future_incompatible)]
#![deny(clippy::all, clippy::if_not_else, clippy::enum_glob_use)]
#![cfg_attr(clippy, deny(warnings))]

pub mod block;
pub mod event;
pub mod event_loop;
pub mod index;
pub mod simd_scan;
pub mod sync;
pub mod term;
pub mod thread;
pub mod tty;

pub use crate::block::{ActiveBlock, BlockState, BlockVariant, FrozenBlock};
pub use crate::term::Term;
pub use vte;

/// UI-side block handle. Kept as a thin `usize`-newtype so Layer-5
/// consumers that maintain HashMap keys keyed on blocks don't need
/// to widen to `u64`. Construction maps from `BlockId::0` via
/// `BlockId(id.0 as usize)`. Collisions are impossible in practice
/// (u64 is monotonic and a carrot session won't cross 2^63 blocks).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct BlockId(pub usize);

impl From<block::BlockId> for BlockId {
    fn from(id: block::BlockId) -> Self {
        Self(id.0 as usize)
    }
}
