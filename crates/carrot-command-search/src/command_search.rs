//! Global command search modal.
//!
//! A single floating panel that unifies search across the workspace —
//! sessions, agents, files, workflows, prompts, notebooks, environment
//! variables, drive items, actions, launch configurations and conversations.
//! Triggered by the title bar search field or a global keyboard shortcut.
//!
//! Typed filter prefixes (e.g. `env: HOME`, `sessions: foo`) restrict the
//! result set to one category. Clicking a chip toggles the same filter
//! visually. When no filter is active, results span all registered sources.

mod category;
mod source;

use std::sync::Arc;

use carrot_ui::{
    Color, Icon, IconName, IconSize,
    input::{Input, InputEvent, InputState},
    prelude::*,
};
use carrot_workspace::{ModalView, Workspace};
use inazuma::{
    AnyElement, App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, WeakEntity, Window, actions, div, px,
};
use inazuma_fuzzy::{StringMatchCandidate, match_strings};

pub use category::{SearchCategory, parse_filter_prefix};
pub use source::{SearchAction, SearchResult, SearchSource, default_sources};

actions!(
    command_search,
    [
        /// Toggles the global command search modal. Cmd+P / Ctrl+R style.
        ToggleCommandSearch,
    ]
);

/// Register the global toggle action so any focused element can open the
/// command search modal.
pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _window, _cx| {
        log::info!("[command-search] registering ToggleCommandSearch handler on new workspace");
        workspace.register_action(|workspace, _: &ToggleCommandSearch, window, cx| {
            log::info!("[command-search] handler hit → toggling modal");
            let weak = workspace.weak_handle();
            workspace.toggle_modal(window, cx, move |window, cx| {
                CommandSearch::new(weak, window, cx)
            });
        });
    })
    .detach();
}

/// One row in the result list. Keeps the underlying source data together
/// with the fuzzy match's score and character positions so the modal can
/// render highlighted matches.
struct ResultRow {
    result: SearchResult,
    #[allow(dead_code)]
    score: f64,
    #[allow(dead_code)]
    positions: Vec<usize>,
}

