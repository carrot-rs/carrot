//! Global command palette modal.
//!
//! A single floating panel that unifies search across the workspace —
//! sessions, agents, files, workflows, prompts, notebooks, environment
//! variables, drive items, actions, launch configurations and conversations.
//! Triggered by the title bar search field or a global keyboard shortcut.
//!
//! Typed filter prefixes (e.g. `env: HOME`, `sessions: foo`) restrict the
//! result set to one category. Clicking a chip toggles the same filter
//! visually. When no filter is active, results span all registered sources.

mod action_name;
mod category;
mod persistence;
mod source;

use std::sync::Arc;

use carrot_command_palette_hooks::{
    CommandInterceptItem, CommandInterceptResult, GlobalCommandPaletteInterceptor,
};
use carrot_ui::{
    Color, HighlightedLabel, Icon, IconName, IconSize, KeyBinding,
    input::{Input, InputEvent, InputState},
    prelude::*,
};
use carrot_workspace::{ModalView, Workspace};
use inazuma::{
    Action, AnyElement, App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, WeakEntity, Window, div, px,
};
use inazuma_fuzzy::{StringMatchCandidate, match_strings};
use schemars::JsonSchema;
use serde::Deserialize;

/// Per-source row cap for the empty-query "Suggested" mix. Low enough
/// that Actions don't drown Sessions/Files, high enough that the user
/// sees useful entries from each category at a glance.
const SUGGESTED_PER_SOURCE: usize = 8;

pub use action_name::{humanize_action_name, normalize_action_query};
pub use carrot_actions::command_palette::Toggle;
pub use category::{SearchCategory, parse_filter_prefix};
pub use persistence::CommandPaletteDB;
pub use source::{SearchAction, SearchResult, SearchSource, default_sources};

use source::FilesSource;

/// Opens the command palette with a pre-selected category filter.
///
/// Keybinds like `Cmd+O`, `Cmd+Shift+P`, `Cmd+R` all dispatch this action
/// with a different `category_filter`, so the user sees the same modal
/// with the matching chip already active instead of a separate per-category
/// panel.
#[derive(Clone, Default, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = command_palette)]
#[serde(deny_unknown_fields)]
pub struct ToggleWithFilter {
    /// Category chip to mark as active on open. `None` is equivalent to
    /// dispatching `Toggle` — the universal (no-filter) mode.
    #[serde(default)]
    pub category_filter: Option<SearchCategory>,
}

/// Register the global toggle actions so any focused element can open the
/// command palette modal.
pub fn init(cx: &mut App) {
    carrot_command_palette_hooks::init(cx);
    source::files_init(cx);
    cx.observe_new(|workspace: &mut Workspace, _window, _cx| {
        workspace.register_action(|workspace, _: &Toggle, window, cx| {
            CommandPalette::toggle_or_retarget(workspace, None, window, cx);
        });
        workspace.register_action(|workspace, action: &ToggleWithFilter, window, cx| {
            CommandPalette::toggle_or_retarget(workspace, action.category_filter, window, cx);
        });
    })
    .detach();
}

/// One row in the result list. Keeps the underlying source data together
/// with the fuzzy match's character positions so the modal can render
/// highlighted matches inline.
struct ResultRow {
    result: SearchResult,
    positions: Vec<usize>,
}

pub struct CommandPalette {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    search: inazuma::Entity<InputState>,
    /// Active filter category — derived from the typed prefix in the
    /// search input. Single source of truth: chip clicks, action
    /// shortcuts (`Cmd+O`/etc.) and direct typing all funnel through the
    /// input value, so there is no separate "selected via chip" vs
    /// "typed prefix" state to keep in sync.
    active_prefix: Option<SearchCategory>,
    sources: Vec<Arc<dyn SearchSource>>,
    /// Separate handle to the `FilesSource` so the modal can flip the
    /// include-ignored toggle without having to downcast through the
    /// generic source list.
    files_source: Arc<FilesSource>,
    include_ignored: bool,
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

impl CommandPalette {
    /// Opens the modal, or — if it is already mounted — re-targets its
    /// filter without closing. Pressing `Cmd+O` while the universal
    /// `Cmd+P` view is open should switch to the Files filter in place,
    /// not slam the modal shut so the user has to reopen it.
    pub fn toggle_or_retarget(
        workspace: &mut Workspace,
        category_filter: Option<SearchCategory>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        if let Some(palette) = workspace.active_modal::<CommandPalette>(cx) {
            palette.update(cx, |modal, cx| {
                modal.set_filter(category_filter, window, cx);
            });
            return;
        }
        Self::open_with_filter(workspace, "", category_filter, window, cx);
    }

