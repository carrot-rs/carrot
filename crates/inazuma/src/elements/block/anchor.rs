use crate::{Pixels, px};

/// Visual alignment of block content inside the element when the content
/// is shorter than the viewport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VisualAnchor {
    /// Content fills from the top; empty space at the bottom when short.
    #[default]
    Top,
    /// Content anchored to the bottom; empty space at the top when short.
    /// Terminal-style layout — new blocks grow upward from the viewport
    /// bottom, old ones push up and eventually out into the fold-area.
    Bottom,
}

/// How the scroll position responds when new entries are appended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollBehavior {
    /// Scroll sticks to the tail — new entries keep themselves visible.
    #[default]
    FollowTail,
    /// Scroll is controlled only by explicit calls.
    Manual,
}

/// Strategy for measuring entry heights.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlockMeasuringBehavior {
    /// Measure only entries in the visible range (+ overdraw). Default.
    #[default]
    Visible,
    /// Measure every entry on first paint — used when the consumer needs
    /// an accurate scrollbar thumb size even for off-screen entries.
    All,
}

/// Configuration for a `BlockState`. Builder-style; clone-copy friendly.
#[derive(Clone, Copy, Debug)]
pub struct BlockConfig {
    pub(super) visual_anchor: VisualAnchor,
    pub(super) scroll_behavior: ScrollBehavior,
    pub(super) measuring_behavior: BlockMeasuringBehavior,
    pub(super) overdraw: Pixels,
}

impl Default for BlockConfig {
    fn default() -> Self {
        Self {
            visual_anchor: VisualAnchor::default(),
            scroll_behavior: ScrollBehavior::default(),
            measuring_behavior: BlockMeasuringBehavior::default(),
            overdraw: px(200.0),
        }
    }
}

impl BlockConfig {
    /// Set the visual anchor (how content is aligned when shorter than viewport).
    pub fn visual_anchor(mut self, anchor: VisualAnchor) -> Self {
        self.visual_anchor = anchor;
        self
    }

    /// Set the scroll-follow behaviour for new entries.
    pub fn scroll_behavior(mut self, behavior: ScrollBehavior) -> Self {
        self.scroll_behavior = behavior;
        self
    }

    /// Set the measuring strategy (Visible vs All).
    pub fn measuring_behavior(mut self, behavior: BlockMeasuringBehavior) -> Self {
        self.measuring_behavior = behavior;
        self
    }

    /// Set how many extra pixels are rendered above and below the visible
    /// range to keep scrolling smooth. Default 200 px.
    pub fn overdraw(mut self, overdraw: Pixels) -> Self {
        self.overdraw = overdraw;
        self
    }
}
