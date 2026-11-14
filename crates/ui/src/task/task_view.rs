//! The task view: the open task's title bar, claim bar, labels, git panel,
//! tabs and
//! either the Markdown preview or the editor. It holds the open task's ID,
//! never a position in the list, and reads the task from the shared
//! `IssueStore`.
//!
//! Like the list, it keeps its own view state, such as scrolling, tabs and
//! vim mode, and asks the workspace for every change to an issue.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, ThemeStyled as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{self as ed, Editor, EditorState},
    menu::{DropdownMenu as _, PopupMenuItem},
    text::TextView,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, InteractiveElement as _,
    IntoElement, KeyContext, KeyDownEvent, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _,
};

use llmtodo_api_types::IssueGit;

use crate::inbox::AnswerInputs;
use crate::model::data::{Issue, IssueId};
use crate::model::store::IssueStore;
use crate::shell::icons::HasIcon as _;
use crate::shell::tooltip::{Card, with_card};
use crate::shell::view_events::{IssueAction, ViewEvent, ViewEvents};
use crate::shell::widgets::{self, lock_colors};
use crate::task::fold_highlighter;
use crate::task::panel_resize;
use crate::task::sections;
use crate::task::vim_editor::{self, Command, VimMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailMode {
    Preview,
    Editing,
}

/// The open task and what the task view shows of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detail {
    pub issue: IssueId,
    pub mode: DetailMode,
}

/// What the prepared tabs were built from. A change to any part builds
/// them again.
#[derive(Clone, PartialEq)]
struct PreparedStamp {
    issue: IssueId,
    description: SharedString,
    history: u64,
    keys: u64,
    server: Option<SharedString>,
}

/// The open task's tabs, each tab's Markdown ready for the preview, and the
/// description's questions. Cloning it costs three counter bumps.
#[derive(Clone)]
struct PreparedTabs {
    stamp: PreparedStamp,
    tabs: Rc<Vec<sections::Tab>>,
    bodies: Rc<[SharedString]>,
    questions: Rc<Vec<llmtodo_core::Question>>,
}

/// The conflict view's lines and the two texts they came from.
struct ConflictLines {
    theirs: SharedString,
    mine: SharedString,
    lines: Rc<[(SharedString, bool)]>,
}

/// The task view, when it shows the preview and holds focus.
pub const DETAIL_CONTEXT: &str = "DetailPane";

pub struct TaskView {
    store: Entity<IssueStore>,
    events: ViewEvents,
    /// The answer fields of owner questions, shared with the inbox.
    answers: Entity<AnswerInputs>,
    open: Option<Detail>,
    /// Focus for the task view while it shows the preview.
    pub focus: FocusHandle,
    preview_scroll: ScrollHandle,
    /// The preview's width in the last frame. Wide enough, and the tabs
    /// show side by side as columns.
    preview_width: std::rc::Rc<std::cell::Cell<gpui_kit::Pixels>>,
    /// The room the tab bar's tabs had in the last frame. It decides which
    /// tabs show and which go under the More button.
    tab_room: std::rc::Rc<std::cell::Cell<gpui_kit::Pixels>>,
    /// The tab bar's shared hover background. The workspace draws it over
    /// this view, so its frames do not draw this view (LLM-220).
    pub tab_hover: Entity<crate::task::tab_bar::TabHover>,
    pub editor: Entity<EditorState>,
    pub editor_vim: VimMode,
    /// Whether the editor has vim keys. Off, it stays in insert mode.
    pub vim_enabled: bool,
    /// The vim register for `yy`, `dd`, `x` and `p`.
    register: Option<String>,
    /// The open tab per task, by task key. Missing means the first tab.
    pub open_tabs: HashMap<SharedString, usize>,
    /// The panels the owner dragged to a height, in rem, by panel name.
    /// See `panel_resize`.
    pub panel_heights: std::collections::BTreeMap<String, f32>,
    /// The claim bar shows every line, not only the first three.
    claim_expanded: bool,
    /// The git panel lists every commit, not only the first three.
    git_expanded: bool,
    /// Set when a click gave the task view focus, cleared by any key.
    pub focus_by_mouse: bool,
    /// The open task's Activity tab, as Markdown, once the server sent it.
    activity: Option<(IssueId, String)>,
    /// The open task's Sessions tab: the work log of agent sessions.
    sessions: Option<(IssueId, String)>,
    /// Counts up whenever `activity` or `sessions` changes.
    history_revision: u64,
    /// The open task's tabs and their preview text. GPUI renders the view
    /// on every scroll frame, so the text work runs only when its input
    /// changes. See [`PreparedTabs`].
    prepared: RefCell<Option<PreparedTabs>>,
    /// The server's lines in the save conflict view, each with whether the
    /// owner's text lacks it. Built again when either text changes.
    conflict_lines: RefCell<Option<ConflictLines>>,
    /// How often the prepared tabs were built, for tests.
    #[cfg(test)]
    pub(crate) prepared_builds: std::cell::Cell<usize>,
    /// What each session cost on the open task, for the cost line's card.
    pub session_costs: Option<(IssueId, crate::task::history::CostBreakdown)>,
    /// The editor holds the owner's text from a refused save, and the
    /// server's version shows beside it.
    pub conflict: bool,
    /// The review answers on their way to the server, one per task. The
    /// panel's buttons wait for the open task's answer.
    pub review_sending: HashMap<IssueId, llmtodo_api_types::ReviewVerdict>,
    /// Why the server refused each task's last review answer, for the
    /// panel.
    pub review_error: HashMap<IssueId, SharedString>,
}