    /// Open the modal pre-loaded with `query` and `category_filter`.
    /// The filter is stored as a chip-style UI element on the modal
    /// itself, not as text in the input — that's what gives it the
    /// italic-bold rendering and the "backspace at empty clears the
    /// chip" behaviour.
    pub fn open_with_filter(
        workspace: &mut Workspace,
        query: &str,
        category_filter: Option<SearchCategory>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let weak = workspace.weak_handle();
        let query: SharedString = query.to_string().into();
        workspace.toggle_modal(window, cx, move |window, cx| {
            let mut modal = CommandPalette::new(weak, window, cx);
            modal.active_prefix = category_filter;
            if !query.is_empty() {
                modal.search.update(cx, |state, cx| {
                    state.set_value(query, window, cx);
                });
            }
            modal
        });
    }

    /// Replace the active filter with `category_filter`, clearing the
    /// search input so the user types into a fresh query under the new
    /// scope. Used by chip clicks, the `Cmd+O`/etc. retarget path, and
    /// the Tab autocomplete commit.
    pub fn set_filter(
        &mut self,
        category_filter: Option<SearchCategory>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_prefix = category_filter;
        self.search.update(cx, |state, cx| {
            state.set_value(SharedString::default(), window, cx);
        });
    }

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

        // Initial refresh is deferred: `CommandPalette::new` runs inside
        // `workspace.toggle_modal`, which is itself inside a
        // `workspace.update` frame — calling `workspace.read(cx)`
        // synchronously here would panic. `defer_in` schedules the
        // refresh for the end of the current effect cycle, after the
        // workspace update has returned control.
        cx.defer_in(window, |this, window, cx| {
            this.refresh_results(window, cx);
        });

        let files_source = Arc::new(FilesSource::new());
        let sources: Vec<Arc<dyn SearchSource>> = vec![
            Arc::new(source::ActionsSource),
            Arc::new(source::SessionsSource),
            files_source.clone(),
            Arc::new(source::HistorySource::new()),
            Arc::new(source::EnvVarsSource),
        ];

        Self {
            workspace,
            focus_handle,
            search,
            active_prefix: None,
            sources,
            files_source,
            include_ignored: false,
            results: Vec::new(),
            selected_index: 0,
            has_interacted: false,
            _search_task: None,
            _subscriptions: vec![input_subscription],
        }
    }

    fn toggle_include_ignored(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.include_ignored = !self.include_ignored;
        self.files_source.set_include_ignored(self.include_ignored);
        self.refresh_results(window, cx);
    }

    /// Find the unique [`SearchCategory`] whose prefix starts with the
    /// current input (case-insensitive). Returns `None` when the input
    /// is empty, already a complete prefix, ambiguous, or unrelated.
    /// Used both by the Tab key handler and by the hint chip rendered
    /// next to the search input.
    fn pending_prefix_completion(&self, cx: &App) -> Option<SearchCategory> {
        let raw = self.search.read(cx).value().to_string();
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.contains(':') {
            return None;
        }
        let needle = trimmed.to_ascii_lowercase();
        let mut iter = SearchCategory::all()
            .iter()
            .copied()
            .filter(|c| c.prefix().starts_with(&needle));
        let first = iter.next()?;
        // Ambiguous (multiple prefixes share this stem) → no completion.
        if iter.next().is_some() {
            return None;
        }
        Some(first)
    }

    fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "tab" => {
                if let Some(cat) = self.pending_prefix_completion(cx) {
                    self.set_filter(Some(cat), window, cx);
                    self.has_interacted = true;
                }
            }
            "backspace" => {
                // Chip-style: backspace on an already-empty input
                // discards the active prefix and falls back to the
                // universal Cmd+P view, exposing the discovery chip
                // strip again.
                if self.active_prefix.is_some()
                    && self.search.read(cx).value().is_empty()
                {
                    self.set_filter(None, window, cx);
                }
            }
            _ => {}
        }
    }

    /// Recompute `results` from the current query + filter state. Uses
    /// `inazuma_fuzzy::match_strings` on a background executor so large
    /// env var lists don't block the UI thread.
    fn refresh_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Auto-promote: if the user typed `files:` literally into the
        // input, lift it to a proper chip and strip the prefix from the
        // query text. Tab and the chip strip are the friendlier paths,
        // but typing the prefix has to keep working.
        let raw = self.search.read(cx).value().to_string();
        let (typed_prefix, rest) = parse_filter_prefix(&raw);
        if let Some(cat) = typed_prefix {
            self.active_prefix = Some(cat);
            let rest_value: SharedString = rest.to_string().into();
            self.search.update(cx, |state, cx| {
                state.set_value(rest_value, window, cx);
            });
        }
        let effective_cat = self.active_prefix;
        let query = if typed_prefix.is_some() {
            rest.to_string()
        } else {
            raw.clone()
        };

        // Placeholder follows the active filter so an empty input under
        // a `files:` chip reads "Search files" instead of the universal
        // "Search for a command".
        let placeholder: SharedString = match effective_cat {
            Some(cat) => cat.search_placeholder().into(),
            None => "Search for a command".into(),
        };
        self.search.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx);
        });

        let Some(workspace) = self.workspace.upgrade() else {
            self.results.clear();
            self.selected_index = 0;
            cx.notify();
            return;
        };

        // Three routing modes for the source loop:
        //  1. explicit category (chip or `prefix:`) → only that source
        //  2. universal + empty query → Suggested: only `default_visible`
        //  3. universal + typed query → wide search: only `searchable`
        // Rules 2 and 3 are independent gates, so History can stay out
        // of Suggested while still matching typed queries, and EnvVars
        // can be chip-only without polluting the live-search results.
        let is_default_view = effective_cat.is_none() && query.is_empty();
        let raw_results: Vec<SearchResult> = {
            let mut collected = Vec::new();
            let sources = self.sources.clone();
            for src in &sources {
                let include = if let Some(cat) = effective_cat {
                    src.category() == cat
                } else if query.is_empty() {
                    src.default_visible()
                } else {
                    src.searchable()
                };
                if !include {
                    continue;
                }
                let mut from_source = src.collect(&workspace, &query, window, cx);
                if is_default_view {
                    from_source.truncate(SUGGESTED_PER_SOURCE);
                }
                collected.extend(from_source);
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

        // Dynamic action discovery: crates like carrot-vim register an
        // interceptor so user-typed text can produce synthetic actions
        // (`:q`, `:wq`, carrot://…). Only consulted when Actions is in
        // scope, otherwise we'd leak vim commands into a files search.
        let interceptor_task = if matches!(effective_cat, None | Some(SearchCategory::Actions)) {
            GlobalCommandPaletteInterceptor::intercept(&query, self.workspace.clone(), cx)
        } else {
            None
        };

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
            let intercept_result = match interceptor_task {
                Some(task) => task.await,
                None => CommandInterceptResult::default(),
            };

            let mut slot: Vec<Option<SearchResult>> = raw_results.into_iter().map(Some).collect();
            let mut ordered: Vec<ResultRow> = Vec::new();

            // Intercepted items come first — they're typically exact matches
            // for the exact query the user typed (e.g. `:q` → `vim::Quit`).
            for CommandInterceptItem {
                action,
                string,
                positions,
            } in intercept_result.results
            {
                ordered.push(ResultRow {
                    result: SearchResult {
                        id: format!("intercepted:{}", action.name()).into(),
                        category: SearchCategory::Actions,
                        title: SharedString::from(string),
                        subtitle: None,
                        icon: SearchCategory::Actions.icon(),
                        action: SearchAction::DispatchAction(action),
                    },
                    positions,
                });
            }

            if !intercept_result.exclusive {
                for m in matches {
                    if let Some(spot) = slot.get_mut(m.candidate_id)
                        && let Some(result) = spot.take()
                    {
                        ordered.push(ResultRow {
                            result,
                            positions: m.positions,
                        });
                    }
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
        // Frecency: record Actions invocations so the most-used entries
        // float to the top on subsequent opens. Other categories manage
        // their own ranking (sessions by order, files by recency).
        match (&row.result.category, &row.result.action) {
            (SearchCategory::Actions, SearchAction::DispatchAction(action)) => {
                let raw_name = action.name().to_string();
                let query = self.search.read(cx).value().to_string();
                let db = CommandPaletteDB::global(cx);
                cx.background_spawn(async move {
                    db.write_command_invocation(raw_name, query).await.ok();
                })
                .detach();
            }
            (SearchCategory::Files, SearchAction::OpenPath(path)) => {
                // Bubble the open to the top of the frecency list so the
                // next Cmd+O surfaces it without a fuzzy match.
                self.files_source.record_open(path.clone());
            }
            _ => {}
        }
        row.result.action.run(workspace, window, cx);
        cx.emit(DismissEvent);
    }

    fn render_chip(&self, category: SearchCategory, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let is_selected = self.active_prefix == Some(category);
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
            .id(("command-palette-chip", category as usize))
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
                let next = if this.active_prefix == Some(category) {
                    None
                } else {
                    Some(category)
                };
                this.set_filter(next, window, cx);
            }))
    }

    fn render_result_row(
        &self,
        index: usize,
        row: &ResultRow,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        // A row is considered "selected" only once the user has engaged
        // with the modal (typed or pressed an arrow key). Before that the
        // row with `selected_index == 0` gets no visual treatment, so the
        // list doesn't look pre-highlighted on open.
        let is_selected = self.has_interacted && index == self.selected_index;
        // Theme-aware highlight: `element_hover` / `element_selected`
        // adapt to whatever accent the active theme defines, where
        // `text_accent` was locked to the default palette's blue.
        let hover_bg = colors.element_hover;
        let selected_bg = colors.element_selected;
        let bg = if is_selected {
            selected_bg
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
        // Positions come from the concatenated haystack (`title + " " +
        // subtitle`). Split them back into per-label offsets so
        // `HighlightedLabel` can bold the matched bytes in each segment
        // independently — important for file rows where the match may
        // hit the filename, the parent path, or both.
        let (title_positions, subtitle_positions) =
            crate::source::split_path_positions(&row.positions, title.len());

        let mut item = h_flex()
            .id(("command-palette-result", index))
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
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(HighlightedLabel::new(title, title_positions).color(Color::Default)),
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
                    .child(
                        HighlightedLabel::new(subtitle_text, subtitle_positions)
                            .color(Color::Muted)
                            .size(carrot_ui::LabelSize::XSmall),
                    ),
            );
        }
        // Actions with a bound key get a trailing glyph so the user can
        // see the shortcut without invoking `which-key`.
        if row.result.category == SearchCategory::Actions
            && let Some(action) = row.result.action_ref()
        {
            let binding = KeyBinding::for_action(action, cx);
            if binding.has_binding(window) {
                item = item.child(binding);
            }
        }
        item.into_any_element()
    }

    fn empty_state_label(&self) -> &'static str {
        match self.active_prefix {
            Some(SearchCategory::Sessions) => "No sessions match.",
            Some(SearchCategory::EnvironmentVariables) => "No environment variables match.",
            Some(_) => "No results — this category has no source registered yet.",
            None => "Type to search. Use prefixes like sessions: or env: to filter.",
        }
    }
}

