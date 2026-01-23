use inazuma::{
    AnyElement, ClickEvent, MouseButton, MouseDownEvent, Pixels, Stateful,
    StatefulInteractiveElement,
};
use inazuma_component_registry::{Component, ComponentScope};

use crate::prelude::*;

/// Glass UI list-item primitive. Used for tab rows, file tree entries,
/// outline nodes, git change items, agent messages — anything that lives
/// inside a `FloatingPanel` as a row.
///
/// Default state: transparent. Hover and active states render an opaque
/// rounded background. Each card has its own gap so a list of cards looks
/// like floating chips, not a continuous strip.
///
/// Three slot APIs for trailing content:
/// - [`Card::end_slot`] — inline, always visible (badge, count).
/// - [`Card::end_hover_slot`] — inline, only visible while hovered.
/// - [`Card::overlay`] — absolute-positioned floating chip in the
///   top-right corner with its own background, border and padding.
///   Only visible while hovered. Use this when hover actions should
///   read as a distinct floating UI element rather than an inline slot
///   (session tabs, file tree hover actions, git row actions, agent
///   message toolbar).
///
/// See CLAUDE.md "Design System: Glass UI Pattern".
///
/// # Example
///
/// ```ignore
/// Card::new(("session-tab", index))
///     .start_slot(Icon::new(IconName::Terminal))
///     .title("erscheint nicht")
///     .subtitle("feat/terminal-editor-fusion")
///     .selected(is_active)
///     .overlay(h_flex().child(more_btn).child(close_btn))
///     .on_click(cx.listener(|this, _, w, cx| this.activate(index, w, cx)))
/// ```
#[derive(IntoElement, RegisterComponent)]
pub struct Card {
    id: ElementId,
    selected: bool,
    title: Option<SharedString>,
    subtitle: Option<SharedString>,
    /// Third text line, rendered below subtitle in the same v_flex.
    /// Used by the vertical-tabs expanded-mode row for a longer
    /// description (e.g. full working-directory path) below the
    /// title/branch subtitle. `None` keeps the compact two-line layout.
    description: Option<SharedString>,
    /// Horizontal row of metadata chips/badges rendered below the
    /// description in the same v_flex. Used for +N/-M diff-stats,
    /// PR-link, worktree indicator. `None` = no badge row.
    badges_row: Option<AnyElement>,
    start_slot: Option<AnyElement>,
    end_slot: Option<AnyElement>,
    end_hover_slot: Option<AnyElement>,
    overlay: Option<AnyElement>,
    /// When true, the card skips its normal `.hover()` bg styling. Used by
    /// consumers that render their own hover chip: cursor moving from the
    /// card body into the chip should not leave a stale card-hover bg in
    /// the rest of the row. The consumer tracks chip hover state and
    /// passes `true` for that frame.
    suppress_hover: bool,
    /// When true, the `overlay` chip renders visible regardless of cursor
    /// position — the card's own hover bg remains untouched. Used to keep
    /// the chip icons visible while a popover menu anchored to the chip
    /// is open, without locking the card body into its hover state.
    pin_overlay: bool,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
    on_secondary_mouse_down: Option<Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>>,
    radius: Pixels,
    height: Pixels,
}

impl Card {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            selected: false,
            title: None,
            subtitle: None,
            description: None,
            badges_row: None,
            start_slot: None,
            end_slot: None,
            end_hover_slot: None,
            overlay: None,
            suppress_hover: false,
            pin_overlay: false,
            on_click: None,
            on_secondary_mouse_down: None,
            radius: px(4.),
            height: px(36.),
        }
    }

    /// Mark the card as the active/selected one in its list.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Third text line shown below subtitle in the expanded-mode layout.
    /// Use for a secondary muted description line (e.g. full CWD path)
    /// while subtitle carries the primary secondary info (e.g. branch).
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Horizontal row of small metadata badges rendered below the
    /// description. Each consumer passes an already-assembled `h_flex`
    /// (or any IntoElement) of chips: diff-stats, PR-link, worktree etc.
    /// Card doesn't know what they represent — it just renders them.
    pub fn badges_row(mut self, row: impl IntoElement) -> Self {
        self.badges_row = Some(row.into_any_element());
        self
    }

    /// Slot rendered before the title (icon, avatar, indicator).
    pub fn start_slot(mut self, slot: impl IntoElement) -> Self {
        self.start_slot = Some(slot.into_any_element());
        self
    }

    /// Slot rendered after the title, always visible (badge, count).
    pub fn end_slot(mut self, slot: impl IntoElement) -> Self {
        self.end_slot = Some(slot.into_any_element());
        self
    }

    /// Slot rendered after the title, only visible while hovered. Use this
    /// for inline actions like close × or more ⋮ buttons.
    pub fn end_hover_slot(mut self, slot: impl IntoElement) -> Self {
        self.end_hover_slot = Some(slot.into_any_element());
        self
    }

    /// Suppress the card's hover background for this render. Intended for
    /// use with `.overlay(...)`: when the cursor enters the chip, the
    /// consumer flips this flag so the card body goes back to transparent
    /// and only the chip (with its own bg) stays highlighted.
    pub fn suppress_hover(mut self, suppress: bool) -> Self {
        self.suppress_hover = suppress;
        self
    }

    /// Keep the `overlay` chip rendered even when the card is not hovered.
    /// The card's own hover behaviour stays untouched: cursor enters →
    /// hover bg, cursor leaves → transparent. Only the chip visibility is
    /// pinned. Used to keep the ⋮/× chip visible while a popover menu
    /// anchored to the chip is open.
    pub fn pin_overlay(mut self, pin: bool) -> Self {
        self.pin_overlay = pin;
        self
    }

    /// Floating hover chip anchored to the top-right corner of the card.
    /// The chip has its own elevated-surface background, a subtle border,
    /// and rounded corners — distinct from the card itself. Only visible
    /// while the card is hovered.
    ///
    /// Prefer this over `end_hover_slot` when the hover actions should
    /// read as a separate floating element (recommended for session tab
    /// management, file tree entry actions, git row actions, agent
    /// message toolbars).
    pub fn overlay(mut self, slot: impl IntoElement) -> Self {
        self.overlay = Some(slot.into_any_element());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    pub fn on_secondary_mouse_down(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_secondary_mouse_down = Some(Box::new(handler));
        self
    }

    pub fn radius(mut self, radius: Pixels) -> Self {
        self.radius = radius;
        self
    }

    pub fn height(mut self, height: Pixels) -> Self {
        self.height = height;
        self
    }
}