impl TaskView {
    pub fn new(
        store: Entity<IssueStore>,
        events: ViewEvents,
        answers: Entity<AnswerInputs>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language(fold_highlighter::LANGUAGE)
                .line_number(true)
                .soft_wrap(true);
            state.set_highlighter_factory(fold_highlighter::factory(), cx);
            state
        });
        // The view is cached. It draws the open task from the store, and
        // the answer fields from `answers`.
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        cx.observe(&answers, |_, _, cx| cx.notify()).detach();
        Self {
            store,
            events,
            answers,
            open: None,
            focus: cx.focus_handle(),
            preview_scroll: ScrollHandle::new(),
            preview_width: Default::default(),
            tab_room: Default::default(),
            tab_hover: cx.new(|_| Default::default()),
            editor,
            editor_vim: VimMode::Normal,
            vim_enabled: true,
            register: None,
            open_tabs: HashMap::new(),
            panel_heights: Default::default(),
            claim_expanded: false,
            git_expanded: false,
            focus_by_mouse: false,
            conflict: false,
            review_sending: HashMap::new(),
            review_error: HashMap::new(),
            activity: None,
            sessions: None,
            history_revision: 0,
            prepared: RefCell::new(None),
            conflict_lines: RefCell::new(None),
            #[cfg(test)]
            prepared_builds: Default::default(),
            session_costs: None,
        }
    }

    /// Sets the open task's Activity and Sessions tabs and its cost card.
    #[cfg(test)]
    pub fn set_history(
        &mut self,
        id: IssueId,
        activity: String,
        sessions: String,
        costs: crate::task::history::CostBreakdown,
        cx: &mut Context<Self>,
    ) {
        self.set_activity(id, activity, cx);
        self.set_sessions(id, sessions, costs, cx);
    }

    /// Sets the open task's Activity tab.
    pub fn set_activity(&mut self, id: IssueId, activity: String, cx: &mut Context<Self>) {
        self.activity = Some((id, activity));
        self.history_revision += 1;
        cx.notify();
    }

    /// Sets the open task's Sessions tab and its cost card.
    pub fn set_sessions(
        &mut self,
        id: IssueId,
        sessions: String,
        costs: crate::task::history::CostBreakdown,
        cx: &mut Context<Self>,
    ) {
        self.sessions = Some((id, sessions));
        self.session_costs = Some((id, costs));
        self.history_revision += 1;
        cx.notify();
    }

    #[cfg(test)]
    pub fn git_expanded(&self) -> bool {
        self.git_expanded
    }

    pub fn detail(&self) -> Option<Detail> {
        self.open
    }

    pub fn editing(&self) -> bool {
        self.open.is_some_and(|d| d.mode == DetailMode::Editing)
    }

    /// Shows `id` in the preview, from the top.
    pub fn show(&mut self, id: IssueId, cx: &mut Context<Self>) {
        self.open = Some(Detail {
            issue: id,
            mode: DetailMode::Preview,
        });
        self.preview_scroll.set_offset(gpui_kit::Point::default());
        cx.notify();
    }

    /// The preview follows the list selection, but never leaves the editor.
    pub fn follow(&mut self, id: Option<IssueId>, cx: &mut Context<Self>) {
        if let Some(id) = id
            && let Some(detail) = self.open
            && detail.mode == DetailMode::Preview
            && detail.issue != id
        {
            self.show(id, cx);
        }
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        cx.notify();
    }

    pub fn set_mode(&mut self, mode: DetailMode, cx: &mut Context<Self>) {
        if let Some(detail) = &mut self.open {
            detail.mode = mode;
            if mode == DetailMode::Preview {
                self.conflict = false;
            }
            cx.notify();
        }
    }

    /// Opens the editor on `id` with `text`, in vim normal mode.
    pub fn start_editing(
        &mut self,
        id: IssueId,
        text: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |state, cx| {
            state.set_value(text.to_string(), window, cx);
            state.focus(window, cx);
        });
        self.open = Some(Detail {
            issue: id,
            mode: DetailMode::Editing,
        });
        self.editor_vim = if self.vim_enabled {
            VimMode::Normal
        } else {
            VimMode::Insert
        };
        self.conflict = false;
        cx.notify();
    }

    pub fn set_conflict(&mut self, conflict: bool, cx: &mut Context<Self>) {
        self.conflict = conflict && self.editing();
        cx.notify();
    }

    /// Whether the task view, or the editor inside it, holds focus.
    pub fn focused(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx) || self.editor_focused(window, cx)
    }

    pub fn editor_focused(&self, window: &Window, cx: &App) -> bool {
        self.editor.focus_handle(cx).contains_focused(window, cx)
    }

    /// Focus on the editor while editing, else on the preview.
    pub fn take_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing() {
            self.editor.update(cx, |state, cx| state.focus(window, cx));
        } else if self.open.is_some() {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// The focus outline shows for keyboard focus only, like CSS
    /// `:focus-visible`. A click into the task view gives it focus without
    /// the outline. The next key press shows it again.
    pub fn show_focus_outline(&self, window: &Window, cx: &App) -> bool {
        self.focused(window, cx) && !self.focus_by_mouse
    }

    // Scrolling the preview.

    fn scroll_preview(&mut self, delta: gpui_kit::Pixels, cx: &mut Context<Self>) {
        let max = self.preview_scroll.max_offset().y;
        let mut offset = self.preview_scroll.offset();
        offset.y = (offset.y - delta).clamp(-max, gpui_kit::px(0.));
        self.preview_scroll.set_offset(offset);
        cx.notify();
    }

    /// About two lines of body text.
    fn line_step(window: &Window) -> gpui_kit::Pixels {
        window.rem_size() * 3.
    }

    fn half_page(&self) -> gpui_kit::Pixels {
        self.preview_scroll.bounds().size.height / 2.
    }

    pub fn scroll_lines(&mut self, down: bool, window: &Window, cx: &mut Context<Self>) {
        let step = Self::line_step(window);
        self.scroll_preview(if down { step } else { -step }, cx);
    }

    pub fn scroll_half_page(&mut self, down: bool, cx: &mut Context<Self>) {
        let step = self.half_page();
        self.scroll_preview(if down { step } else { -step }, cx);
    }

    pub fn scroll_top(&mut self, cx: &mut Context<Self>) {
        self.preview_scroll.set_offset(gpui_kit::Point::default());
        cx.notify();
    }

    pub fn scroll_bottom(&mut self, cx: &mut Context<Self>) {
        let max = self.preview_scroll.max_offset().y;
        self.preview_scroll
            .set_offset(gpui_kit::point(gpui_kit::px(0.), -max));
        cx.notify();
    }

    pub fn preview_offset(&self) -> gpui_kit::Pixels {
        self.preview_scroll.offset().y
    }

    // Tabs.

    /// The open tab of the task view, clamped to the tabs it has now.
    pub fn open_tab(&self, cx: &App) -> Option<(usize, Rc<Vec<sections::Tab>>)> {
        let detail = self.open?;
        let issue = self.store.read(cx).get(detail.issue)?;
        let tabs = self.prepared(issue, cx).tabs;
        let ix = self
            .open_tabs
            .get(&issue.key)
            .copied()
            .unwrap_or(0)
            .min(tabs.len().saturating_sub(1));
        Some((ix, tabs))
    }

    /// The prepared tabs of `issue`, built again only when the text, the
    /// history, the server address or the store's keys changed.
    fn prepared(&self, issue: &Issue, cx: &App) -> PreparedTabs {
        let store = self.store.read(cx);
        let stamp = PreparedStamp {
            issue: issue.id,
            description: issue.description.clone(),
            history: self.history_revision,
            keys: store.keys_revision(),
            server: store.server_url.clone(),
        };
        let mut cache = self.prepared.borrow_mut();
        if let Some(prepared) = cache.as_ref().filter(|p| p.stamp == stamp) {
            return prepared.clone();
        }
        #[cfg(test)]
        self.prepared_builds.set(self.prepared_builds.get() + 1);
        let tabs = Rc::new(self.build_tabs(issue));
        let bodies = tabs
            .iter()
            .map(|tab| {
                crate::task::preview::prepare(&tab.body, stamp.server.as_deref(), |key| {
                    store.by_key(key).is_some()
                })
                .into()
            })
            .collect::<Vec<SharedString>>()
            .into();
        let questions = Rc::new(llmtodo_core::questions(&issue.description));
        let prepared = PreparedTabs {
            stamp,
            tabs,
            bodies,
            questions,
        };
        *cache = Some(prepared.clone());
        prepared
    }

    /// Each tab's preview text for the open task, for tests.
    #[cfg(test)]
    pub(crate) fn prepared_text(&self, cx: &App) -> Vec<SharedString> {
        let Some(issue) = self.open.and_then(|d| self.store.read(cx).get(d.issue)) else {
            return Vec::new();
        };
        self.prepared(issue, cx).bodies.to_vec()
    }

    /// Splits the description into tabs and adds the history tabs.
    fn build_tabs(&self, issue: &Issue) -> Vec<sections::Tab> {
        let mut tabs = sections::tabs(&issue.description);
        // An empty description keeps its tab. Without it, the read-only
        // Activity tab comes first, and `e` refuses to edit (LLM-131).
        if tabs.is_empty() {
            tabs.push(sections::Tab {
                title: sections::DESCRIPTION_TAB.to_string(),
                body: String::new(),
                read_only: None,
            });
        }
        // The Activity and Sessions tabs follow the description's own tabs.
        for (title, shows, extra) in [
            (
                crate::task::history::ACTIVITY_TAB,
                crate::task::history::ACTIVITY_SHOWS,
                &self.activity,
            ),
            (
                crate::task::history::SESSIONS_TAB,
                crate::task::history::SESSIONS_SHOWS,
                &self.sessions,
            ),
        ] {
            if let Some((_, body)) = extra
                .as_ref()
                .filter(|(id, body)| *id == issue.id && !body.is_empty())
            {
                tabs.push(sections::Tab {
                    title: title.into(),
                    body: body.clone(),
                    read_only: Some(shows),
                });
            }
        }
        tabs
    }

    pub fn set_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(key) = self
            .open
            .and_then(|d| self.store.read(cx).get(d.issue))
            .map(|i| i.key.clone())
        else {
            return;
        };
        // Missing means the first tab, so only other tabs are kept.
        if ix == 0 {
            self.open_tabs.remove(&key);
        } else {
            self.open_tabs.insert(key, ix);
        }
        self.preview_scroll.set_offset(gpui_kit::Point::default());
        cx.notify();
    }

    // Vim in the editor.

    pub fn run_vim(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_vim != VimMode::Normal {
            return;
        }
        let register = self.register.clone();
        let plan = self.editor.update(cx, |state, cx| {
            vim_editor::apply(command, state, register.as_deref(), window, cx)
        });
        if plan.register.is_some() {
            self.register = plan.register;
        }
        if plan.enter_insert {
            self.editor_vim = VimMode::Insert;
        }
        cx.notify();
    }

    /// One half of the Edit / Preview toggle. The selected mode is filled,
    /// the other is a quiet ghost button with dimmed text. The caller wraps
    /// it in a card tooltip.
    fn mode_button(
        &self,
        id: &'static str,
        label: &'static str,
        selected: bool,
        cx: &App,
    ) -> Button {
        let muted = cx.theme().muted_foreground;
        let events = self.events.clone();
        Button::new(id)
            .xsmall()
            .label(label)
            .map(|b| {
                if selected {
                    b.primary()
                } else {
                    b.ghost().text_color(muted)
                }
            })
            .on_click(move |_, window, cx| {
                if selected {
                    return;
                }
                events.emit(
                    if id == "mode-edit" {
                        ViewEvent::EditDescription
                    } else {
                        ViewEvent::ShowPreview
                    },
                    window,
                    cx,
                );
            })
    }

    fn render_detail(
        &self,
        detail: Detail,
        issue: &Issue,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let preview = detail.mode == DetailMode::Preview;
        let focused = self.show_focus_outline(window, cx);
        let body = match detail.mode {
            DetailMode::Editing if self.conflict => {
                let editor = self.render_editor(cx).into_any_element();
                let toggle = self.mode_toggle(detail, issue, cx);
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(description_title_row(toggle, cx))
                    .child(self.render_conflict(issue, editor, cx))
                    .into_any_element()
            }
            DetailMode::Editing => {
                let toggle = self.mode_toggle(detail, issue, cx);
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(description_title_row(toggle, cx))
                    .child(self.render_editor(cx))
                    .into_any_element()
            }
            DetailMode::Preview => {
                let toggle = self.mode_toggle(detail, issue, cx);
                self.render_preview(issue, toggle, window, cx)
                    .into_any_element()
            }
        };
        let header = self.render_detail_header(issue, window, cx);
        let meta_line = self.render_meta_line(issue, cx);

        let git = self
            .store
            .read(cx)
            .git(issue.id)
            .filter(|g| has_git(g))
            .cloned();
        let claim_bar = issue
            .claim
            .clone()
            .map(|claim| self.render_claim_bar(&claim, git.as_ref(), cx));
        // Analyzing shows only without a claim: Locked says more.
        let analyzing_bar = issue
            .analyzing
            .clone()
            .filter(|a| issue.claim.is_none() && a.is_live(jiff::Timestamp::now()))
            .map(|a| Self::render_analyzing_bar(&a, issue.id, cx));
        // A project's linked repo wins over the worktree's `origin`.
        let repo = issue
            .project
            .as_deref()
            .and_then(|p| self.store.read(cx).catalog.project_repo(p))
            .map(str::to_string);
        let heights = &self.panel_heights;
        let git_height = heights.get(panel_resize::GIT).copied();
        let review_height = heights.get(panel_resize::REVIEW).copied();
        let dragged = git_height.is_some() || review_height.is_some();
        let me = cx.entity().downgrade();
        let git_bar = git.map(|g| {
            let repo = repo.or_else(|| g.repo_url.clone());
            let panel = self
                .render_git_bar(&g, repo.as_deref(), git_height.is_some(), cx)
                .into_any_element();
            panel_resize::resizable(
                panel_resize::GIT,
                git_height,
                panel,
                me.clone(),
                self.events.clone(),
            )
        });
        let review_panel = {
            let store = self.store.read(cx);
            let state = crate::task::review::PanelState {
                answers: store.reviews(issue.id),
                feedback: self.answers.read(cx).feedback(issue.id),
                sending: self.review_sending.get(&issue.id).copied(),
                error: self.review_error.get(&issue.id).cloned(),
                fill: review_height.is_some(),
            };
            crate::task::review::panel(issue, state, self.events.clone(), cx).map(|panel| {
                panel_resize::resizable(
                    panel_resize::REVIEW,
                    review_height,
                    panel,
                    me.clone(),
                    self.events.clone(),
                )
            })
        };
        let theme = cx.theme();
        // The divider stays on the outer element. The inner element draws
        // the focus outline, so focus never shifts the layout.
        div()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            // Room for the focus ring, so the panel edge does not clip it.
            .p_1()
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.focus_by_mouse = true;
                cx.notify();
            }))
            .child(
                v_flex()
                    .id("detail")
                    // A click into the preview moves focus into the task view.
                    // It runs in the capture phase, before the children: a
                    // field inside, such as an answer field, then takes the
                    // focus for itself.
                    .when(preview, |this| {
                        this.capture_any_mouse_down(cx.listener(
                            |this, event: &gpui_kit::MouseDownEvent, window, cx| {
                                if event.button == gpui_kit::MouseButton::Left {
                                    window.focus(&this.focus, cx);
                                    cx.notify();
                                }
                            },
                        ))
                    })
                    .size_full()
                    // The same outline an input draws when focused: a
                    // one-pixel ring color border, the theme radius and the
                    // focus ring around it.
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(if focused {
                        theme.ring
                    } else {
                        gpui_kit::transparent_black()
                    })
                    .when(focused, |this| this.focus_ring_style(window, cx))
                    // Only the preview takes the task-view keys. In the
                    // editor, `j` and `k` belong to vim or to typing.
                    .when(preview, |this| {
                        this.key_context(DETAIL_CONTEXT).track_focus(&self.focus)
                    })
                    // The title and its meta line read as one block.
                    .child(
                        v_flex()
                            .id("detail-head")
                            .test_support()
                            .flex_none()
                            .w_full()
                            .min_w_0()
                            .border_b_1()
                            .border_color(theme.border)
                            .child(header)
                            .child(meta_line),
                    )
                    // Each panel is its own section with a hairline below.
                    .children(claim_bar)
                    .children(analyzing_bar)
                    .children(review_panel)
                    .children(git_bar)
                    // Dragged panels shrink before the description does, so
                    // it always keeps some room.
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .when(dragged, |this| {
                                this.min_h(gpui_kit::rems(panel_resize::BODY_MIN_REM))
                            })
                            .child(body),
                    ),
            )
    }

    /// The task's cost, its active time and its estimates, one group of the
    /// meta line. Every task shows all three, with a dash for a missing
    /// value. The card lists the cost per session.
    fn render_cost_line(&self, issue: &Issue, cx: &App) -> impl IntoElement + use<> {
        let time = issue.active_seconds.map_or(
            MISSING.to_string(),
            llmtodo_client::types::format_active_time,
        );
        let estimate = estimate_text(issue.estimate_minutes, issue.estimate_cost);
        let breakdown = self
            .session_costs
            .as_ref()
            .filter(|(id, _)| *id == issue.id)
            .map(|(_, breakdown)| breakdown);
        let theme = cx.theme();
        // `card` marks a value that opens a hover card.
        let pair = |name: &'static str, text: String, card: bool| {
            h_flex()
                .gap_2()
                .child(div().text_color(theme.muted_foreground).child(name))
                .child(
                    div()
                        .when(card, |d| widgets::card_underline(d, cx))
                        .font_family(theme.mono_font_family.clone())
                        .text_color(if text == MISSING {
                            theme.muted_foreground
                        } else {
                            theme.foreground
                        })
                        .child(text),
                )
        };
        let icon = || {
            Icon::new(IconName::CircleDollarSign)
                .xsmall()
                .text_color(theme.muted_foreground)
        };
        let cost = match issue.cost {
            Some(cost) => {
                let card = Card::cost(cost, issue.status.is_closed(), breakdown);
                let content = h_flex().gap_2().child(icon()).child(pair(
                    "Cost",
                    crate::model::data::format_cost(cost),
                    true,
                ));
                with_card("cost-tip", content, card).into_any_element()
            }
            None => h_flex()
                .gap_2()
                .child(icon())
                .child(pair("Cost", MISSING.to_string(), false))
                .into_any_element(),
        };
        h_flex()
            .id("cost-line")
            .test_support()
            .flex_wrap()
            .max_w_full()
            .min_w_0()
            .gap_x_3()
            .gap_y_1()
            .child(cost)
            .child(pair("Time", time, false))
            .child(pair("Estimate", estimate, false))
    }

    /// The button that starts a new dependency. It opens the palette with
    /// the command typed, so the owner picks the task by typing its key.
    fn dependency_menu(&self) -> impl IntoElement + use<> {
        let events = self.events.clone();
        with_card(
            "deps-tip",
            Button::new("deps-add")
                .ghost()
                .xsmall()
                .icon(IconName::Workflow)
                .label("Dependency")
                .dropdown_menu(move |menu, _, _| {
                    [("Depends on…", "depends "), ("Blocks…", "blocks ")]
                        .into_iter()
                        .fold(menu, |menu, (label, command)| {
                            let events = events.clone();
                            menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                                events.emit(
                                    ViewEvent::OpenPaletteWith(command.to_string()),
                                    window,
                                    cx,
                                );
                            }))
                        })
                }),
            Card::text("Add a dependency (:depends or :blocks)"),
        )
    }

    /// The dependencies, one group of the meta line: what this task waits
    /// for and what waits for it, as one chip per task. The Dependency
    /// button ends the group.
    fn render_dependencies(&self, issue: &Issue, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let store = self.store.read(cx);
        // Order edges show as one line each way, not one chip per task.
        let mut order_edges = (false, false);
        let mut list = |direction: &str| -> Vec<llmtodo_api_types::IssueLink> {
            store
                .relations(issue.id)
                .into_iter()
                .flatten()
                .filter(|r| {
                    r.kind == llmtodo_api_types::RelationType::Blocks
                        && r.direction == direction
                        && !r.issue.deleted
                })
                .filter(|r| {
                    let order = r.source == llmtodo_api_types::RelationSource::Order;
                    if order {
                        match direction {
                            "blocked-by" => order_edges.0 = true,
                            _ => order_edges.1 = true,
                        }
                    }
                    !order
                })
                .map(|r| r.issue.clone())
                .collect()
        };
        let depends = list("blocked-by");
        let blocks = list("blocks");
        let order_line = issue.parent.clone().zip(issue.order_step);
        let order_depends = order_edges.0 && order_line.is_some();
        let order_blocks = order_edges.1 && order_line.is_some();
        let any = !depends.is_empty() || !blocks.is_empty() || order_depends || order_blocks;
        let locked = issue.claim.is_some();
        let muted = cx.theme().muted_foreground;
        let mut items = Vec::new();
        for (title, links, verb, order) in [
            ("Depends on", depends, "undepend", order_depends),
            ("Blocks", blocks, "unblock", order_blocks),
        ] {
            if links.is_empty() && !order {
                continue;
            }
            items.push(div().text_color(muted).child(title).into_any_element());
            if let Some((parent, step)) = order_line.clone().filter(|_| order) {
                items.push(self.order_row(verb, parent, step, cx));
            }
            for link in links {
                items.push(self.dependency_row(issue.id, verb, &link, locked, cx));
            }
        }
        meta_group()
            .id("deps-group")
            .when(any, |this| {
                this.child(meta_group().id("deps-panel").test_support().children(items))
            })
            .child(self.dependency_menu())
            .into_any_element()
    }

    /// The one chip that stands for every order edge one way, such as
    /// "Order: step 3 of LLM-1". A click opens the parent. `:step` changes
    /// the order, so the chip has no X.
    fn order_row(
        &self,
        verb: &'static str,
        parent: SharedString,
        step: u32,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = cx.theme();
        let hover = theme.secondary_hover;
        let pressed = theme.secondary_active;
        let muted = theme.muted_foreground;
        let radius = theme.radius;
        let border = theme.border;
        let events = self.events.clone();
        let title = self.store.read(cx).by_key(&parent).map(|p| p.title.clone());
        let open_key = parent.clone();
        let chip = h_flex()
            .id(SharedString::from(format!("dep-order-{verb}")))
            .test_support()
            .flex_none()
            .h_5()
            .gap_1()
            .px_1()
            .rounded(radius)
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .active(move |s| s.bg(pressed))
            .child(
                Icon::new(IconName::GitCommitVertical)
                    .xsmall()
                    .text_color(muted),
            )
            .child(widgets::card_text(
                SharedString::from(format!("Order: step {step} of {parent}")),
                cx,
            ))
            .on_click(move |_, window, cx| {
                events.emit(ViewEvent::OpenKey(open_key.to_string()), window, cx);
            });
        let card = match title {
            Some(title) => format!("{parent} {title}"),
            None => parent.to_string(),
        };
        with_card(
            SharedString::from(format!("dep-order-tip-{verb}")),
            chip,
            Card::text(card).row(
                "Change it",
                Some(SharedString::from(":step <n>, :step new or :step none")),
            ),
        )
    }

    /// One task chip in the dependencies group. Its status comes from the
    /// live copy in the store, so it changes as that task changes. A click
    /// opens the task, and the card shows its title. The X removes the
    /// dependency.
    fn dependency_row(
        &self,
        id: IssueId,
        verb: &'static str,
        link: &llmtodo_api_types::IssueLink,
        locked: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let key: SharedString = link.key.to_string().into();
        let store = self.store.read(cx);
        let held = store.by_key(&key);
        let team = key.split('-').next().unwrap_or_default().to_string();
        let status = held.map_or_else(
            || store.catalog.status(&team, &link.status),
            |i| i.status.clone(),
        );
        let title: SharedString =
            held.map_or_else(|| link.title.clone().into(), |i| i.title.clone());
        let status_name = status.name.clone();
        let theme = cx.theme();
        let hover = theme.secondary_hover;
        let pressed = theme.secondary_active;
        let muted = theme.muted_foreground;
        let (icon_color, text_color) = if status.is_done() {
            (theme.success, muted)
        } else if status.is_closed() {
            (muted, muted)
        } else {
            (muted, theme.foreground)
        };
        let mono = theme.mono_font_family.clone();
        let radius = theme.radius;
        let border = theme.border;
        let open_events = self.events.clone();
        let remove_events = self.events.clone();
        let open_key = key.clone();
        let remove_key = key.clone();
        let open = h_flex()
            .id(SharedString::from(format!("dep-open-{verb}-{key}")))
            .test_support()
            .h_full()
            .gap_1()
            .px_1()
            .rounded(radius)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .active(move |s| s.bg(pressed))
            .child(Icon::new(status.icon()).xsmall().text_color(icon_color))
            .child(
                widgets::card_text(key.clone(), cx)
                    .font_family(mono)
                    .text_color(text_color),
            )
            .on_click(move |_, window, cx| {
                open_events.emit(ViewEvent::OpenKey(open_key.to_string()), window, cx);
            });
        h_flex()
            .id(SharedString::from(format!("dep-{verb}-{key}")))
            .test_support()
            .flex_none()
            .h_5()
            .rounded(radius)
            .border_1()
            .border_color(border)
            .child(with_card(
                SharedString::from(format!("dep-tip-{verb}-{key}")),
                open,
                Card::text(format!("{key} {title}")).row("Status", Some(status_name)),
            ))
            // A locked task stays read-only, so it offers no way to remove.
            .when(!locked, |this| {
                this.child(with_card(
                    SharedString::from(format!("dep-remove-tip-{verb}-{key}")),
                    Button::new(SharedString::from(format!("dep-remove-{verb}-{key}")))
                        .ghost()
                        .xsmall()
                        .icon(IconName::X)
                        .on_click(move |_, window, cx| {
                            remove_events.emit(
                                ViewEvent::ChangeDependency {
                                    id,
                                    verb: verb.to_string(),
                                    other: remove_key.to_string(),
                                },
                                window,
                                cx,
                            );
                        }),
                    Card::text(format!("Remove this dependency (:{verb} {key})")),
                ))
            })
            .into_any_element()
    }

    /// One gray line while an agent analyzes the task without a claim. The
    /// task stays editable, so the bar uses no lock color. It is not
    /// clickable, so it has no hover state.
    fn render_analyzing_bar(
        mark: &crate::model::data::Analyzing,
        id: crate::model::data::IssueId,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        // The workspace clock redraws the task view each minute.
        let line = format!(
            "Analyzed by {} for {}. You can still edit it.",
            mark.actor.label(),
            mark.age(jiff::Timestamp::now())
        );
        h_flex()
            .id(SharedString::from(format!("detail-analyzing-{id}")))
            .test_support()
            .flex_none()
            .w_full()
            .min_w_0()
            .px_3()
            .py_1p5()
            .gap_2()
            .text_xs()
            .bg(theme.muted)
            .text_color(theme.muted_foreground)
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(Icon::new(IconName::Eye).xsmall())
                    .child(Icon::new(mark.actor.icon()).xsmall()),
            )
            .child(div().flex_1().min_w_0().child(line))
    }

    /// Who holds the task and where, one fact per line. Lines wrap; past
    /// the third, a toggle shows the rest.
    fn render_claim_bar(
        &self,
        claim: &crate::model::data::Claim,
        git: Option<&IssueGit>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let (lock_bg, lock_fg) = lock_colors(cx);
        let theme = cx.theme();
        // The workspace clock redraws the task view each minute.
        let mut lines = vec![format!(
            "Locked by {} for {}. Read-only until it releases.",
            claim.actor.label(),
            claim.age(jiff::Timestamp::now())
        )];
        // The git panel below shows the same worktree and branch once.
        if let Some(w) = &claim.worktree
            && git.and_then(|g| g.worktree.as_deref()) != Some(w.as_ref())
        {
            lines.push(format!("Worktree: {}", crate::model::data::shorten_home(w)));
        }
        if let Some(b) = &claim.branch
            && git.and_then(|g| g.branch.as_deref()) != Some(b.as_ref())
        {
            lines.push(format!("Branch: {b}"));
        }
        if let Some(p) = &claim.pane {
            lines.push(format!("herdr pane: {p}"));
        }
        lines.push(format!("Session: {}", claim.session));
        let hidden = lines.len().saturating_sub(3);
        let expanded = self.claim_expanded;
        let shown = if expanded { lines.len() } else { 3 };
        let hover = lock_bg.opacity(0.6);
        let pressed = theme.foreground.opacity(0.12);
        let fg = theme.foreground;
        h_flex()
            .flex_none()
            .w_full()
            .min_w_0()
            .items_start()
            .px_3()
            .py_1p5()
            .gap_2()
            .text_xs()
            .bg(lock_bg)
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .pt_0p5()
                    .child(Icon::new(IconName::Lock).xsmall().text_color(lock_fg))
                    .child(Icon::new(claim.actor.icon()).xsmall()),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .children(
                        lines
                            .into_iter()
                            .take(shown)
                            .enumerate()
                            .map(move |(ix, line)| {
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .text_color(if ix == 0 { lock_fg } else { fg })
                                    .child(line)
                            }),
                    )
                    .when(hidden > 0, |this| {
                        this.child(more_toggle(
                            "claim-more",
                            if expanded {
                                "Show less".to_string()
                            } else {
                                format!("Show {hidden} more")
                            },
                            hover,
                            pressed,
                            cx.listener(|this, _, _, cx| {
                                this.claim_expanded = !this.claim_expanded;
                                cx.notify();
                            }),
                            cx,
                        ))
                    }),
            )
    }

    /// The issue's branch, worktree, PR and commits, one fact per row, in
    /// the monospace font. Past the third commit, a toggle shows the rest.
    /// With a known `repo` web page, each commit hash links to its commit.
    fn render_git_bar(
        &self,
        git: &IssueGit,
        repo: Option<&str>,
        fill: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let hover = theme.secondary_hover;
        let pressed = theme.secondary_active;
        let rows = git_fact_rows(git, cx);
        let total = git.commits.len();
        let expanded = self.git_expanded;
        let shown = if expanded { total } else { total.min(3) };
        let commits = git_commit_rows(git, repo, shown, cx);
        // The section title row. Past the third commit, it also holds the
        // toggle, so the toggle stays in place while the commits scroll.
        let more = (total > 3).then(|| {
            h_flex()
                .id("git-commits-header")
                .test_support()
                .flex_none()
                .gap_2()
                .text_xs()
                .text_color(muted)
                .child(format!("{total} commits"))
                .child(more_toggle(
                    "git-more",
                    if expanded {
                        "Show fewer".to_string()
                    } else {
                        format!("Show {} more", total - 3)
                    },
                    hover,
                    pressed,
                    cx.listener(|this, _, _, cx| {
                        this.git_expanded = !this.git_expanded;
                        cx.notify();
                    }),
                    cx,
                ))
        });
        let title = h_flex()
            .w_full()
            .min_w_0()
            .justify_between()
            .pb_0p5()
            .child(section_title(IconName::GitBranch, "Git", cx))
            .children(more);
        v_flex()
            .id("git-panel")
            .test_support()
            .flex_none()
            // A dragged panel fills its wrapper, and the commits take the
            // room that the rows leave.
            .when(fill, |this| this.size_full())
            .w_full()
            .min_w_0()
            .px_3()
            .py_2()
            .gap_0p5()
            .text_xs()
            .border_b_1()
            .border_color(theme.border)
            .child(title)
            .children(rows)
            .when(!commits.is_empty(), |this| {
                // The open list scrolls on its own, so it never pushes the
                // description out of the task view.
                this.child(
                    v_flex()
                        .id("git-commits")
                        .test_support()
                        .w_full()
                        .min_w_0()
                        .gap_0p5()
                        .map(|this| {
                            if fill {
                                this.flex_1().min_h_0()
                            } else {
                                this.max_h(gpui_kit::rems(Self::COMMITS_MAX_REM))
                            }
                        })
                        .overflow_y_scroll()
                        .children(commits),
                )
            })
    }

    /// The tallest the commit list gets, in rem: about five commits. It
    /// leaves the preview room on a short window, below the meta line.
    const COMMITS_MAX_REM: f32 = 7.;

    /// Whether `tabs` tabs fit side by side in `width`, each at least
    /// [`Self::COLUMN_MIN_REM`] wide. One tab never makes columns.
    pub fn columns_fit(width: gpui_kit::Pixels, tabs: usize, rem: gpui_kit::Pixels) -> bool {
        tabs > 1 && width >= rem * Self::COLUMN_MIN_REM * tabs as f32
    }

    /// The narrowest a description column gets, in rem. At the preview's
    /// text size, about 65 characters fit on a line.
    pub const COLUMN_MIN_REM: f32 = 30.;

    /// A tab's content. The Owner questions tab shows its questions as
    /// cards that answer them, like the inbox. Every other tab is Markdown.
    fn tab_body(
        &self,
        element_id: SharedString,
        tab: &sections::Tab,
        body: SharedString,
        questions: &[llmtodo_core::Question],
        issue: &Issue,
        cx: &App,
    ) -> gpui_kit::AnyElement {
        if tab
            .title
            .eq_ignore_ascii_case(llmtodo_core::questions::QUESTIONS_SECTION)
            && !questions.is_empty()
        {
            let answers = Some(self.answers.clone());
            return gpui_kit::component::v_flex()
                .id(element_id)
                .test_support()
                .w_full()
                .gap_2()
                .children(questions.iter().map(|q| {
                    let input = answers
                        .as_ref()
                        .and_then(|a| a.read(cx).get(issue.id, q.number));
                    crate::inbox::question_card(issue.id, q, input, false, self.events.clone(), cx)
                }))
                .into_any_element();
        }
        self.tab_view(element_id, body, cx).into_any_element()
    }

    /// One tab's Markdown, ready for the preview: file paths get the
    /// server's address, issue keys become links, and a click on a link
    /// opens the issue in the app or the page in the browser.
    fn tab_view(&self, element_id: SharedString, body: SharedString, cx: &App) -> TextView {
        let server = self.store.read(cx).server_url.clone();
        let link_events = self.events.clone();
        TextView::markdown(element_id, body)
            // Same size as the list rows.
            .text_sm()
            // Like the default, only a left or middle click, or a key,
            // follows a link.
            .on_link_click(move |url, event, window, cx| {
                if let gpui_kit::ClickEvent::Mouse(click) = event
                    && !matches!(
                        click.up.button,
                        gpui_kit::MouseButton::Left | gpui_kit::MouseButton::Middle
                    )
                {
                    return;
                }
                match crate::task::preview::link_target(url, server.as_deref()) {
                    crate::task::preview::LinkTarget::Issue(key) => {
                        link_events.emit(ViewEvent::OpenKey(key.to_string()), window, cx);
                    }
                    crate::task::preview::LinkTarget::Browser(url) => cx.open_url(&url),
                }
            })
    }

    fn render_preview(
        &self,
        issue: &Issue,
        toggle: Option<gpui_kit::AnyElement>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let prepared = self.prepared(issue, cx);
        let tabs = &prepared.tabs;
        let active = self
            .open_tabs
            .get(&issue.key)
            .copied()
            .unwrap_or(0)
            .min(tabs.len().saturating_sub(1));
        let id = issue.id;
        // Until the fetch brings the text, the view shows that it loads.
        let loading = !issue.description_loaded;
        let no_description = issue.description.trim().is_empty();
        // The empty state belongs to the Description tab, so an empty task
        // can still show its Activity tab.
        let empty = no_description && tabs.get(active).is_none_or(|t| t.read_only.is_none());
        let locked = issue.claim.is_some();
        // On a wide task view every tab shows at once, side by side. The
        // width is the one measured in the last frame.
        let columns = !no_description
            && Self::columns_fit(self.preview_width.get(), tabs.len(), window.rem_size());
        let shown: Vec<(usize, &sections::Tab)> = if columns {
            tabs.iter().enumerate().collect()
        } else {
            tabs.get(active).map(|t| (active, t)).into_iter().collect()
        };
        let views: Vec<(usize, Option<&'static str>, gpui_kit::AnyElement)> = shown
            .into_iter()
            .map(|(ix, t)| {
                let view = self.tab_body(
                    format!("{id}-tab-{ix}").into(),
                    t,
                    prepared.bodies[ix].clone(),
                    &prepared.questions,
                    issue,
                    cx,
                );
                (ix, t.read_only, view)
            })
            .collect();
        let titles: Vec<(String, Option<&'static str>)> = tabs
            .iter()
            .map(|t| (t.title.clone(), t.read_only))
            .collect();
        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        // A read-only tab's body starts with one muted caption line.
        let body = move |ix: usize, read_only: Option<&'static str>, view: gpui_kit::AnyElement| {
            v_flex()
                .gap_2()
                .when_some(read_only, |this, shows| {
                    this.child(
                        div()
                            .id(("read-only-caption", ix))
                            .test_support()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("Read only · {shows}")),
                    )
                })
                .child(view)
        };
        let measure = self.measure_preview_width(tabs.len(), cx);
        v_flex()
            .relative()
            .flex_1()
            .min_h_0()
            .child(measure)
            // The panel's header holds the tabs, or the title when one tab
            // is the whole description, and the Edit and Preview buttons.
            // Columns: one title row that stays put, then the columns. Each
            // column fills the panel's height and scrolls on its own. The tab
            // that `gt` / `gT` picks has the brighter title, and the scroll
            // keys move its column.
            .map(|this| {
                if columns {
                    this.child(Self::render_column_titles(&titles, active, toggle, cx))
                } else if titles.len() > 1 {
                    this.child(
                        crate::task::tab_bar::TabBar {
                            titles: &titles,
                            active,
                            toggle,
                            events: self.events.clone(),
                            room: self.tab_room.clone(),
                            view: cx.entity_id(),
                            hover: self.tab_hover.clone(),
                        }
                        .render(window, cx),
                    )
                } else {
                    this.child(description_title_row(toggle, cx))
                }
            })
            .child(
                v_flex()
                    .id("preview")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .when(!columns, |this| {
                        this.overflow_y_scroll().track_scroll(&self.preview_scroll)
                    })
                    .map(|this| {
                        if loading {
                            this.px_4()
                                .py_3()
                                .items_center()
                                .justify_center()
                                .text_sm()
                                .text_color(muted)
                                .child("Loading")
                        } else if empty {
                            // Centered both ways, so it reads as a state, not
                            // as content.
                            this.px_4().py_3().items_center().justify_center().child(
                                v_flex()
                                    .items_center()
                                    .gap_1()
                                    .text_sm()
                                    .text_color(muted)
                                    .child("No description")
                                    .child(div().text_xs().child(if locked {
                                        "An agent holds this task"
                                    } else {
                                        "Press e to write one"
                                    })),
                            )
                        } else if columns {
                            let scroll = self.preview_scroll.clone();
                            this.child(h_flex().size_full().min_h_0().items_stretch().children(
                                views.into_iter().map(move |(ix, read_only, view)| {
                                    div()
                                        .id(("preview-column", ix))
                                        .test_support()
                                        .flex_1()
                                        .min_w_0()
                                        .h_full()
                                        .overflow_y_scroll()
                                        .when(ix == active, |d| d.track_scroll(&scroll))
                                        .px_4()
                                        .py_3()
                                        .when(ix > 0, |d| d.border_l_1().border_color(border))
                                        .child(body(ix, read_only, view))
                                }),
                            ))
                        } else {
                            // With animations on, a newly picked tab's
                            // content fades in and rises a little. Keyed by
                            // the tab, so moving to another task on the
                            // same tab does not replay it.
                            this.px_4().py_3().children(views.into_iter().map(
                                |(ix, read_only, view)| {
                                    crate::shell::motion::swap_in(
                                        body(ix, read_only, view),
                                        ("tab-swap", ix),
                                        cx,
                                    )
                                },
                            ))
                        }
                    }),
            )
    }

    /// The server's lines for the conflict view, from the cache while
    /// neither text changed.
    fn conflict_lines(
        &self,
        theirs: &SharedString,
        mine: SharedString,
    ) -> Rc<[(SharedString, bool)]> {
        let mut cache = self.conflict_lines.borrow_mut();
        if let Some(c) = cache
            .as_ref()
            .filter(|c| &c.theirs == theirs && c.mine == mine)
        {
            return c.lines.clone();
        }
        let mine_lines: std::collections::HashSet<&str> = mine.lines().collect();
        let lines: Rc<[(SharedString, bool)]> = theirs
            .lines()
            .map(|line| {
                let text = if line.is_empty() {
                    SharedString::from(" ")
                } else {
                    SharedString::from(line.to_string())
                };
                (text, !mine_lines.contains(line))
            })
            .collect();
        *cache = Some(ConflictLines {
            theirs: theirs.clone(),
            mine: mine.clone(),
            lines: lines.clone(),
        });
        lines
    }

    /// The save conflict view: what to do, then the owner's text in the
    /// editor on the left and the server's version on the right. Server
    /// lines that the owner's text lacks are shaded.
    fn render_conflict(
        &self,
        issue: &Issue,
        editor: gpui_kit::AnyElement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let mine = self.editor.read(cx).value();
        let shown = self.conflict_lines(&issue.description, mine);
        let theme = cx.theme();
        let changed_bg = theme.secondary_active;
        let lines: Vec<_> = shown
            .iter()
            .map(|(line, changed)| {
                div()
                    .w_full()
                    .px_3()
                    .when(*changed, |d| d.bg(changed_bg))
                    .child(line.clone())
            })
            .collect();
        let choice = |id: &'static str, label: &'static str, tooltip: &'static str, ghost: bool| {
            let events = self.events.clone();
            let button = Button::new(id)
                .xsmall()
                .label(label)
                .when(ghost, |b| b.ghost())
                .on_click(move |_, window, cx| {
                    events.emit(
                        ViewEvent::RunCommand(
                            if id == "conflict-mine" {
                                "mine"
                            } else {
                                "theirs"
                            }
                            .to_string(),
                        ),
                        window,
                        cx,
                    );
                });
            with_card(format!("{id}-tip"), button, Card::text(tooltip))
        };
        let column_title = |title: &'static str| {
            div()
                .h_7()
                .flex_none()
                .flex()
                .items_center()
                .px_3()
                .text_xs()
                .text_color(theme.muted_foreground)
                .border_b_1()
                .border_color(theme.border)
                .child(title)
        };
        let bar = h_flex()
            .flex_none()
            .w_full()
            .min_w_0()
            .items_center()
            .px_3()
            .py_1p5()
            .gap_2()
            .text_xs()
            .bg(theme.secondary)
            .border_b_1()
            .border_color(theme.border)
            .child(Icon::new(IconName::GitCompare).xsmall())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child("The text changed on the server while you edited it. Edit yours to merge, then press Esc to save it."),
            )
            .child(choice(
                "conflict-mine",
                "Keep mine",
                "Save your text over the server's version (:mine)",
                false,
            ))
            .child(choice(
                "conflict-theirs",
                "Keep theirs",
                "Drop your text and keep the server's version (:theirs)",
                true,
            ));
        let mono = theme.mono_font_family.clone();
        let border = theme.border;
        let mine_title = column_title("Your text");
        let theirs_title = column_title("Server version");
        v_flex().flex_1().min_h_0().child(bar).child(
            h_flex()
                .flex_1()
                .min_h_0()
                .items_start()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(mine_title)
                        .child(editor),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .border_l_1()
                        .border_color(border)
                        .child(theirs_title)
                        .child(
                            v_flex()
                                .id("conflict-theirs")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .py_1()
                                .font_family(mono)
                                .text_sm()
                                .children(lines),
                        ),
                ),
        )
    }

    fn render_editor(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut context = KeyContext::default();
        context.add(vim_editor::CONTEXT);
        context.set("vim", self.editor_vim.name());

        // Each vim action runs its planned command.
        macro_rules! vim_actions {
            ($el:expr, $($action:ident),*) => {
                $el$(.on_action(cx.listener(|this, _: &vim_editor::$action, window, cx| {
                    this.run_vim(Command::$action, window, cx)
                })))*
            };
        }
        // In normal mode, the editor's own edit actions must not run. They
        // are stopped in the capture phase, before they reach the editor.
        // Cut and Paste stay: only Cmd-X and Cmd-V send them, and every
        // text field takes those keys, as in other macOS apps.
        macro_rules! block_in_normal {
            ($el:expr, $($action:ty),*) => {
                $el$(.capture_action(cx.listener(|this, _: &$action, _, cx| {
                    if this.editor_vim == VimMode::Normal {
                        cx.stop_propagation();
                    }
                })))*
            };
        }

        let wrapper = div()
            .flex_1()
            .min_h_0()
            .key_context(context)
            // Letters with no normal-mode binding stop here. GPUI types a
            // character only when its key-down event propagates.
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let m = &event.keystroke.modifiers;
                if this.editor_vim == VimMode::Normal && !m.platform && !m.control {
                    cx.stop_propagation();
                }
            }));
        let wrapper = vim_actions!(
            wrapper,
            Insert,
            Append,
            AppendEnd,
            InsertStart,
            OpenBelow,
            OpenAbove,
            DeleteChar,
            DeleteLine,
            DeleteToEnd,
            YankLine,
            Paste,
            LineStart,
            FirstNonBlank,
            LineEnd
        );
        block_in_normal!(
            wrapper,
            ed::Backspace,
            ed::Delete,
            ed::Enter,
            ed::Indent,
            ed::Outdent,
            ed::IndentInline,
            ed::OutdentInline,
            ed::DeleteToEndOfLine,
            ed::DeleteToBeginningOfLine,
            ed::DeleteToNextWordEnd,
            ed::DeleteToPreviousWordStart
        )
        .child(Editor::new(&self.editor).bordered(false).h_full())
    }

    /// A canvas that records the preview's width after layout.
    fn measure_preview_width(
        &self,
        tab_count: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        // Records the width after layout. When it changes which layout
        // fits, the task view draws again with the right one.
        let width = self.preview_width.clone();
        let entity = cx.entity_id();
        gpui_kit::canvas(
            move |bounds, window, cx| {
                let old = width.replace(bounds.size.width);
                let rem = window.rem_size();
                if Self::columns_fit(old, tab_count, rem)
                    != Self::columns_fit(bounds.size.width, tab_count, rem)
                {
                    cx.notify(entity);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }

    /// The titles above the columns of a wide preview. The tab that `gt`
    /// and `gT` pick has the brighter title.
    fn render_column_titles(
        titles: &[(String, Option<&'static str>)],
        active: usize,
        toggle: Option<gpui_kit::AnyElement>,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let last = titles.len().saturating_sub(1);
        let mut toggle = toggle;
        let theme = cx.theme();
        let border = theme.border;
        let (fg, muted) = (theme.foreground, theme.muted_foreground);
        let eye = move || read_only_eye(muted);
        let why = read_only_card;
        h_flex()
            .id("preview-titles")
            .test_support()
            .flex_none()
            .w_full()
            .h_8()
            .text_sm()
            .border_b_1()
            .border_color(border)
            .children(titles.iter().enumerate().map(|(ix, (title, read_only))| {
                h_flex()
                    .id(("preview-title", ix))
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap_1()
                    .px_4()
                    .when(ix > 0, |d| d.border_l_1().border_color(border))
                    .text_color(if ix == active { fg } else { muted })
                    .when(read_only.is_some(), |d| d.child(eye()).tooltip(why(title)))
                    .child(
                        h_flex().flex_1().min_w_0().child(
                            div()
                                .min_w_0()
                                .truncate()
                                .when(read_only.is_some(), |d| widgets::card_underline(d, cx))
                                .child(title.clone()),
                        ),
                    )
                    // The last column's title ends with the Edit and
                    // Preview buttons, so the titles stay over their columns.
                    .when(ix == last, |d| d.pr_2().children(toggle.take()))
            }))
    }

    /// The Edit and Preview buttons of the description panel. A locked
    /// task is read-only, so it shows only the preview and no buttons. The
    /// claim bar says who holds it. The selected button is the mode the
    /// task view is in.
    fn mode_toggle(&self, detail: Detail, issue: &Issue, cx: &App) -> Option<gpui_kit::AnyElement> {
        issue.claim.is_none().then(|| {
            h_flex()
                .id("mode-toggle")
                .test_support()
                .flex_none()
                .gap_0p5()
                .child(with_card(
                    "mode-edit-tip",
                    self.mode_button("mode-edit", "Edit", detail.mode == DetailMode::Editing, cx),
                    Card::text("Edit the description (e)"),
                ))
                .child(with_card(
                    "mode-preview-tip",
                    self.mode_button(
                        "mode-preview",
                        "Preview",
                        detail.mode == DetailMode::Preview,
                        cx,
                    ),
                    Card::text("Save and show the preview (Esc)"),
                ))
                .into_any_element()
        })
    }

    /// The task view's top row: status, key, title and who it waits on.
    /// The meta line sits right under it, in the same block.
    fn render_detail_header(
        &self,
        issue: &Issue,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let issue_id = issue.id;
        let key = widgets::issue_key(
            SharedString::from(format!("detail-key-{issue_id}")).into(),
            issue,
            self.events.clone(),
            cx.theme().muted_foreground,
            cx,
        )
        .into_any_element();
        let theme = cx.theme();
        // The panel sits inside the focus-ring padding and border, so the
        // row is that much shorter. A one-line title then sits on the filter
        // bar's line on the left. A long title wraps and the row grows down.
        let row = widgets::top_bar_height(window) - widgets::focus_inset(window);
        let line = window.rem_size() * HEADER_LINE_REM;
        // Each item beside the title is one line tall, so it stays on the
        // title's first line when the title wraps.
        let on_first_line = |id: &'static str, child: gpui_kit::AnyElement| {
            h_flex()
                .id(id)
                .test_support()
                .flex_none()
                .h(line)
                .items_center()
                .child(child)
        };
        h_flex()
            .id("detail-header")
            .test_support()
            .min_h(row)
            .flex_none()
            .items_start()
            .px_3()
            .py((row - line) / 2.)
            .gap_2()
            .text_sm()
            .line_height(line)
            .child(on_first_line(
                "detail-status",
                with_card(
                    SharedString::from(format!("detail-status-{issue_id}")),
                    Icon::new(issue.status.icon())
                        .small()
                        .when(issue.status.is_done(), |i| i.text_color(theme.success)),
                    Card::text(format!("Status: {}", issue.status.name)),
                )
                .into_any_element(),
            ))
            .child(on_first_line("detail-key", key))
            .child(
                div()
                    .id("detail-title")
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .child(issue.title.clone()),
            )
            .children(
                widgets::waiting_on(
                    SharedString::from(format!("detail-waiting-{issue_id}")).into(),
                    issue.waiting_on,
                    cx,
                )
                .map(|w| on_first_line("detail-waiting", w.into_any_element())),
            )
    }

    /// The second line under the title: the labels, the cost, time and
    /// estimates, and the dependencies. Each is one group. The line wraps
    /// on a narrow task view, and a group wraps inside itself too.
    fn render_meta_line(&self, issue: &Issue, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let labels = self.render_labels_group(issue, cx);
        let cost = self.render_cost_line(issue, cx);
        let deps = self.render_dependencies(issue, cx);
        h_flex()
            .id("detail-meta")
            .test_support()
            .w_full()
            .min_w_0()
            .flex_wrap()
            .items_center()
            .gap_x_4()
            .gap_y_1()
            .px_3()
            .pb_2()
            .text_xs()
            .child(labels)
            .child(cost)
            .child(deps)
    }

    /// The label tags, then the Labels button that changes them.
    fn render_labels_group(
        &self,
        issue: &Issue,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let issue_id = issue.id;
        let tags: Vec<_> = widgets::tags(issue, cx).collect();
        let store = self.store.clone();
        let label_events = self.events.clone();
        meta_group()
            .id("labels-group")
            .test_support()
            .children(tags)
            .child(with_card(
                "labels-tip",
                Button::new("labels")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Tag)
                    .label("Labels")
                    .dropdown_menu(move |menu, _, cx| {
                        let store = store.read(cx);
                        let Some(current) = store.get(issue_id).map(|i| i.labels.clone()) else {
                            return menu;
                        };
                        let labels = store.catalog.labels.clone();
                        labels
                            .into_iter()
                            .fold(menu.scrollable(true), |menu, label| {
                                let events = label_events.clone();
                                menu.item(
                                    PopupMenuItem::new(label.name.clone())
                                        .checked(current.contains(&label))
                                        .on_click(move |_, window, cx| {
                                            events.emit(
                                                ViewEvent::OnIssue(
                                                    issue_id,
                                                    IssueAction::ToggleLabel(label.clone()),
                                                ),
                                                window,
                                                cx,
                                            );
                                        }),
                                )
                            })
                    }),
                Card::text("Add or remove labels (:label)"),
            ))
    }
}

impl Render for TaskView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.tab_hover.update(cx, |hover, _| hover.start_frame());
        #[cfg(test)]
        crate::render_count::bump::<Self>();
        let Some(detail) = self.open else {
            return div().into_any_element();
        };
        let Some(issue) = self.store.read(cx).get(detail.issue).cloned() else {
            return div().into_any_element();
        };
        // The answer fields of owner questions are made here,
        // where the window can make them. The Owner questions tab reads them.
        let answers = self.answers.clone();
        let questions = self.prepared(&issue, cx).questions;
        for q in questions.iter() {
            answers.update(cx, |a, cx| {
                a.input(issue.id, q, window, cx);
            });
        }
        if issue.status.review {
            answers.update(cx, |a, cx| {
                a.feedback_input(issue.id, window, cx);
            });
        }
        self.render_detail(detail, &issue, window, cx)
            .into_any_element()
    }
}

