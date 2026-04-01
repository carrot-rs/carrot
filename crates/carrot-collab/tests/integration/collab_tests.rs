use carrot_call::Room;
use carrot_client::ChannelId;
use inazuma::{Entity, TestAppContext};

// Aliases for old crate names used in test code
pub use carrot_call as call;
pub use carrot_collab_ui as collab_ui;
pub use carrot_command_palette_hooks as command_palette_hooks;
pub use carrot_dap as dap;
pub use carrot_dap_adapters as dap_adapters;
pub use carrot_debugger_ui as debugger_ui;
pub use carrot_editor as editor;
pub use carrot_git as git;
pub use carrot_http_client as http_client;
pub use carrot_language as language;
pub use carrot_log as zlog;
pub use carrot_lsp as lsp;
pub use carrot_project as project;
pub use carrot_release_channel as release_channel;
pub use carrot_rpc as rpc;
pub use carrot_theme as theme;
pub use carrot_theme_settings as theme_settings;
pub use carrot_workspace as workspace;
pub use inazuma as gpui;
pub use inazuma_menu as menu;
pub use inazuma_settings_framework as settings;
pub use inazuma_text as text;
pub use inazuma_tokio as gpui_tokio;
pub use inazuma_util as util;

mod agent_sharing_tests;
mod channel_buffer_tests;
mod channel_guest_tests;
mod channel_tests;
mod db_tests;
mod editor_tests;
mod following_tests;
mod git_tests;
mod integration_tests;
mod notification_tests;
mod random_channel_buffer_tests;
mod random_project_collaboration_tests;
mod randomized_test_helpers;
mod remote_editing_collaboration_tests;
mod test_server;

pub use randomized_test_helpers::{
    RandomizedTest, TestError, UserTestPlan, run_randomized_test, save_randomized_test_plan,
};
pub use test_server::{TestClient, TestServer};

#[derive(Debug, Eq, PartialEq)]
struct RoomParticipants {
    remote: Vec<String>,
    pending: Vec<String>,
}

fn room_participants(room: &Entity<Room>, cx: &mut TestAppContext) -> RoomParticipants {
    room.read_with(cx, |room, _| {
        let mut remote = room
            .remote_participants()
            .values()
            .map(|participant| participant.user.github_login.clone().to_string())
            .collect::<Vec<_>>();
        let mut pending = room
            .pending_participants()
            .iter()
            .map(|user| user.github_login.clone().to_string())
            .collect::<Vec<_>>();
        remote.sort();
        pending.sort();
        RoomParticipants { remote, pending }
    })
}

fn channel_id(room: &Entity<Room>, cx: &mut TestAppContext) -> Option<ChannelId> {
    cx.read(|cx| room.read(cx).channel_id())
}
