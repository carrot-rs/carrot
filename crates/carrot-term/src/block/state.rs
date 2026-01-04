//! Two-state block lifecycle enum.
//!
//! At any moment a block is either:
//! - [`BlockVariant::Active`] — the VT state machine is still writing to it.
//! - [`BlockVariant::Frozen`] — `CommandEnd` was received; it's immutable.
//!
//! The terminal core holds a `Vec<BlockVariant>` (not `BlockState` —
//! that's the top-level wrapper used by tests and consumers that want
//! a single handle).

use std::sync::Arc;

use super::active::ActiveBlock;
use super::frozen::FrozenBlock;

/// A block in its current lifecycle state.
///
/// `Active` is boxed so the enum stays small even though `ActiveBlock`
/// is ~288 B — callers keep `Vec<BlockVariant>` and we don't want
/// `remove(i)` to memmove a fat enum around.
pub enum BlockVariant {
    /// Mutable, actively being written by the VT state machine.
    Active(Box<ActiveBlock>),
    /// Immutable snapshot — `Arc` so clones are cheap and the render
    /// thread can hold a reference without blocking writers.
    Frozen(Arc<FrozenBlock>),
}

impl BlockVariant {
    /// Fresh active block at the given column width.
    pub fn new_active(cols: u16) -> Self {
        Self::Active(Box::new(ActiveBlock::new(cols)))
    }

    /// Is this block still being written?
    pub fn is_active(&self) -> bool {
        matches!(self, BlockVariant::Active(_))
    }

    /// Is this block frozen?
    pub fn is_frozen(&self) -> bool {
        matches!(self, BlockVariant::Frozen(_))
    }

    /// Convenience: total rows regardless of variant.
    pub fn total_rows(&self) -> usize {
        match self {
            BlockVariant::Active(a) => a.total_rows(),
            BlockVariant::Frozen(f) => f.total_rows(),
        }
    }

    /// Borrow as active if it is active.
    pub fn as_active(&self) -> Option<&ActiveBlock> {
        if let BlockVariant::Active(a) = self {
            Some(a.as_ref())
        } else {
            None
        }
    }

    /// Borrow as active mutable if it is active.
    pub fn as_active_mut(&mut self) -> Option<&mut ActiveBlock> {
        if let BlockVariant::Active(a) = self {
            Some(a.as_mut())
        } else {
            None
        }
    }

    /// Borrow as frozen if it is frozen.
    pub fn as_frozen(&self) -> Option<&Arc<FrozenBlock>> {
        if let BlockVariant::Frozen(f) = self {
            Some(f)
        } else {
            None
        }
    }
}

/// Convenience wrapper for tests and callers that want a single opaque
/// handle. The terminal core typically keeps individual `BlockVariant`
/// values inside a `Vec`, not this wrapper.
pub struct BlockState {
    variant: BlockVariant,
}

impl BlockState {
    /// Construct in Active state.
    pub fn new_active(cols: u16) -> Self {
        Self {
            variant: BlockVariant::new_active(cols),
        }
    }

    /// Current variant.
    pub fn variant(&self) -> &BlockVariant {
        &self.variant
    }

    /// Mutable variant — for in-place VT writes.
    pub fn variant_mut(&mut self) -> &mut BlockVariant {
        &mut self.variant
    }

    /// Transition from Active → Frozen. No-op if already frozen.
    ///
    /// Returns `Some(Arc<FrozenBlock>)` on the transition, `None` if the
    /// block was already frozen.
    pub fn finish(
        &mut self,
        exit_code: Option<i32>,
        finished_at: Option<std::time::Instant>,
    ) -> Option<Arc<FrozenBlock>> {
        // Take ownership temporarily by swapping in a placeholder Frozen
        // with empty data. This is only safe because nobody is supposed
        // to observe the state *during* the swap — it's a single-threaded
        // transition invoked from the terminal core.
        let placeholder = BlockVariant::Active(Box::new(ActiveBlock::new(1)));
        let taken = std::mem::replace(&mut self.variant, placeholder);
        match taken {
            BlockVariant::Active(active) => {
                let frozen = active.finish(exit_code, finished_at);
                self.variant = BlockVariant::Frozen(frozen.clone());
                Some(frozen)
            }
            already @ BlockVariant::Frozen(_) => {
                self.variant = already;
                None
            }
        }
    }
}
