//! Query parser — splits the user's raw input into path-fuzzy-match query
//! and optional `:row:col` position suffix.

use inazuma_util::paths::PathWithPosition;

#[derive(Debug, Clone)]
pub(crate) struct FileSearchQuery {
    pub(crate) raw_query: String,
    pub(crate) file_query_end: Option<usize>,
    pub(crate) path_position: PathWithPosition,
}

impl FileSearchQuery {
    pub(crate) fn path_query(&self) -> &str {
        match self.file_query_end {
            Some(file_path_end) => &self.raw_query[..file_path_end],
            None => &self.raw_query,
        }
    }
}
