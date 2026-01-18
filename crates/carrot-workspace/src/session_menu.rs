//! Shared session tab context menu builder.
//!
//! Produces the session tab menu used by both the title bar's horizontal
//! tabs (right-click trigger) and the vertical tabs panel's cards (hover
//! ⋮ button trigger). The menu surfaces Share / Rename / Move / Close /
//! Close others / Close to the right, plus six preset color dots.
//!
//! Two consumer concerns are parameterised:
//!
//! - **Orientation** (`SessionMenuVariant`) — horizontal tabs use
//!   "Move tab left/right"; vertical cards use "Move tab up/down". Close
//!   neighbours follows the same axis.
//! - **Rename handling** (`on_rename`) — title bar dispatches the global
//!   `RenameActiveSession` action; vertical tabs passes a callback that
//!   opens an inline input inside the card.
//!
//! Everything else (Share, Close, Close others, color dots) operates on
//! workspace/session APIs directly — no per-consumer customization.

use std::rc::Rc;

use carrot_ui::{ContextMenu, Icon, IconName, IconSize, h_flex, prelude::*};
use inazuma::{App, Entity, Oklch, SharedString, Window, div, oklch, px};

use crate::Workspace;

/// Orientation variant for the session menu. Title bar runs horizontal,
/// vertical tabs runs — well, vertical. The variant only affects the
/// wording of the move/close-neighbour entries; everything else is
/// identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMenuVariant {
    Horizontal,
    Vertical,
}

impl SessionMenuVariant {
    fn move_forward_label(self) -> &'static str {
        match self {
            Self::Horizontal => "Move tab right",
            Self::Vertical => "Move tab down",
        }
    }

    fn move_back_label(self) -> &'static str {
        match self {
            Self::Horizontal => "Move tab left",
            Self::Vertical => "Move tab up",
        }
    }

    fn close_forward_label(self) -> &'static str {
        match self {
            Self::Horizontal => "Close tabs to the right",
            Self::Vertical => "Close tabs below",
        }
    }
}

