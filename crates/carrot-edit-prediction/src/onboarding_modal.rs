use std::sync::Arc;

use crate::{CarrotPredictUpsell, EditPredictionStore};
use carrot_ai_onboarding::EditPredictionOnboarding;
use carrot_client::{Client, UserStore};
use carrot_db::kvp::Dismissable;
use carrot_fs::Fs;
use carrot_language::language_settings::EditPredictionProvider;
use carrot_ui::{Vector, VectorName, prelude::*};
use carrot_workspace::{ModalView, Workspace};
use inazuma::{
    ClickEvent, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, MouseDownEvent, Render,
    linear_color_stop, linear_gradient,
};
use inazuma_settings_framework::update_settings_file;

#[macro_export]
macro_rules! onboarding_event {
    ($name:expr) => {
        carrot_telemetry::event!($name, source = "Edit Prediction Onboarding");
    };
    ($name:expr, $($key:ident $(= $value:expr)?),+ $(,)?) => {
        carrot_telemetry::event!($name, source = "Edit Prediction Onboarding", $($key $(= $value)?),+);
    };
}

/// Introduces user to Carrot's Edit Prediction feature
pub struct CarrotPredictModal {
    onboarding: Entity<EditPredictionOnboarding>,
    focus_handle: FocusHandle,
}

pub(crate) fn set_edit_prediction_provider(provider: EditPredictionProvider, cx: &mut App) {
    let fs = <dyn Fs>::global(cx);
    update_settings_file(fs, cx, move |settings, _| {
        settings
            .project
            .all_languages
            .edit_predictions
            .get_or_insert(Default::default())
            .provider = Some(provider);
    });
}

impl CarrotPredictModal {
    pub fn toggle(
        workspace: &mut Workspace,
        user_store: Entity<UserStore>,
        client: Arc<Client>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let project = workspace.project().clone();
        workspace.toggle_modal(window, cx, |_window, cx| {
            let weak_entity = cx.weak_entity();
            let copilot = EditPredictionStore::try_global(cx)
                .and_then(|store| store.read(cx).copilot_for_project(&project));
            Self {
                onboarding: cx.new(|cx| {
                    EditPredictionOnboarding::new(
                        user_store.clone(),
                        client.clone(),
                        copilot
                            .as_ref()
                            .is_some_and(|copilot| copilot.read(cx).status().is_configured()),
                        Arc::new({
                            let this = weak_entity.clone();
                            move |_window, cx| {
                                CarrotPredictUpsell::set_dismissed(true, cx);
                                set_edit_prediction_provider(EditPredictionProvider::Carrot, cx);
                                this.update(cx, |_, cx| cx.emit(DismissEvent)).ok();
                            }
                        }),
                        Arc::new({
                            let this = weak_entity.clone();
                            move |window, cx| {
                                CarrotPredictUpsell::set_dismissed(true, cx);
                                set_edit_prediction_provider(EditPredictionProvider::Copilot, cx);
                                this.update(cx, |_, cx| cx.emit(DismissEvent)).ok();
                                if let Some(copilot) = copilot.clone() {
                                    carrot_copilot_ui::initiate_sign_in(copilot, window, cx);
                                }
                            }
                        }),
                        cx,
                    )
                }),
                focus_handle: cx.focus_handle(),
            }
        });
    }

    fn cancel(&mut self, _: &inazuma_menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        CarrotPredictUpsell::set_dismissed(true, cx);
        cx.emit(DismissEvent);
    }
}

impl EventEmitter<DismissEvent> for CarrotPredictModal {}

impl Focusable for CarrotPredictModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ModalView for CarrotPredictModal {
    fn on_before_dismiss(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> carrot_workspace::DismissDecision {
        CarrotPredictUpsell::set_dismissed(true, cx);
        carrot_workspace::DismissDecision::Dismiss(true)
    }
}

impl Render for CarrotPredictModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let window_height = window.viewport_size().height;
        let max_height = window_height - px(200.);

        v_flex()
            .id("edit-prediction-onboarding")
            .key_context("CarrotPredictModal")
            .relative()
            .w(px(550.))
            .h_full()
            .max_h(max_height)
            .p_4()
            .gap_2()
            .elevation_3(cx)
            .track_focus(&self.focus_handle(cx))
            .overflow_hidden()
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(|_, _: &inazuma_menu::Cancel, _window, cx| {
                onboarding_event!("Cancelled", trigger = "Action");
                cx.emit(DismissEvent);
            }))
            .on_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.focus_handle.focus(window, cx);
            }))
            .child(
                div()
                    .opacity(0.5)
                    .absolute()
                    .top(px(-8.0))
                    .right_0()
                    .w(px(400.))
                    .h(px(92.))
                    .child(
                        Vector::new(VectorName::AiGrid, rems_from_px(400.), rems_from_px(92.))
                            .color(Color::Custom(cx.theme().colors().text.alpha(0.32))),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .w(px(660.))
                    .h(px(401.))
                    .overflow_hidden()
                    .bg(linear_gradient(
                        75.,
                        linear_color_stop(cx.theme().colors().panel.background.alpha(0.01), 1.0),
                        linear_color_stop(cx.theme().colors().panel.background, 0.45),
                    )),
            )
            .child(h_flex().absolute().top_2().right_2().child(
                IconButton::new("cancel", IconName::Close).on_click(cx.listener(
                    |_, _: &ClickEvent, _window, cx| {
                        onboarding_event!("Cancelled", trigger = "X click");
                        cx.emit(DismissEvent);
                    },
                )),
            ))
            .child(self.onboarding.clone())
    }
}
