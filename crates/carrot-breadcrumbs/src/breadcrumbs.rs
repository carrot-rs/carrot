use carrot_ui::prelude::*;
use carrot_workspace::{
    ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView,
    item::{HighlightedText, ItemEvent, ItemHandle},
    workspace_settings::ToolbarSettings,
};
use inazuma::{
    AnyElement, App, Context, EventEmitter, Font, Global, IntoElement, Render, SharedString,
    Subscription, Window,
};
use inazuma_settings_framework::Settings;

type RenderBreadcrumbTextFn = fn(
    Vec<HighlightedText>,
    Option<Font>,
    Option<AnyElement>,
    &dyn ItemHandle,
    bool,
    &mut Window,
    &App,
) -> AnyElement;

pub struct RenderBreadcrumbText(pub RenderBreadcrumbTextFn);

impl Global for RenderBreadcrumbText {}

pub fn init(cx: &mut App) {
    // Set default breadcrumb renderer if none is registered yet.
    // carrot-editor can override this with a richer renderer via set_global.
    if cx.try_global::<RenderBreadcrumbText>().is_none() {
        cx.set_global(RenderBreadcrumbText(default_render_breadcrumb_text));
    }
    // Toolbar registration is handled by initialize_pane() in carrot-app,
    // using the centralized pane toolbar setup pattern.
}

fn default_render_breadcrumb_text(
    segments: Vec<HighlightedText>,
    _font: Option<Font>,
    prefix: Option<AnyElement>,
    _item: &dyn ItemHandle,
    _is_tab_bar: bool,
    _window: &mut Window,
    cx: &App,
) -> AnyElement {
    let mut container = h_flex().gap_1();

    if let Some(prefix_el) = prefix {
        container = container.child(prefix_el);
    }

    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            container = container.child(
                div()
                    .child(SharedString::from(" › "))
                    .text_color(cx.theme().colors().text_muted),
            );
        }
        container = container.child(
            div()
                .child(segment.text.clone())
                .text_color(cx.theme().colors().text),
        );
    }

    container.into_any_element()
}

pub struct Breadcrumbs {
    pane_focused: bool,
    active_item: Option<Box<dyn ItemHandle>>,
    subscription: Option<Subscription>,
}

impl Default for Breadcrumbs {
    fn default() -> Self {
        Self::new()
    }
}

impl Breadcrumbs {
    pub fn new() -> Self {
        Self {
            pane_focused: false,
            active_item: Default::default(),
            subscription: Default::default(),
        }
    }
}

impl EventEmitter<ToolbarItemEvent> for Breadcrumbs {}

impl Render for Breadcrumbs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let element = h_flex()
            .id("breadcrumb-container")
            .flex_grow()
            .h_8()
            .overflow_x_scroll()
            .text_ui(cx);

        let Some(active_item) = self.active_item.as_ref() else {
            return element.into_any_element();
        };

        let Some((segments, breadcrumb_font)) = active_item.breadcrumbs(cx) else {
            return element.into_any_element();
        };

        let prefix_element = active_item.breadcrumb_prefix(window, cx);

        if let Some(render_fn) = cx.try_global::<RenderBreadcrumbText>() {
            (render_fn.0)(
                segments,
                breadcrumb_font,
                prefix_element,
                active_item.as_ref(),
                false,
                window,
                cx,
            )
        } else {
            element.into_any_element()
        }
    }
}

impl ToolbarItemView for Breadcrumbs {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        cx.notify();
        self.active_item = None;

        if !ToolbarSettings::get_global(cx).breadcrumbs {
            return ToolbarItemLocation::Hidden;
        }

        let Some(item) = active_pane_item else {
            return ToolbarItemLocation::Hidden;
        };

        let this = cx.entity().downgrade();
        self.subscription = Some(item.subscribe_to_item_events(
            window,
            cx,
            Box::new(move |event, _, cx| {
                if let ItemEvent::UpdateBreadcrumbs = event {
                    this.update(cx, |this, cx| {
                        cx.notify();
                        if let Some(active_item) = this.active_item.as_ref() {
                            cx.emit(ToolbarItemEvent::ChangeLocation(
                                active_item.breadcrumb_location(cx),
                            ))
                        }
                    })
                    .ok();
                }
            }),
        ));
        self.active_item = Some(item.boxed_clone());
        item.breadcrumb_location(cx)
    }

    fn pane_focus_update(
        &mut self,
        pane_focused: bool,
        _window: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.pane_focused = pane_focused;
    }
}
