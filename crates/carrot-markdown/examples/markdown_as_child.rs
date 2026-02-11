use carrot_assets::Assets;
use carrot_language::LanguageRegistry;
use carrot_markdown::{Markdown, MarkdownElement, MarkdownStyle};
use carrot_node_runtime::NodeRuntime;
use carrot_theme::LoadThemes;
use carrot_ui::div;
use carrot_ui::prelude::*;
use inazuma::{Entity, KeyBinding, Length, StyleRefinement, WindowOptions, rgb};
use inazuma_settings_framework::SettingsStore;
use std::sync::Arc;

const MARKDOWN_EXAMPLE: &str = r#"
this text should be selectable

wow so cool

## Heading 2
"#;
pub fn main() {
    env_logger::init();

    inazuma::application().with_assets(Assets).run(|cx| {
        let store = SettingsStore::test(cx);
        cx.set_global(store);
        cx.bind_keys([KeyBinding::new("cmd-c", carrot_markdown::Copy, None)]);

        let node_runtime = NodeRuntime::unavailable();
        let language_registry = Arc::new(LanguageRegistry::new(cx.background_executor().clone()));
        let fs = carrot_fs::FakeFs::new(cx.background_executor().clone());
        carrot_languages::init(language_registry, fs, node_runtime, cx);
        carrot_theme_settings::init(LoadThemes::JustBase, cx);
        Assets.load_fonts(cx).unwrap();

        cx.activate(true);
        let _ = cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|cx| {
                let markdown = cx.new(|cx| Markdown::new(MARKDOWN_EXAMPLE.into(), None, None, cx));

                HelloWorld { markdown }
            })
        });
    });
}
struct HelloWorld {
    markdown: Entity<Markdown>,
}

impl Render for HelloWorld {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let markdown_style = MarkdownStyle {
            base_text_style: inazuma::TextStyle {
                font_family: "Carrot Mono".into(),
                color: cx.theme().colors().text,
                ..Default::default()
            },
            code_block: StyleRefinement {
                text: inazuma::TextStyleRefinement {
                    font_family: Some("Carrot Mono".into()),
                    background_color: Some(cx.theme().colors().editor.background),
                    ..Default::default()
                },
                margin: inazuma::EdgesRefinement {
                    top: Some(Length::Definite(rems(4.).into())),
                    left: Some(Length::Definite(rems(4.).into())),
                    right: Some(Length::Definite(rems(4.).into())),
                    bottom: Some(Length::Definite(rems(4.).into())),
                },
                ..Default::default()
            },
            inline_code: inazuma::TextStyleRefinement {
                font_family: Some("Carrot Mono".into()),
                background_color: Some(cx.theme().colors().editor.background),
                ..Default::default()
            },
            rule_color: Color::Muted.color(cx),
            block_quote_border_color: Color::Muted.color(cx),
            block_quote: inazuma::TextStyleRefinement {
                color: Some(Color::Muted.color(cx)),
                ..Default::default()
            },
            link: inazuma::TextStyleRefinement {
                color: Some(Color::Accent.color(cx)),
                underline: Some(inazuma::UnderlineStyle {
                    thickness: px(1.),
                    color: Some(Color::Accent.color(cx)),
                    wavy: false,
                }),
                ..Default::default()
            },
            syntax: cx.theme().syntax().clone(),
            selection_background_color: cx.theme().colors().element_selection,
            heading: Default::default(),
            ..Default::default()
        };

        div()
            .flex()
            .bg(rgb(0x2e7d32))
            .size(Length::Definite(px(700.0).into()))
            .justify_center()
            .items_center()
            .shadow_lg()
            .border_1()
            .border_color(rgb(0x0000ff))
            .text_xl()
            .text_color(rgb(0xffffff))
            .child(
                div()
                    .child(MarkdownElement::new(self.markdown.clone(), markdown_style))
                    .p_20(),
            )
    }
}
