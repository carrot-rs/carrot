use std::borrow::Borrow;
use std::rc::Rc;

use crate::prelude::*;
use crate::{Color, ElevationIndex, KeyBinding, Label, LabelSize, Size, h_flex, v_flex};
use inazuma::{
    Action, AnyElement, AnyView, AppContext, FocusHandle, IntoElement, Render, StyleRefinement,
    Styled, prelude::FluentBuilder,
};

#[derive(RegisterComponent)]
pub struct Tooltip {
    title: Title,
    style: StyleRefinement,
    /// Controls text size and container padding. `Medium` is the default
    /// (standard app tooltips); `Small` / `XSmall` render tight compact
    /// tooltips suitable for dense header controls.
    size: Size,
    meta: Option<SharedString>,
    key_binding: Option<KeyBinding>,
    action: Option<(Box<dyn Action>, Option<SharedString>)>,
}

#[derive(Clone, IntoElement)]
enum Title {
    Str(SharedString),
    Callback(Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>),
}

impl From<SharedString> for Title {
    fn from(value: SharedString) -> Self {
        Title::Str(value)
    }
}

impl RenderOnce for Title {
    fn render(self, window: &mut Window, cx: &mut App) -> impl inazuma::IntoElement {
        match self {
            Title::Str(title) => title.into_any_element(),
            Title::Callback(element) => element(window, cx),
        }
    }
}

impl Tooltip {
    pub fn simple(title: impl Into<SharedString>, cx: &mut App) -> AnyView {
        cx.new(|_| Self {
            title: Title::Str(title.into()),
            style: StyleRefinement::default(),
            size: Size::Medium,
            meta: None,
            key_binding: None,
            action: None,
        })
        .into()
    }

    pub fn text(title: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let title = title.into();
        move |_, cx| {
            cx.new(|_| Self {
                title: title.clone().into(),
                style: StyleRefinement::default(),
                size: Size::Medium,
                meta: None,
                key_binding: None,
                action: None,
            })
            .into()
        }
    }

    /// Compact tooltip suitable for dense header controls. Smaller text
    /// and tighter padding than the default `text()` helper.
    pub fn small(title: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let title = title.into();
        move |_, cx| {
            cx.new(|_| Self {
                title: title.clone().into(),
                style: StyleRefinement::default(),
                size: Size::Small,
                meta: None,
                key_binding: None,
                action: None,
            })
            .into()
        }
    }

