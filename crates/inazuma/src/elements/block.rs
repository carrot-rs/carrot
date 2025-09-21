mod anchor;
mod element;
mod kind;
mod layout;
mod lifecycle;
mod metadata;
mod state;

#[cfg(test)]
mod tests;

pub use anchor::{BlockConfig, BlockMeasuringBehavior, ScrollBehavior, VisualAnchor};
pub use element::{Block, BlockPrepaintState, blocks};
pub use kind::BlockKind;
pub use lifecycle::BlockLifecycle;
pub use metadata::{BlockMetadata, SemanticTag};
pub use state::{BlockId, BlockIdRange, BlockOffset, BlockState, FoldState};
