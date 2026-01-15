//! Cmdline ↔ block mount state machine.
//!
//! The cmdline surface is a single entity that moves between
//! "mount points" depending on the terminal's current state:
//!
//! 1. **Bottom**: the cmdline sits at the bottom of the viewport,
//!    attached to the upcoming / active prompt block. Default idle
//!    position.
//! 2. **Inside block**: the cmdline lives *inside* the active
//!    block when the running command asks for interactive input
//!    (sudo / ssh / git-credential). Matches `PromptState::Interactive`.
//! 3. **Unmounted**: no block owns the cmdline. Used briefly
//!    during transitions and during non-shell windows (settings,
//!    agent panel).
//!
//! This module owns the state machine that drives those
//! transitions. `BlockId` is abstracted as an opaque `u64` here to
//! avoid pulling `inazuma::block` — the glue layer maps it to the
//! real `BlockId` type.

use std::num::NonZeroU64;

use inazuma::{BlockId, BlockLifecycle};

/// Opaque block identifier. `inazuma::block::BlockId` maps to this
/// via [`BlockHandle::from_block_id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockHandle(pub NonZeroU64);

impl BlockHandle {
    pub fn new(id: u64) -> Option<Self> {
        NonZeroU64::new(id).map(BlockHandle)
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }

    /// Convert from an `inazuma::block::BlockId`. `BlockId(0)` is used
    /// as a sentinel for "no block" in the block primitive, so it
    /// maps to `None` here rather than a handle.
    pub fn from_block_id(id: BlockId) -> Option<Self> {
        Self::new(id.0)
    }

    /// Convert back to an `inazuma::block::BlockId`.
    pub fn to_block_id(self) -> BlockId {
        BlockId(self.get())
    }
}

/// Where the cmdline currently lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MountSite {
    /// Not attached to any block. Transitional / non-shell state.
    #[default]
    Unmounted,
    /// Attached at the bottom of the viewport, about to spawn a new
    /// block or already composing into a brand-new one.
    Bottom(BlockHandle),
    /// Mounted inside the given block (Interactive state).
    Inside(BlockHandle),
}

impl MountSite {
    pub fn block(&self) -> Option<BlockHandle> {
        match self {
            MountSite::Unmounted => None,
            MountSite::Bottom(b) | MountSite::Inside(b) => Some(*b),
        }
    }

    pub fn is_inside(&self) -> bool {
        matches!(self, MountSite::Inside(_))
    }

    pub fn is_bottom(&self) -> bool {
        matches!(self, MountSite::Bottom(_))
    }

    pub fn is_mounted(&self) -> bool {
        !matches!(self, MountSite::Unmounted)
    }
}

/// One-step transitions the cmdline mount can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MountTransition {
    /// Attach at the bottom of the viewport for `block`.
    AttachBottom(BlockHandle),
    /// Move into `block` (interactive prompt detected).
    DescendInto(BlockHandle),
    /// Leave the block we were inside; return to bottom of the
    /// *same* block (interactive prompt concluded but block still
    /// running).
    AscendToBottom,
    /// Detach completely — no block, no cmdline. Non-shell windows
    /// / shutdown.
    Detach,
}

/// State machine guarding mount transitions.
///
/// Invalid transitions (e.g. DescendInto when we aren't mounted
/// on that block at all) return the site unchanged.
#[derive(Debug, Clone, Default)]
pub struct MountController {
    site: MountSite,
}