    pub fn for_action_title<T: Into<SharedString>>(
        title: T,
        action: &dyn Action,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + use<T> {
        let title = title.into();
        let action = action.boxed_clone();
        move |_, cx| {
            cx.new(|cx| Self {
                title: Title::Str(title.clone()),
                style: StyleRefinement::default(),
                size: Size::Medium,
                meta: None,
                key_binding: Some(KeyBinding::for_action(action.as_ref(), cx)),
                action: None,
            })
            .into()
        }
    }

    pub fn for_action_title_in<Str: Into<SharedString>>(
        title: Str,
        action: &dyn Action,
        focus_handle: &FocusHandle,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + use<Str> {
        let title = title.into();
        let action = action.boxed_clone();
        let focus_handle = focus_handle.clone();
        move |_, cx| {
            cx.new(|cx| Self {
                title: Title::Str(title.clone()),
                style: StyleRefinement::default(),
                size: Size::Medium,
                meta: None,
                key_binding: Some(KeyBinding::for_action_in(
                    action.as_ref(),
                    &focus_handle,
                    cx,
                )),
                action: None,
            })
            .into()
        }
    }

    pub fn for_action(
        title: impl Into<SharedString>,
        action: &dyn Action,
        cx: &mut App,
    ) -> AnyView {
        cx.new(|cx| Self {
            title: Title::Str(title.into()),
            style: StyleRefinement::default(),
            size: Size::Medium,
            meta: None,
            key_binding: Some(KeyBinding::for_action(action, cx)),
            action: None,
        })
        .into()
    }

    pub fn for_action_in(
        title: impl Into<SharedString>,
        action: &dyn Action,
        focus_handle: &FocusHandle,
        cx: &mut App,
    ) -> AnyView {
        cx.new(|cx| Self {
            title: title.into().into(),
            style: StyleRefinement::default(),
            size: Size::Medium,
            meta: None,
            key_binding: Some(KeyBinding::for_action_in(action, focus_handle, cx)),
            action: None,
        })
        .into()
    }

    pub fn with_meta(
        title: impl Into<SharedString>,
        action: Option<&dyn Action>,
        meta: impl Into<SharedString>,
        cx: &mut App,
    ) -> AnyView {
        cx.new(|cx| Self {
            title: title.into().into(),
            style: StyleRefinement::default(),
            size: Size::Medium,
            meta: Some(meta.into()),
            key_binding: action.map(|action| KeyBinding::for_action(action, cx)),
            action: None,
        })
        .into()
    }

    pub fn with_meta_in(
        title: impl Into<SharedString>,
        action: Option<&dyn Action>,
        meta: impl Into<SharedString>,
        focus_handle: &FocusHandle,
        cx: &mut App,
    ) -> AnyView {
        cx.new(|cx| Self {
            title: title.into().into(),
            style: StyleRefinement::default(),
            size: Size::Medium,
            meta: Some(meta.into()),
            key_binding: action.map(|action| KeyBinding::for_action_in(action, focus_handle, cx)),
            action: None,
        })
        .into()
    }

    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into().into(),
            style: StyleRefinement::default(),
            size: Size::Medium,
            meta: None,
            key_binding: None,
            action: None,
        }
    }

    pub fn new_element(title: impl Fn(&mut Window, &mut App) -> AnyElement + 'static) -> Self {
        Self {
            title: Title::Callback(Rc::new(title)),
            style: StyleRefinement::default(),
            size: Size::Medium,
            meta: None,
            key_binding: None,
            action: None,
        }
    }

    pub fn element(
        title: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let title = Title::Callback(Rc::new(title));
        move |_, cx| {
            let title = title.clone();
            cx.new(|_| Self {
                title,
                style: StyleRefinement::default(),
                size: Size::Medium,
                meta: None,
                key_binding: None,
                action: None,
            })
            .into()
        }
    }

    /// Override the size. Controls text size + container padding.
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    pub fn meta(mut self, meta: impl Into<SharedString>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    pub fn key_binding(mut self, key_binding: impl Into<Option<KeyBinding>>) -> Self {
        self.key_binding = key_binding.into();
        self
    }

    /// Set an Action to auto-resolve key binding display in the tooltip.
    pub fn action(mut self, action: &dyn Action, context: Option<&str>) -> Self {
        self.action = Some((action.boxed_clone(), context.map(SharedString::new)));
        self
    }

    /// Build the tooltip and return it as an `AnyView`.
    pub fn build(self, _: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|_| self).into()
    }
}

impl FluentBuilder for Tooltip {}

impl Styled for Tooltip {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let size = self.size;
        tooltip_container_sized(cx, size, |el, _| {
            el.child(
                h_flex()
                    .gap_4()
                    .child(div().max_w_72().child(self.title.clone()))
                    .when_some(self.key_binding.clone(), |this, key_binding| {
                        this.justify_between().child(key_binding)
                    }),
            )
            .when_some(self.meta.clone(), |this, meta| {
                this.child(
                    div()
                        .max_w_72()
                        .child(Label::new(meta).size(LabelSize::Small).color(Color::Muted)),
                )
            })
        })
    }
}

pub fn tooltip_container<C>(cx: &mut C, f: impl FnOnce(Div, &mut C) -> Div) -> impl IntoElement
where
    C: AppContext + Borrow<App>,
{
    tooltip_container_sized(cx, Size::Medium, f)
}

