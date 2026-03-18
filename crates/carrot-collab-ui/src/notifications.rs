pub mod incoming_call_notification;
pub mod project_shared_notification;

use carrot_workspace::AppState;
use inazuma::App;
use std::sync::Arc;

pub fn init(app_state: &Arc<AppState>, cx: &mut App) {
    incoming_call_notification::init(app_state, cx);
    project_shared_notification::init(app_state, cx);
}