pub struct CommandSearch {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    search: inazuma::Entity<InputState>,
    /// Chip-selected category. Overridden by a typed prefix when one is
    /// present.
    selected_category: Option<SearchCategory>,
    /// Prefix category extracted from the current input text. Stored so
    /// the empty-state label and rendering can reflect the effective
    /// filter without re-parsing.
    active_prefix: Option<SearchCategory>,
    sources: Vec<Arc<dyn SearchSource>>,
    results: Vec<ResultRow>,
    selected_index: usize,
    /// `selected_index` exists from first frame so Enter always has a
    /// target, but the visual selection highlight is suppressed until the
    /// user either types or navigates with the arrow keys. Avoids a
    /// confusing pre-highlighted row at modal open.
    has_interacted: bool,
    _search_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl CommandSearch {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search for a command"));

        let input_subscription = cx.subscribe_in(
            &search,
            window,
            |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    this.has_interacted = true;
                    this.refresh_results(window, cx);
                }
                InputEvent::PressEnter { .. } => this.activate_selected(window, cx),
                InputEvent::Escape => cx.emit(DismissEvent),
                InputEvent::HistoryUp => {
                    this.has_interacted = true;
                    this.move_selection_up(cx);
                }
                InputEvent::HistoryDown => {
                    this.has_interacted = true;
                    this.move_selection_down(cx);
                }
                _ => {}
            },
        );

        // Initial refresh is deferred: `CommandSearch::new` runs inside
        // `workspace.toggle_modal`, which is itself inside a
        // `workspace.update` frame — calling `workspace.read(cx)`
        // synchronously here would panic. `defer_in` schedules the
        // refresh for the end of the current effect cycle, after the
        // workspace update has returned control.
        cx.defer_in(window, |this, window, cx| {
            this.refresh_results(window, cx);
        });

        Self {
            workspace,
            focus_handle,
            search,
            selected_category: None,
            active_prefix: None,
            sources: default_sources(),
            results: Vec::new(),
            selected_index: 0,
            has_interacted: false,
            _search_task: None,
            _subscriptions: vec![input_subscription],
        }
    }

    /// Recompute `results` from the current query + filter state. Uses
    /// `inazuma_fuzzy::match_strings` on a background executor so large
    /// env var lists don't block the UI thread.
    fn refresh_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.search.read(cx).value().to_string();
        let (prefix_cat, rest) = parse_filter_prefix(&raw);
        self.active_prefix = prefix_cat;
        let effective_cat = prefix_cat.or(self.selected_category);
        let query = rest.to_string();

        let Some(workspace) = self.workspace.upgrade() else {
            self.results.clear();
            self.selected_index = 0;
            cx.notify();
            return;
        };

        let is_default_view = effective_cat.is_none() && query.is_empty();
        let raw_results: Vec<SearchResult> = {
            let workspace_ref = workspace.read(cx);
            let mut collected = Vec::new();
            for src in &self.sources {
                if let Some(cat) = effective_cat {
                    if src.category() != cat {
                        continue;
                    }
                } else if is_default_view && !src.default_visible() {
                    continue;
                }
                collected.extend(src.collect(workspace_ref, cx));
            }
            collected
        };

        let candidates: Vec<StringMatchCandidate> = raw_results
            .iter()
            .enumerate()
            .map(|(ix, r)| {
                let haystack = match &r.subtitle {
                    Some(sub) => format!("{} {}", r.title, sub),
                    None => r.title.to_string(),
                };
                StringMatchCandidate::new(ix, &haystack)
            })
            .collect();

        let executor = cx.background_executor().clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let matches = match_strings(
                &candidates,
                &query,
                false,
                true,
                200,
                &Default::default(),
                executor,
            )
            .await;
            let mut slot: Vec<Option<SearchResult>> = raw_results.into_iter().map(Some).collect();
            let mut ordered: Vec<ResultRow> = Vec::with_capacity(matches.len());
            for m in matches {
                if let Some(spot) = slot.get_mut(m.candidate_id)
                    && let Some(result) = spot.take()
                {
                    ordered.push(ResultRow {
                        result,
                        score: m.score,
                        positions: m.positions,
                    });
                }
            }
            this.update(cx, |this, cx| {
                this.results = ordered;
                this.selected_index = 0;
                cx.notify();
            })
            .ok();
        });
        self._search_task = Some(task);
    }

    fn move_selection_up(&mut self, cx: &mut Context<Self>) {
        if self.results.is_empty() {
            return;
        }
        if self.selected_index == 0 {
            self.selected_index = self.results.len() - 1;
        } else {
            self.selected_index -= 1;
        }
        cx.notify();
    }

    fn move_selection_down(&mut self, cx: &mut Context<Self>) {
        if self.results.is_empty() {
            return;
        }
        self.selected_index = (self.selected_index + 1) % self.results.len();
        cx.notify();
    }

    fn activate_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_index >= self.results.len() {
            return;
        }
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let row = self.results.swap_remove(self.selected_index);
        row.result.action.run(workspace, window, cx);
        cx.emit(DismissEvent);
    }

    fn render_chip(&self, category: SearchCategory, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let is_selected = self.selected_category == Some(category);
        let accent = colors.text_accent;
        let icon_color = category
            .icon_color()
            .map(|name| Color::Custom(name.scale(400)))
            .unwrap_or(Color::Muted);

        let (border_color, label_color) = if is_selected {
            (accent, accent)
        } else {
            (colors.border_variant, colors.text)
        };

        h_flex()
            .id(("command-search-chip", category as usize))
            .px_3p5()
            .py_1p5()
            .gap_2()
            .items_center()
            .rounded(px(999.))
            .border_1()
            .border_color(border_color)
            .bg(inazuma::transparent_black())
            .cursor_pointer()
            .hover(|el| el.border_color(accent).text_color(accent))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(label_color)
                    .child(category.label()),
            )
            .child(
                Icon::new(category.icon())
                    .size(IconSize::Small)
                    .color(icon_color),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.selected_category = if this.selected_category == Some(category) {
                    None
                } else {
                    Some(category)
                };
                this.refresh_results(window, cx);
            }))
    }

    fn render_result_row(
        &self,
        index: usize,
        row: &ResultRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let accent = colors.text_accent;
        // A row is considered "selected" only once the user has engaged
        // with the modal (typed or pressed an arrow key). Before that the
        // row with `selected_index == 0` gets no visual treatment, so the
        // list doesn't look pre-highlighted on open.
        let is_selected = self.has_interacted && index == self.selected_index;
        // Selected and hover share the same accent-tinted background so
        // keyboard navigation and pointer hover produce a consistent
        // affordance. Text stays neutral — only the bg tints.
        let hover_bg = accent.opacity(0.10);
        let bg = if is_selected {
            hover_bg
        } else {
            inazuma::transparent_black()
        };
        let subtitle_fg = colors.text_muted;
        let icon_color = row
            .result
            .category
            .icon_color()
            .map(|n| Color::Custom(n.scale(400)))
            .unwrap_or(Color::Muted);

        let title = row.result.title.clone();
        let subtitle = row.result.subtitle.clone();
        let icon = row.result.icon;

        let mut item = h_flex()
            .id(("command-search-result", index))
            .px_3()
            .py_2()
            .gap_3()
            .items_center()
            .rounded(px(6.))
            .bg(bg)
            .cursor_pointer()
            .hover(move |el| el.bg(hover_bg))
            .child(Icon::new(icon).size(IconSize::Small).color(icon_color))
            .child(
                v_flex().gap_0p5().flex_1().min_w_0().child(
                    div()
                        .text_size(px(13.))
                        .text_color(colors.text)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(title),
                ),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.selected_index = index;
                this.activate_selected(window, cx);
            }));
        if let Some(subtitle_text) = subtitle {
            item = item.child(
                div()
                    .text_size(px(11.))
                    .text_color(subtitle_fg)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .max_w(px(240.))
                    .child(subtitle_text),
            );
        }
        item.into_any_element()
    }

    fn empty_state_label(&self) -> &'static str {
        let effective = self.active_prefix.or(self.selected_category);
        match effective {
            Some(SearchCategory::Sessions) => "No sessions match.",
            Some(SearchCategory::EnvironmentVariables) => "No environment variables match.",
            Some(_) => "No results — this category has no source registered yet.",
            None => "Type to search. Use prefixes like sessions: or env: to filter.",
        }
    }
}

