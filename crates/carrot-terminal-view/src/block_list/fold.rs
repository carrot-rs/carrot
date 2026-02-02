//! Fold-area rendering for the block list.
//!
//! When blocks scroll above the viewport the list pushes them into a
//! fold area — a compact `✓ command (duration)` line per hidden
//! block. This module owns the per-entry rendering plus the fold
//! counter that expands / collapses the stack.

use carrot_theme::Theme;
use inazuma::{
    App, ClickEvent, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};

use crate::block_list::header::BlockHeaderView;
use crate::constants::*;

/// Render a single fold-line for a block that scrolled above the viewport.
///
/// Layout: `[badge] command-text ...                  (duration)`
pub fn render_fold_line(
    header: &BlockHeaderView,
    command: &str,
    index: usize,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (badge, badge_color) = if header.is_running {
        ("●", fold_badge_running(theme))
    } else if header.is_error {
        ("✗", fold_badge_error(theme))
    } else {
        ("✓", fold_badge_success(theme))
    };

    let hover_bg = fold_line_hover_bg(theme);
    let error_bg = fold_line_error_bg(theme);
    let is_error = header.is_error;
    let duration_text: SharedString = header.duration_display().into();
    let command_text: SharedString = command.to_string().into();
    let id = SharedString::from(format!("fold-line-{}", index));

    div()
        .id(id)
        .h(px(FOLD_LINE_HEIGHT))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .px(px(BLOCK_HEADER_PAD_X))
        .gap(px(8.0))
        .when(is_error, |d| d.bg(error_bg))
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .on_click(on_click)
        .child(
            div()
                .text_size(px(11.0))
                .text_color(badge_color)
                .flex_shrink_0()
                .child(badge),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_x_hidden()
                .text_size(px(12.0))
                .text_color(header_command_fg(theme))
                .child(command_text),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(11.0))
                .text_color(header_metadata_fg(theme))
                .child(duration_text),
        )
}

/// Render the fold counter line — clickable to expand/collapse all fold-lines.
///
/// - `hidden_count > 0`: shows "⌃ N more commands above" (click → show all)
/// - `hidden_count == 0`: shows "▾ show less" (click → collapse back to 3)
pub fn render_fold_counter(
    hidden_count: usize,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let text: SharedString = if hidden_count > 0 {
        format!("⌃ {} more commands above", hidden_count).into()
    } else {
        "▾ show less".into()
    };
    let hover_bg = fold_line_hover_bg(theme);

    div()
        .id("fold-counter")
        .h(px(FOLD_COUNTER_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .px(px(BLOCK_HEADER_PAD_X))
        .text_size(px(11.0))
        .text_color(header_metadata_fg(theme))
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .on_click(on_click)
        .child(text)
}