/// Whether a git panel has anything to show.
fn has_git(g: &IssueGit) -> bool {
    g.branch.is_some() || g.worktree.is_some() || g.pr.is_some() || !g.commits.is_empty()
}

/// The eye before the title of a read-only tab.
pub(super) fn read_only_eye(muted: gpui_kit::Hsla) -> Icon {
    Icon::new(IconName::Eye).xsmall().text_color(muted)
}

/// The card that says why a read-only tab cannot be edited.
pub(super) fn read_only_card(
    title: &str,
) -> impl Fn(&mut Window, &mut App) -> gpui_kit::AnyView + 'static + use<> {
    Card::titled(format!("{title} is read only"))
        .body(crate::task::history::READ_ONLY_WHY)
        .builder()
}

/// One row of the git panel: an icon, then whatever the caller adds. The
/// icon's card names what the row shows, such as "Branch".
fn git_row(
    id: impl Into<gpui_kit::ElementId>,
    icon: IconName,
    what: impl Into<SharedString>,
    muted: gpui_kit::Hsla,
) -> gpui_kit::Div {
    h_flex()
        .w_full()
        .min_w_0()
        .items_start()
        .gap_2()
        .child(div().flex_none().pt_0p5().child(with_card(
            id,
            Icon::new(icon).xsmall().text_color(muted),
            Card::text(what),
        )))
}