impl Focusable for CommandSearch {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // Delegate to the search input so that opening the modal puts the
        // caret straight into the search field — without this delegation
        // the modal grabs focus itself and typing/backspace never reach
        // the input.
        self.search.focus_handle(cx)
    }
}

impl EventEmitter<DismissEvent> for CommandSearch {}

impl ModalView for CommandSearch {}

impl Render for CommandSearch {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let elevated_surface = cx.theme().colors().elevated_surface;
        let border = cx.theme().colors().border;
        let text_muted = cx.theme().colors().text_muted;
        let chips = SearchCategory::all()
            .iter()
            .map(|cat| self.render_chip(*cat, cx).into_any_element())
            .collect::<Vec<_>>();

        let result_elements: Vec<AnyElement> = self
            .results
            .iter()
            .enumerate()
            .map(|(ix, row)| self.render_result_row(ix, row, cx))
            .collect();
        let has_results = !result_elements.is_empty();
        let empty_label = SharedString::from(self.empty_state_label());
        let query_is_empty = self.search.read(cx).value().is_empty();
        let show_suggested_header =
            query_is_empty && self.selected_category.is_none() && self.active_prefix.is_none();

        v_flex()
            .key_context("CommandSearch")
            .track_focus(&self.focus_handle)
            .w(px(640.))
            .max_h(px(560.))
            .rounded(px(10.))
            .bg(elevated_surface)
            .overflow_hidden()
            .child(
                h_flex()
                    .pl_5()
                    .pr_4()
                    .py_3()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        Icon::new(IconName::MagnifyingGlass)
                            .size_5()
                            .color(Color::Muted),
                    )
                    .child(
                        Input::new(&self.search)
                            .appearance(false)
                            .bordered(false)
                            .with_size(carrot_ui::Size::Medium),
                    ),
            )
            .child(
                v_flex()
                    .px_4()
                    .py_4()
                    .gap_4()
                    .child(h_flex().flex_wrap().gap_2().children(chips))
                    .child(if has_results {
                        let list = v_flex().gap_0p5().children(result_elements);
                        if show_suggested_header {
                            v_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(text_muted)
                                        .child("Suggested"),
                                )
                                .child(list)
                                .into_any_element()
                        } else {
                            list.into_any_element()
                        }
                    } else {
                        div()
                            .text_size(px(11.))
                            .text_color(text_muted)
                            .child(empty_label)
                            .into_any_element()
                    }),
            )
    }
}
