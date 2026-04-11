use carrot_workspace::Workspace;
use inazuma::{App, actions};

pub mod svg_preview_view;

pub use carrot_actions::preview::svg::{OpenPreview, OpenPreviewToTheSide};

actions!(
    svg,
    [
        /// Opens a following SVG preview that syncs with the editor.
        OpenFollowingPreview
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        crate::svg_preview_view::SvgPreviewView::register(workspace, window, cx);
    })
    .detach();
}
