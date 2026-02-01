//! Rendering surface for the vertical tabs panel.
//!
//! Everything that turns panel state into an `IntoElement` lives
//! here, grouped by concern:
//!
//! - [`header`] — control bar at the top of the panel (search
//!   input, settings popover, new-session button).
//! - [`row_data`] — pure data resolver: builds one
//!   `TabRowData` per row from the cached sessions + settings.
//!   No element construction.
//! - [`row`] — element construction for a single row (group
//!   header, inline rename, full card with chip + drag wrapper,
//!   and the Panes-mode outer pane wrapper).
//! - [`drag`] — drag payload + floating drag view used by
//!   drag-and-drop reordering.
//!
//! The `Render` impl for `VerticalTabsPanel` lives in
//! `vertical_tabs.rs` and is a thin loop that resolves
//! `TabRowData`s then calls `build_row` for each.

pub mod drag;
pub mod header;
pub mod row;
pub mod row_data;