/// Monospace text in a git panel row.
fn git_text(s: String, mono: &SharedString, fg: gpui_kit::Hsla) -> gpui_kit::Div {
    div()
        .flex_1()
        .min_w_0()
        .font_family(mono.clone())
        .text_color(fg)
        .child(s)
}

/// The git panel's rows for the branch, the worktree and the PR.
fn git_fact_rows(git: &IssueGit, cx: &App) -> Vec<gpui_kit::AnyElement> {
    let theme = cx.theme();
    let mono = theme.mono_font_family.clone();
    let (fg, muted) = (theme.foreground, theme.muted_foreground);
    let text = |s: String| git_text(s, &mono, fg);
    let mut rows = Vec::new();
    if let Some(b) = &git.branch {
        rows.push(
            git_row("git-branch-icon", IconName::GitBranch, "Branch", muted)
                .child(text(b.clone()))
                .into_any_element(),
        );
    }
    if let Some(w) = &git.worktree {
        let mut place = crate::model::data::shorten_home(w);
        if let Some(h) = &git.host {
            place = format!("{place} on {h}");
        }
        rows.push(
            git_row("git-worktree-icon", IconName::FolderGit, "Worktree", muted)
                .child(text(place))
                .into_any_element(),
        );
    }
    if let Some(pr) = &git.pr {
        let label = match (pr.number, &pr.title) {
            (_, Some(t)) => t.clone(),
            (Some(n), None) => format!("#{n}"),
            (None, None) => pr.url.clone(),
        };
        let (icon, what) = match pr.state.as_deref() {
            Some("merged") => (IconName::GitMerge, "Merged pull request"),
            Some("closed") => (IconName::GitPullRequestClosed, "Closed pull request"),
            Some("draft") => (IconName::GitPullRequestDraft, "Draft pull request"),
            _ => (IconName::GitPullRequest, "Pull request"),
        };
        rows.push(
            git_row("git-pr-icon", icon, what, muted)
                .child(
                    crate::shell::widgets::link("git-pr", pr.url.clone(), cx)
                        .min_w_0()
                        .child(label),
                )
                .when_some(pr.state.clone(), |this, state| {
                    this.child(div().flex_none().text_color(muted).child(state))
                })
                .into_any_element(),
        );
    }
    rows
}