impl Component for Card {
    fn scope() -> ComponentScope {
        ComponentScope::DataDisplay
    }

    fn description() -> Option<&'static str> {
        Some(
            "Glass UI list-item primitive. Default transparent, hover/active \
             states render rounded opaque backgrounds. Used for all list rows \
             inside FloatingPanel containers — tabs, file tree, outline, etc.",
        )
    }
}

impl RenderOnce for Card {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();
        let title_color = if self.selected {
            colors.text
        } else {
            colors.text_muted
        };
        // Static bg selection. `selected` (the active card in its list)
        // wins over hover — everything else falls through to the dynamic
        // `.hover()` listener below.
        let bg = if self.selected {
            Some(colors.element_selected)
        } else {
            None
        };

        let mut row: Stateful<Div> = h_flex()
            .id(self.id)
            .group("carrot-card")
            .w_full()
            .h(self.height)
            .px_2()
            .gap_2()
            .items_center()
            .rounded(self.radius)
            .cursor_pointer();

        if let Some(bg) = bg {
            row = row.bg(bg);
        }

        // `.hover()` attaches the dynamic hover bg unless the consumer
        // suppresses it (chip underneath has its own hover affordance and
        // wants the card body transparent for that frame).
        if !self.suppress_hover {
            row = row.hover(|el| el.bg(colors.element_hover));
        }
        row = row
            .when_some(self.start_slot, |el, slot| el.child(slot))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .justify_center()
                    .gap_0()
                    .when_some(self.title, |el, title| {
                        el.child(
                            div()
                                .text_size(px(13.))
                                .text_color(title_color)
                                .truncate()
                                .overflow_hidden()
                                .child(title),
                        )
                    })
                    .when_some(self.subtitle, |el, subtitle| {
                        el.child(
                            div()
                                .text_size(px(11.))
                                .text_color(colors.text_muted)
                                .truncate()
                                .overflow_hidden()
                                .child(subtitle),
                        )
                    })
                    .when_some(self.description, |el, description| {
                        el.child(
                            div()
                                .text_size(px(11.))
                                .text_color(colors.text_muted)
                                .truncate()
                                .overflow_hidden()
                                .child(description),
                        )
                    })
                    .when_some(self.badges_row, |el, row| {
                        el.child(div().mt_0p5().child(row))
                    }),
            );

        // End-slot is always visible. End-hover-slot replaces it on hover so
        // the row stays balanced (no width jumping).
        match (self.end_slot, self.end_hover_slot) {
            (Some(persistent), Some(hover)) => {
                row = row.child(
                    div()
                        .flex_shrink_0()
                        .visible()
                        .group_hover("carrot-card", |s| s.invisible())
                        .child(persistent),
                );
                row = row.child(
                    div()
                        .flex_shrink_0()
                        .invisible()
                        .group_hover("carrot-card", |s| s.visible())
                        .child(hover),
                );
            }
            (Some(persistent), None) => {
                row = row.child(div().flex_shrink_0().child(persistent));
            }
            (None, Some(hover)) => {
                row = row.child(
                    div()
                        .flex_shrink_0()
                        .invisible()
                        .group_hover("carrot-card", |s| s.visible())
                        .child(hover),
                );
            }
            (None, None) => {}
        }

        // Floating hover overlay. Anchored to the card's top-right corner,
        // visible only while the card is hovered. The chip has a subtle
        // element-background tint (matching button-like UI) with no border,
        // and no extra padding — the IconButtons inside bring their own
        // sizing and per-icon hover backgrounds, so the chip reads as two
        // button-sized cells sitting on top of the card.
        if let Some(overlay) = self.overlay {
            // `pin_overlay` drops the `group_hover` gating so the chip
            // stays rendered even with the cursor off the card — used
            // while a popover menu anchored to the chip is open.
            let pinned = self.pin_overlay;
            row = row.relative().child(
                div()
                    .absolute()
                    .top_0p5()
                    .right_1()
                    .flex()
                    .items_center()
                    .p_0p5()
                    .rounded(px(4.))
                    // Chip reads as a floating elevated surface above the
                    // panel. Consumers that nest IconButtons inside must
                    // override the buttons' hover bg with a value lighter
                    // than `elevated_surface` so the hover rectangle stays
                    // visible against the chip. The 2px padding keeps each
                    // inner button's hover rectangle inset from the chip
                    // edges, so the hover cell reads as a distinct small
                    // box with air around it.
                    .bg(colors.elevated_surface)
                    .map(|el| {
                        if pinned {
                            el
                        } else {
                            el.invisible().group_hover("carrot-card", |s| s.visible())
                        }
                    })
                    .child(overlay),
            );
        }

        if let Some(handler) = self.on_click {
            row = row.on_click(handler);
        }
        if let Some(handler) = self.on_secondary_mouse_down {
            row = row.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                handler(event, window, cx);
            });
        }

        row
    }
}