impl Focusable for CommandPalette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // Delegate to the search input so that opening the modal puts the
        // caret straight into the search field — without this delegation
        // the modal grabs focus itself and typing/backspace never reach
        // the input.
        self.search.focus_handle(cx)
    }
}

impl EventEmitter<DismissEvent> for CommandPalette {}

impl ModalView for CommandPalette {
    fn is_command_palette(&self) -> bool {
        true
    }
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let elevated_surface = cx.theme().colors().elevated_surface;
        let border = cx.theme().colors().border;
        let text_muted = cx.theme().colors().text_muted;

        let result_elements: Vec<AnyElement> = self
            .results
            .iter()
            .enumerate()
            .map(|(ix, row)| self.render_result_row(ix, row, window, cx))
            .collect();
        let has_results = !result_elements.is_empty();
        let empty_label = SharedString::from(self.empty_state_label());
        let query_is_empty = self.search.read(cx).value().is_empty();
        // Compact view when a category is locked in (typed prefix or
        // chip click). The discovery chip grid would just take up space
        // and re-shows the very category the user already picked.
        let in_compact_mode = self.active_prefix.is_some();
        let show_suggested_header = query_is_empty && self.active_prefix.is_none();

        v_flex()
            .key_context("CommandPalette")
            .track_focus(&self.focus_handle)
            .w(px(640.))
            .max_h(px(560.))
            .rounded(px(10.))
            .bg(elevated_surface)
            .overflow_hidden()
            // Tab → commit a pending category-prefix completion.
            // Backspace on an empty input → drop the active prefix
            // chip and return to universal Cmd+P.
            .on_key_down(cx.listener(Self::on_key_down))
            .child({
                let pending_completion = self.pending_prefix_completion(cx);
                let active_prefix = self.active_prefix;
                let accent = cx.theme().colors().text_accent;
                let prefix_element: AnyElement = match active_prefix {
                    Some(cat) => {
                        // Build the prefix face explicitly from the
                        // user's UI font so this works regardless of
                        // which family is configured. Italic and Bold
                        // are best-effort: when the active font lacks
                        // those variants, the colour contrast carries
                        // the visual distinction on its own — Inazuma
                        // does not synthesise italics, see
                        // zed-industries/zed#28569.
                        let ui_font = carrot_theme::theme_settings(cx).ui_font(cx);
                        let prefix_font = inazuma::Font {
                            family: ui_font.family.clone(),
                            features: ui_font.features.clone(),
                            fallbacks: ui_font.fallbacks.clone(),
                            weight: inazuma::FontWeight::BOLD,
                            style: inazuma::FontStyle::Italic,
                            stretch: ui_font.stretch,
                        };
                        div()
                            .text_color(accent)
                            .font(prefix_font)
                            .child(SharedString::from(cat.prefix()))
                            .into_any_element()
                    }
                    None => Icon::new(IconName::MagnifyingGlass)
                        .size_5()
                        .color(Color::Muted)
                        .into_any_element(),
                };
                h_flex()
                    .pl_5()
                    .pr_4()
                    .py_3()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        // Anchor the prefix inside the Input's own
                        // prefix slot so it inherits the input's text
                        // style scope and shares the size/baseline of
                        // the placeholder/value rendered next to it.
                        // The previous wrapper-div approach pushed the
                        // label into a parent whose text styles didn't
                        // cascade into the input's render context.
                        Input::new(&self.search)
                            .appearance(false)
                            .bordered(false)
                            .with_size(carrot_ui::Size::Medium)
                            .prefix(prefix_element),
                    )
                    .when_some(pending_completion, |row, cat| {
                        // Tab-hint chip: shows up the moment the typed
                        // input matches a unique category-prefix stem.
                        // Pressing Tab — or clicking the chip — commits
                        // the completion.
                        row.child(
                            div()
                                .id("command-palette-tab-hint")
                                .px_2()
                                .py_0p5()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(border)
                                .text_size(px(11.))
                                .text_color(text_muted)
                                .child(SharedString::from("tab"))
                                .cursor_pointer()
                                .hover(|el| el.text_color(cx.theme().colors().text))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.set_filter(Some(cat), window, cx);
                                    this.has_interacted = true;
                                })),
                        )
                    })
            })
            .child({
                // Three layouts share the result body:
                //  * universal + empty query  → chip discovery grid + Suggested
                //  * universal + typed query  → just results
                //  * compact (prefix active)  → just results, no chips
                // The chip grid only ever appears in the first case; in
                // compact mode it would re-show the very category the
                // user already locked in.
                let body = v_flex().px_4().py_4().gap_4();
                let body = if query_is_empty && !in_compact_mode {
                    let chips = SearchCategory::all()
                        .iter()
                        .map(|cat| self.render_chip(*cat, cx).into_any_element())
                        .collect::<Vec<_>>();
                    body.child(h_flex().flex_wrap().gap_2().children(chips))
                } else {
                    body
                };
                body.child(if has_results {
                    if show_suggested_header {
                        self.render_grouped_results(window, cx).into_any_element()
                    } else {
                        v_flex().gap_0p5().children(result_elements).into_any_element()
                    }
                } else {
                    div()
                        .text_size(px(11.))
                        .text_color(text_muted)
                        .child(empty_label)
                        .into_any_element()
                })
            })
            .child(self.render_footer(cx))
    }
}

