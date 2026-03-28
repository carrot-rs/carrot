mod cloud;

use carrot_client::{Client, UserStore};
use carrot_language_model::LanguageModelRegistry;
use carrot_web_search::{WebSearchProviderId, WebSearchRegistry};
use inazuma::{App, Context, Entity};
use std::sync::Arc;

pub fn init(client: Arc<Client>, user_store: Entity<UserStore>, cx: &mut App) {
    let registry = WebSearchRegistry::global(cx);
    registry.update(cx, |registry, cx| {
        register_web_search_providers(registry, client, user_store, cx);
    });
}

fn register_web_search_providers(
    registry: &mut WebSearchRegistry,
    client: Arc<Client>,
    user_store: Entity<UserStore>,
    cx: &mut Context<WebSearchRegistry>,
) {
    register_carrot_web_search_provider(
        registry,
        client.clone(),
        user_store.clone(),
        &LanguageModelRegistry::global(cx),
        cx,
    );

    cx.subscribe(
        &LanguageModelRegistry::global(cx),
        move |this, registry, event, cx| {
            if let carrot_language_model::Event::DefaultModelChanged = event {
                register_carrot_web_search_provider(
                    this,
                    client.clone(),
                    user_store.clone(),
                    &registry,
                    cx,
                )
            }
        },
    )
    .detach();
}

fn register_carrot_web_search_provider(
    registry: &mut WebSearchRegistry,
    client: Arc<Client>,
    user_store: Entity<UserStore>,
    language_model_registry: &Entity<LanguageModelRegistry>,
    cx: &mut Context<WebSearchRegistry>,
) {
    let using_carrot_provider = language_model_registry
        .read(cx)
        .default_model()
        .is_some_and(|default| default.is_provided_by_zed());
    if using_carrot_provider {
        registry.register_provider(
            cloud::CloudWebSearchProvider::new(client, user_store, cx),
            cx,
        )
    } else {
        registry.unregister_provider(WebSearchProviderId(
            cloud::ZED_WEB_SEARCH_PROVIDER_ID.into(),
        ));
    }
}
