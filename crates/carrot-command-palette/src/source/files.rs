//! Streaming filesystem search. Spawns a bounded `ignore::WalkBuilder`
//! traversal in a background thread, drains its channel non-blocking into
//! a per-scope pool, and fuzzy-matches the current pool against the user's
//! query. Results appear as the walker progresses so the modal feels
//! live even on million-file home directories.

mod live_walk_cache;
mod live_walker;
mod path_render;
mod source;

pub use live_walk_cache::init;
pub(crate) use path_render::split_path_positions;
pub use source::{FilesSource, FilesSourceStatus};