impl CommandPalette {
    /// Empty-query rendering. Splits the result list into labeled
    /// sections — Actions become "Suggested", Files become "Recent",
    /// anything else uses the category's own label. Matches Warp's
    /// empty-state behaviour where a handful of curated shortcuts and a
    /// short history of recent files live in distinct groups.
    fn render_grouped_results(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let text_muted = cx.theme().colors().text_muted;
        let mut groups: Vec<(SearchCategory, Vec<usize>)> = Vec::new();
        for (ix, row) in self.results.iter().enumerate() {
            let cat = row.result.category;
            match groups.last_mut() {
                Some((last_cat, list)) if *last_cat == cat => list.push(ix),
                _ => groups.push((cat, vec![ix])),
            }
        }

        let mut root = v_flex().gap_3();
        for (cat, indices) in groups {
            let label = match cat {
                SearchCategory::Actions => "Suggested",
                SearchCategory::Files => "Recent",
                other => other.label(),
            };
            let mut section = v_flex().gap_1();
            section = section.child(
                div()
                    .text_size(px(11.))
                    .text_color(text_muted)
                    .child(SharedString::from(label)),
            );
            for ix in indices {
                let row = &self.results[ix];
                section = section.child(self.render_result_row(ix, row, window, cx));
            }
            root = root.child(section);
        }
        root
    }