/// Size-aware tooltip container. `Medium` is the app default; `Small` and
/// `XSmall` render with tighter padding and smaller text so tooltips on
/// dense header controls don't feel oversized.
pub fn tooltip_container_sized<C>(
    cx: &mut C,
    size: Size,
    f: impl FnOnce(Div, &mut C) -> Div,
) -> impl IntoElement
where
    C: AppContext + Borrow<App>,
{
    let app = (*cx).borrow();
    let ui_font = carrot_theme::theme_settings(app).ui_font(app).clone();
    let bg = app.theme().colors().elevated_surface;
    let shadow = ElevationIndex::ElevatedSurface.shadow(app);

    // Outer div's padding doubles as the offset from the anchor element.
    // Small / XSmall tooltips live on dense header controls so they sit
    // closer to the trigger (~4px); Medium uses the standard ~8px gap.
    // No border on the surface itself — a lit outline around a floating
    // tooltip reads as a distracting window-chrome artifact, so we rely
    // on shadow + rounded corners to separate the tooltip from the page.
    div()
        .map(|el| match size {
            Size::XSmall | Size::Small => el.pl_1().pt_1(),
            _ => el.pl_2().pt_2p5(),
        })
        .child(
            v_flex()
                .bg(bg)
                .rounded_md()
                .shadow(shadow)
                .font(ui_font)
                .text_color(app.theme().colors().text)
                .map(|el| match size {
                    Size::XSmall => el.text_xs().py_0p5().px_1p5(),
                    Size::Small => el.text_xs().py_0p5().px_2(),
                    _ => el.text_ui(app).py_1().px_2(),
                })
                .map(|el| f(el, cx)),
        )
}

/// A compact tooltip for chip elements — smaller, terminal-chip style.
///
/// Renders as a tight pill above the cursor with minimal padding,
/// light background, and small text.
pub struct ChipTooltip {
    title: SharedString,
}

impl ChipTooltip {
    /// Returns a closure suitable for use with `.tooltip()` on a chip element.
    pub fn text(title: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let title = title.into();
        move |_, cx| {
            cx.new(|_| ChipTooltip {
                title: title.clone(),
            })
            .into()
        }
    }
}

impl Render for ChipTooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();

        div()
            .py(px(3.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .bg(colors.element_background)
            .border_1()
            .border_color(colors.border)
            .shadow_sm()
            .child(
                Label::new(self.title.clone())
                    .size(LabelSize::Small)
                    .color(Color::Default),
            )
    }
}

pub struct LinkPreview {
    link: SharedString,
}

impl LinkPreview {
    pub fn new(url: &str, cx: &mut App) -> AnyView {
        let mut wrapped_url = String::new();
        for (i, ch) in url.chars().enumerate() {
            if i == 500 {
                wrapped_url.push('…');
                break;
            }
            if i % 100 == 0 && i != 0 {
                wrapped_url.push('\n');
            }
            wrapped_url.push(ch);
        }
        cx.new(|_| LinkPreview {
            link: wrapped_url.into(),
        })
        .into()
    }
}

impl Render for LinkPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        tooltip_container(cx, |el, _| {
            el.child(
                Label::new(self.link.clone())
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
        })
    }
}

impl Component for Tooltip {
    fn scope() -> ComponentScope {
        ComponentScope::DataDisplay
    }

    fn description() -> Option<&'static str> {
        Some(
            "A tooltip that appears when hovering over an element, optionally showing a keybinding or additional metadata.",
        )
    }

    fn preview(_window: &mut Window, _cx: &mut App) -> Option<AnyElement> {
        Some(
            example_group(vec![single_example(
                "Text only",
                Button::new("delete-example", "Delete")
                    .tooltip(Tooltip::text("This is a tooltip!"))
                    .into_any_element(),
            )])
            .into_any_element(),
        )
    }
}
