//! Component Preview Example
//!
//! Run with: `cargo run -p carrot-component-preview --example component_preview"`
use carrot_fs::RealFs;
use inazuma::{AppContext as _, Bounds, KeyBinding, WindowBounds, WindowOptions, actions, size};

use carrot_client::{Client, UserStore};
use carrot_language::LanguageRegistry;
use carrot_node_runtime::NodeRuntime;
use carrot_project::Project;
use carrot_reqwest_client::ReqwestClient;
use carrot_session::{AppSession, Session};
use carrot_ui::{App, px};
use carrot_workspace::{AppState, Workspace};
use std::sync::Arc;

use carrot_component_preview::{ComponentPreview, init};

actions!(carrot, [Quit]);

fn quit(_: &Quit, cx: &mut App) {
    cx.quit();
}

fn main() {
    inazuma_platform::application().run(|cx| {
        inazuma_component_registry::init();

        cx.on_action(quit);
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        let version =
            carrot_release_channel::AppVersion::load(env!("CARGO_PKG_VERSION"), None, None);
        carrot_release_channel::init(version, cx);

        let http_client =
            ReqwestClient::user_agent("component_preview").expect("Failed to create HTTP client");
        cx.set_http_client(Arc::new(http_client));

        let fs = Arc::new(RealFs::new(None, cx.background_executor().clone()));
        <dyn carrot_fs::Fs>::set_global(fs.clone(), cx);

        inazuma_settings_framework::init(cx);
        carrot_theme_settings::init(carrot_theme::LoadThemes::JustBase, cx);

        let languages = Arc::new(LanguageRegistry::new(cx.background_executor().clone()));
        let client = Client::production(cx);
        carrot_client::init(&client, cx);

        let user_store = cx.new(|cx| UserStore::new(client.clone(), cx));
        let session_id = uuid::Uuid::new_v4().to_string();
        let kvp = carrot_db::kvp::KeyValueStore::global(cx);
        let session = cx
            .foreground_executor()
            .block_on(Session::new(session_id, kvp));
        let session = cx.new(|cx| AppSession::new(session, cx));
        let node_runtime = NodeRuntime::unavailable();

        let app_state = Arc::new(AppState {
            languages,
            client,
            user_store,
            fs,
            build_window_options: |_, _| Default::default(),
            node_runtime,
            session,
        });
        AppState::set_global(Arc::downgrade(&app_state), cx);

        carrot_workspace::init(app_state.clone(), cx);
        init(app_state.clone(), cx);

        let size = size(px(1200.), px(800.));
        let bounds = Bounds::centered(None, size, cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            {
                move |window, cx| {
                    let app_state = app_state;
                    carrot_theme_settings::setup_body_font(window, cx);

                    let project = Project::local(
                        app_state.client.clone(),
                        app_state.node_runtime.clone(),
                        app_state.user_store.clone(),
                        app_state.languages.clone(),
                        app_state.fs.clone(),
                        None,
                        carrot_project::LocalProjectFlags {
                            init_worktree_trust: false,
                            ..Default::default()
                        },
                        cx,
                    );

                    let workspace = cx.new(|cx| {
                        Workspace::new(
                            Default::default(),
                            project.clone(),
                            app_state.clone(),
                            window,
                            cx,
                        )
                    });

                    workspace.update(cx, |workspace, cx| {
                        let weak_workspace = cx.entity().downgrade();
                        let language_registry = app_state.languages.clone();
                        let user_store = app_state.user_store.clone();

                        let component_preview = cx.new(|cx| {
                            ComponentPreview::new(
                                weak_workspace,
                                project,
                                language_registry,
                                user_store,
                                None,
                                None,
                                window,
                                cx,
                            )
                            .expect("Failed to create component preview")
                        });

                        workspace.add_item_to_active_pane(
                            Box::new(component_preview),
                            None,
                            true,
                            window,
                            cx,
                        );
                    });

                    workspace
                }
            },
        )
        .expect("Failed to open component preview window");

        cx.activate(true);
    });
}