    /// Footer combining two rows: the active source's status line
    /// (scope breadcrumb + scanned counter, when a stateful source like
    /// [`FilesSource`] has something to report) and the global pre-filter
    /// shortcut hints.
    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let border = colors.border;
        let accent = colors.text_accent;
        let text_muted = colors.text_muted;

        // Surface the FilesSource status when a files-oriented scope is
        // active, so users know how big their project is and whether the
        // walker truncated.
        let files_status = self
            .sources
            .iter()
            .find(|s| s.category() == SearchCategory::Files)
            .and_then(|s| s.footer_status(cx));

        let include_ignored = self.include_ignored;
        let toggle_label = if include_ignored {
            "ignored on"
        } else {
            "ignored off"
        };
        let toggle_color = if include_ignored { accent } else { text_muted };

        let status_row = files_status.map(|status| {
            let scope = status.scope_root.to_string_lossy().into_owned();
            let counter = if status.done {
                format!("{} files scanned", status.scanned)
            } else {
                format!("{} files scanned · scanning…", status.scanned)
            };
            let trunc = if status.truncated {
                " · truncated"
            } else {
                ""
            };
            h_flex()
                .px_4()
                .py_1()
                .gap_3()
                .border_t_1()
                .border_color(border)
                .text_size(px(10.))
                .text_color(text_muted)
                .child(
                    div()
                        .max_w(px(360.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(format!("scope · {scope}")),
                )
                .child(div().child(format!("· {counter}{trunc}")))
                .child(
                    div()
                        .id("include-ignored-toggle")
                        .ml_auto()
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(toggle_color)
                        .text_color(toggle_color)
                        .cursor_pointer()
                        .child(format!("· {toggle_label}"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_include_ignored(window, cx);
                        })),
                )
        });

        v_flex()
            .when_some(status_row, |el, row| el.child(row))
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .gap_4()
                    .border_t_1()
                    .border_color(border)
                    .text_size(px(10.))
                    .text_color(text_muted)
                    .child(div().child("⌘P · all"))
                    .child(div().child("⌘O · files"))
                    .child(div().child("⌘⇧P · sessions"))
                    .child(div().child("⌘R · history")),
            )
    }
}
