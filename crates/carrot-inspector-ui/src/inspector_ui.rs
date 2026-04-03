#[cfg(debug_assertions)]
mod div_inspector;
#[cfg(debug_assertions)]
mod inspector;

#[cfg(debug_assertions)]
pub use inspector::init;

#[cfg(not(debug_assertions))]
pub fn init(_app_state: std::sync::Arc<carrot_workspace::AppState>, cx: &mut inazuma::App) {
    use carrot_workspace::notifications::NotifyResultExt as _;
    use std::any::TypeId;

    cx.on_action(|_: &carrot_actions::dev::ToggleInspector, cx| {
        Err::<(), anyhow::Error>(anyhow::anyhow!(
            "dev::ToggleInspector is only available in debug builds"
        ))
        .notify_app_err(cx);
    });

    carrot_command_palette_hooks::CommandPaletteFilter::update_global(cx, |filter, _cx| {
        filter.hide_action_types(&[TypeId::of::<carrot_actions::dev::ToggleInspector>()]);
    });
}