impl MountController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn site(&self) -> MountSite {
        self.site
    }

    /// Apply `transition`, returning `true` when the site actually
    /// changed. Rejected transitions are no-ops.
    pub fn apply(&mut self, transition: MountTransition) -> bool {
        let before = self.site;
        self.site = self.next_site(transition);
        self.site != before
    }

    fn next_site(&self, transition: MountTransition) -> MountSite {
        use MountSite::*;
        use MountTransition::*;
        match (self.site, transition) {
            (_, AttachBottom(b)) => Bottom(b),
            (Bottom(current), DescendInto(b)) if current == b => Inside(b),
            (Inside(current), AscendToBottom) => Bottom(current),
            (_, Detach) => Unmounted,
            _ => self.site,
        }
    }

    /// Translate an inazuma `BlockLifecycle` event for `block_id` into
    /// a mount transition and apply it. Returns `true` if the mount
    /// site actually changed.
    ///
    /// - [`BlockLifecycle::PromptStart`] / [`BlockLifecycle::InputStart`]
    ///   attach the cmdline at the bottom of the block. `InputStart`
    ///   is idempotent when we're already there — that's the common
    ///   case and it returns `false`.
    /// - [`BlockLifecycle::CommandStart`] is a no-op: the cmdline stays
    ///   at the bottom of the running block until an interactive
    ///   prompt descends it (handled separately via
    ///   [`PromptState`](crate::prompt_state::PromptState)).
    /// - [`BlockLifecycle::CommandEnd`] is also a no-op here; the
    ///   cmdline stays parked on the finishing block until the next
    ///   `PromptStart` fires and re-attaches it to the new one.
    ///
    /// Callers pass the `BlockId` from the inazuma primitive; a
    /// `BlockId(0)` sentinel is rejected and returns `false`.
    pub fn apply_lifecycle(&mut self, block_id: BlockId, event: BlockLifecycle) -> bool {
        let Some(handle) = BlockHandle::from_block_id(block_id) else {
            return false;
        };
        match event {
            BlockLifecycle::PromptStart | BlockLifecycle::InputStart => {
                self.apply(MountTransition::AttachBottom(handle))
            }
            BlockLifecycle::CommandStart | BlockLifecycle::CommandEnd { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(n: u64) -> BlockHandle {
        BlockHandle::new(n).expect("non-zero id")
    }

    #[test]
    fn handle_rejects_zero_id() {
        assert!(BlockHandle::new(0).is_none());
        assert_eq!(handle(42).get(), 42);
    }

    #[test]
    fn default_site_is_unmounted() {
        let c = MountController::new();
        assert_eq!(c.site(), MountSite::Unmounted);
        assert!(!c.site().is_mounted());
    }

    #[test]
    fn attach_bottom_moves_to_bottom() {
        let mut c = MountController::new();
        let b = handle(1);
        assert!(c.apply(MountTransition::AttachBottom(b)));
        assert_eq!(c.site(), MountSite::Bottom(b));
        assert!(c.site().is_bottom());
        assert!(c.site().is_mounted());
        assert_eq!(c.site().block(), Some(b));
    }

    #[test]
    fn descend_only_succeeds_when_bottom_on_same_block() {
        let mut c = MountController::new();
        let b = handle(1);
        c.apply(MountTransition::AttachBottom(b));
        assert!(c.apply(MountTransition::DescendInto(b)));
        assert!(c.site().is_inside());
    }

    #[test]
    fn descend_to_different_block_is_rejected() {
        let mut c = MountController::new();
        let b = handle(1);
        let other = handle(2);
        c.apply(MountTransition::AttachBottom(b));
        assert!(!c.apply(MountTransition::DescendInto(other)));
        assert_eq!(c.site(), MountSite::Bottom(b));
    }

    #[test]
    fn descend_from_unmounted_is_rejected() {
        let mut c = MountController::new();
        let b = handle(7);
        assert!(!c.apply(MountTransition::DescendInto(b)));
        assert_eq!(c.site(), MountSite::Unmounted);
    }

    #[test]
    fn ascend_from_inside_returns_to_bottom_of_same_block() {
        let mut c = MountController::new();
        let b = handle(3);
        c.apply(MountTransition::AttachBottom(b));
        c.apply(MountTransition::DescendInto(b));
        assert!(c.apply(MountTransition::AscendToBottom));
        assert_eq!(c.site(), MountSite::Bottom(b));
    }

    #[test]
    fn ascend_from_bottom_is_noop() {
        let mut c = MountController::new();
        let b = handle(3);
        c.apply(MountTransition::AttachBottom(b));
        assert!(!c.apply(MountTransition::AscendToBottom));
        assert_eq!(c.site(), MountSite::Bottom(b));
    }

    #[test]
    fn detach_always_returns_to_unmounted() {
        let mut c = MountController::new();
        let b = handle(5);
        c.apply(MountTransition::AttachBottom(b));
        assert!(c.apply(MountTransition::Detach));
        assert_eq!(c.site(), MountSite::Unmounted);
    }

    #[test]
    fn attach_new_block_overwrites_previous_site() {
        let mut c = MountController::new();
        let b1 = handle(1);
        let b2 = handle(2);
        c.apply(MountTransition::AttachBottom(b1));
        c.apply(MountTransition::DescendInto(b1));
        assert!(c.apply(MountTransition::AttachBottom(b2)));
        assert_eq!(c.site(), MountSite::Bottom(b2));
    }

    #[test]
    fn unmounted_block_accessor_returns_none() {
        assert_eq!(MountSite::Unmounted.block(), None);
    }

    #[test]
    fn is_mounted_predicates_cover_every_variant() {
        let b = handle(1);
        assert!(!MountSite::Unmounted.is_mounted());
        assert!(MountSite::Bottom(b).is_mounted());
        assert!(MountSite::Inside(b).is_mounted());
        assert!(MountSite::Bottom(b).is_bottom());
        assert!(!MountSite::Bottom(b).is_inside());
        assert!(MountSite::Inside(b).is_inside());
        assert!(!MountSite::Inside(b).is_bottom());
    }

    #[test]
    fn prompt_start_attaches_cmdline_to_new_block() {
        let mut c = MountController::new();
        let id = BlockId(7);
        assert!(c.apply_lifecycle(id, BlockLifecycle::PromptStart));
        assert_eq!(c.site(), MountSite::Bottom(BlockHandle::new(7).unwrap()));
    }

    #[test]
    fn input_start_is_idempotent_when_already_attached() {
        let mut c = MountController::new();
        let id = BlockId(3);
        c.apply_lifecycle(id, BlockLifecycle::PromptStart);
        // Input-start fires right after prompt-start and doesn't
        // change mount state — returns false.
        assert!(!c.apply_lifecycle(id, BlockLifecycle::InputStart));
        assert_eq!(c.site(), MountSite::Bottom(BlockHandle::new(3).unwrap()));
    }

    #[test]
    fn command_start_and_end_are_mount_no_ops() {
        let mut c = MountController::new();
        let id = BlockId(9);
        c.apply_lifecycle(id, BlockLifecycle::PromptStart);
        assert!(!c.apply_lifecycle(id, BlockLifecycle::CommandStart));
        assert!(!c.apply_lifecycle(id, BlockLifecycle::CommandEnd { exit_code: 0 }));
        // Mount stayed at bottom of block 9 throughout.
        assert_eq!(c.site(), MountSite::Bottom(BlockHandle::new(9).unwrap()));
    }

    #[test]
    fn lifecycle_with_zero_block_id_is_rejected() {
        let mut c = MountController::new();
        assert!(!c.apply_lifecycle(BlockId(0), BlockLifecycle::PromptStart));
        assert_eq!(c.site(), MountSite::Unmounted);
    }

    #[test]
    fn prompt_start_on_new_block_re_attaches() {
        // Block 1 prompts, command runs, block 1 ends; then block 2
        // prompts → cmdline migrates to block 2's bottom.
        let mut c = MountController::new();
        c.apply_lifecycle(BlockId(1), BlockLifecycle::PromptStart);
        c.apply_lifecycle(BlockId(1), BlockLifecycle::CommandStart);
        c.apply_lifecycle(BlockId(1), BlockLifecycle::CommandEnd { exit_code: 0 });
        assert!(c.apply_lifecycle(BlockId(2), BlockLifecycle::PromptStart));
        assert_eq!(c.site(), MountSite::Bottom(BlockHandle::new(2).unwrap()));
    }

    #[test]
    fn block_handle_from_block_id_roundtrips() {
        let original = BlockId(42);
        let handle = BlockHandle::from_block_id(original).expect("non-zero");
        assert_eq!(handle.to_block_id(), original);
    }

    #[test]
    fn block_handle_from_zero_block_id_returns_none() {
        assert!(BlockHandle::from_block_id(BlockId(0)).is_none());
    }
}