/// Colors offered by the context menu's color-dot row: red, green,
/// yellow, blue, purple, teal/cyan.
fn tab_color_presets() -> [(Oklch, &'static str); 6] {
    [
        (oklch(0.65, 0.20, 25.0), "Red"),
        (oklch(0.72, 0.18, 150.0), "Green"),
        (oklch(0.80, 0.15, 90.0), "Yellow"),
        (oklch(0.60, 0.18, 260.0), "Blue"),
        (oklch(0.65, 0.20, 310.0), "Purple"),
        (oklch(0.75, 0.15, 190.0), "Cyan"),
    ]
}

/// Build the right-click context menu for the session tab at `index`.
///
/// Visibility rules:
/// - Move tab right → only when not the last tab
/// - Move tab left → only when not the first tab
/// - Close other tabs → only when more than one session exists
/// - Close tabs to the right → only when not the last tab
/// - Reset tab name → only when the session has a user-set name
pub fn build_session_context_menu(
    index: usize,
    workspace: Entity<Workspace>,
    variant: SessionMenuVariant,
    on_rename: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ContextMenu> {
    // Snapshot the session fields the menu renders against. Scoped so the
    // borrow on `workspace` ends before the ContextMenu::build closure
    // takes ownership of the Entity.
    let (session_count, session_color, has_custom_name) = {
        let workspace_ref = workspace.read(cx);
        let sessions = workspace_ref.sessions();
        let count = sessions.len();
        let (color, has_name) = sessions
            .get(index)
            .map(|s| {
                let session = s.read(cx);
                (session.color(), session.name().is_some())
            })
            .unwrap_or((None, false));
        (count, color, has_name)
    };

    ContextMenu::build(window, cx, move |menu, _window, _cx| {
        let ws_share = workspace.clone();
        let ws_reset_name = workspace.clone();
        let ws_move_right = workspace.clone();
        let ws_move_left = workspace.clone();
        let ws_close = workspace.clone();
        let ws_close_others = workspace.clone();
        let ws_close_right = workspace.clone();
        let ws_color = workspace.clone();

        let mut menu = menu
            .entry("Share session", None, move |_window, _cx| {
                // Placeholder — the Share action is wired in Phase 6 polish.
                let _ = &ws_share;
            })
            .separator();

        // Rename uses the optional callback when present, otherwise falls
        // back to the global action. Title bar uses the action; vertical
        // tabs uses a callback so the rename input can appear in the card.
        if let Some(callback) = on_rename.clone() {
            let callback = callback.clone();
            menu = menu.entry("Rename tab", None, move |window, cx| {
                callback(index, window, cx);
            });
        } else {
            menu = menu.entry("Rename tab", None, move |window, cx| {
                window.dispatch_action(Box::new(carrot_actions::session::RenameActiveSession), cx);
            });
        }

        if has_custom_name {
            menu = menu.entry("Reset tab name", None, move |_window, cx| {
                ws_reset_name.update(cx, |ws, cx| {
                    if let Some(session) = ws.sessions().get(index) {
                        session.update(cx, |s, cx| s.set_name(None, cx));
                    }
                });
            });
        }

        if index + 1 < session_count {
            menu = menu.entry(variant.move_forward_label(), None, move |_window, cx| {
                ws_move_right.update(cx, |ws, cx| ws.move_session_right(index, cx));
            });
        }
        if index > 0 {
            menu = menu.entry(variant.move_back_label(), None, move |_window, cx| {
                ws_move_left.update(cx, |ws, cx| ws.move_session_left(index, cx));
            });
        }

        menu = menu
            .separator()
            .entry("Close tab", None, move |window, cx| {
                ws_close.update(cx, |ws, cx| ws.close_session(index, window, cx));
            });

        if session_count > 1 {
            menu = menu.entry("Close other tabs", None, move |window, cx| {
                ws_close_others.update(cx, |ws, cx| ws.close_other_sessions(index, window, cx));
            });
        }
        if index + 1 < session_count {
            menu = menu.entry(variant.close_forward_label(), None, move |window, cx| {
                ws_close_right.update(cx, |ws, cx| ws.close_sessions_to_right(index, window, cx));
            });
        }

        // "Save as new config" lands once launch configurations are
        // implemented. The entry belongs to the vertical variant only —
        // title-bar tabs don't carry this affordance. Add-back point:
        // after TOML session templates ship under ~/.config/carrot/sessions/.

        // Color-dot row. `custom_entry_static` renders raw markup — we draw
        // six 14px circles plus an "unset" overlay on the currently-active
        // dot. Clicking a dot toggles its color on the session; clicking the
        // active dot clears the color.
        let presets = tab_color_presets();
        menu = menu.separator().custom_entry_static(move |_window, _cx| {
            let dot_size = px(14.);

            h_flex()
                .gap_2()
                .py_2()
                .px_3()
                .items_center()
                .justify_center()
                .children(presets.map(|(color, name)| {
                    let ws_dot = ws_color.clone();
                    let is_selected = session_color
                        .map(|c| (c.h - color.h).abs() < 5.0)
                        .unwrap_or(false);
                    div()
                        .id(SharedString::from(format!("session-color-{name}")))
                        .w(dot_size)
                        .h(dot_size)
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(color)
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.7))
                        .when(is_selected, |this| {
                            this.relative().child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::new(IconName::DropletOff)
                                            .size(IconSize::Small)
                                            .text_color(oklch(0.0, 0.0, 0.0).opacity(0.75)),
                                    ),
                            )
                        })
                        .on_click(move |_, window, cx| {
                            ws_dot.update(cx, |ws, cx| {
                                if let Some(session) = ws.sessions().get(index) {
                                    session.update(cx, |s, cx| {
                                        if is_selected {
                                            s.set_color(None, cx);
                                        } else {
                                            s.set_color(Some(color), cx);
                                        }
                                    });
                                }
                            });
                            window.dispatch_action(Box::new(inazuma_menu::Cancel), cx);
                        })
                }))
                .into_any_element()
        });

        menu
    })
}