/// The git panel's rows for the first `shown` commits.
fn git_commit_rows(
    git: &IssueGit,
    repo: Option<&str>,
    shown: usize,
    cx: &App,
) -> Vec<gpui_kit::AnyElement> {
    let theme = cx.theme();
    let mono = theme.mono_font_family.clone();
    let (fg, muted) = (theme.foreground, theme.muted_foreground);
    let text = |s: String| git_text(s, &mono, fg);
    git.commits
        .iter()
        .take(shown)
        .enumerate()
        .map(|(ix, c)| {
            let (hash, subject) = c.split_once(' ').unwrap_or((c.as_str(), ""));
            let url = repo.and_then(|r| llmtodo_client::git::commit_url(r, hash));
            let row = git_row(
                ("git-commit-icon", ix),
                IconName::GitCommitHorizontal,
                "Commit",
                muted,
            );
            match url {
                Some(url) => row
                    .child(
                        div()
                            .id(("git-commit", ix))
                            .test_support()
                            .flex_none()
                            .child(
                                crate::shell::widgets::link(("git-commit-link", ix), url, cx)
                                    .font_family(mono.clone())
                                    .child(hash.to_string()),
                            ),
                    )
                    .child(text(subject.to_string())),
                // No known remote: the hash stays plain text.
                None => row.child(text(c.clone())),
            }
            .into_any_element()
        })
        .collect()
}

