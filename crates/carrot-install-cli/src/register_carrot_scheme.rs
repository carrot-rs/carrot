use carrot_client::CARROT_URL_SCHEME;
use inazuma::{AsyncApp, actions};

actions!(
    cli,
    [
        /// Registers the carrot:// URL scheme handler.
        RegisterCarrotScheme
    ]
);

pub async fn register_carrot_scheme(cx: &AsyncApp) -> anyhow::Result<()> {
    cx.update(|cx| cx.register_url_scheme(CARROT_URL_SCHEME))
        .await
}