/// The small text button that shows or hides the rest of a long list.
fn more_toggle<F>(
    id: &'static str,
    label: String,
    hover: gpui_kit::Hsla,
    pressed: gpui_kit::Hsla,
    on_click: F,
    cx: &App,
) -> impl IntoElement + use<F>
where
    F: Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
{
    let theme = cx.theme();
    div()
        .id(id)
        .test_support()
        .px_1()
        .rounded(theme.radius)
        .cursor_pointer()
        .text_color(theme.muted_foreground)
        .hover(move |s| s.bg(hover))
        .active(move |s| s.bg(pressed))
        .child(label)
        .on_click(on_click)
}

/// The estimates as one short text, such as `90m · $4.50`. None when the
/// task has neither.
pub(crate) fn estimate_text(minutes: Option<u32>, cost: Option<f64>) -> String {
    if minutes.is_none() && cost.is_none() {
        return MISSING.to_string();
    }
    let time = minutes.map_or(MISSING.to_string(), |m| {
        llmtodo_client::types::format_active_time(u64::from(m) * 60)
    });
    let cost = cost.map_or(MISSING.to_string(), crate::model::data::format_cost);
    format!("{time} · {cost}")
}

/// The text of a value that has no data yet.
const MISSING: &str = "—";

/// The line height of the task view's title row, in rems. The items beside
/// the title are this tall, so they sit on its first line.
const HEADER_LINE_REM: f32 = 1.25;

/// One group of the meta line. Its items sit close and wrap inside it.
fn meta_group() -> gpui_kit::Div {
    h_flex()
        .flex_wrap()
        .items_center()
        .max_w_full()
        .min_w_0()
        .gap_1()
}

/// A section title: a small icon and a medium-weight name. Every panel of
/// the task view starts with one, so each panel reads as its own section.
pub(crate) fn section_title(
    icon: IconName,
    title: impl Into<SharedString>,
    cx: &App,
) -> gpui_kit::Div {
    let theme = cx.theme();
    h_flex()
        .gap_2()
        .text_sm()
        .child(Icon::new(icon).xsmall().text_color(theme.muted_foreground))
        .child(
            div()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .child(title.into()),
        )
}

/// The description panel's header when it shows no tabs: its title, and
/// the Edit and Preview buttons on the right.
fn description_title_row(toggle: Option<gpui_kit::AnyElement>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .id("description-header")
        .test_support()
        .flex_none()
        .w_full()
        .h_8()
        .px_4()
        .pr_2()
        .justify_between()
        .items_center()
        .border_b_1()
        .border_color(theme.border)
        .child(section_title(IconName::FileText, "Description", cx))
        .children(toggle)
}
