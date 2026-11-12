//! The owner inbox: every open owner question across issues, in one place.
//!
//! Agents ask in a `+++ Owner questions` section of a description (see
//! `llmtodo_core::questions`). The inbox lists the open ones from the
//! store, which has them from the server's question list. The app does not
//! hold every description. A choice shows
//! its options as buttons, and every question shows an input. The
//! task view draws the same cards in its Owner questions tab, so a question
//! reads and answers the same way in both places.
//!
//! The view sends nothing itself: an answer goes through
//! the workspace, which updates the app's copy and the server.

use std::collections::HashMap;

use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, h_flex,
    input::{InputEvent, InputState, TextareaState},
    text::TextView,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, actions, div,
    prelude::FluentBuilder as _,
};
use llmtodo_core::Question;

use crate::model::data::IssueId;
use crate::model::store::IssueStore;
use crate::shell::view_events::{ViewEvent, ViewEvents};

pub const CONTEXT: &str = "Inbox";

actions!(
    llmtodo_inbox,
    [
        InboxNext,
        InboxPrev,
        InboxTop,
        InboxBottom,
        /// Open the selected question's issue in the task view.
        InboxOpenIssue,
        /// Focus the answer field of the selected question.
        InboxType,
        /// Take back the answer just sent.
        InboxUndoAnswer,
        Choose1,
        Choose2,
        Choose3,
        Choose4,
        Choose5,
        Choose6,
        Choose7,
        Choose8,
        Choose9,
    ]
);

pub fn bind_keys(cx: &mut App) {
    use gpui_kit::KeyBinding;
    // Only in normal mode: while an answer field has focus, the keys type.
    let ctx = Some("(Workspace && mode == normal) > Inbox");
    cx.bind_keys([
        KeyBinding::new("j", InboxNext, ctx),
        KeyBinding::new("down", InboxNext, ctx),
        KeyBinding::new("ctrl-n", InboxNext, ctx),
        KeyBinding::new("k", InboxPrev, ctx),
        KeyBinding::new("up", InboxPrev, ctx),
        KeyBinding::new("ctrl-p", InboxPrev, ctx),
        KeyBinding::new("g g", InboxTop, ctx),
        KeyBinding::new("shift-g", InboxBottom, ctx),
        KeyBinding::new("o", InboxOpenIssue, ctx),
        KeyBinding::new("enter", InboxType, ctx),
        KeyBinding::new("i", InboxType, ctx),
        KeyBinding::new("u", InboxUndoAnswer, ctx),
        KeyBinding::new("1", Choose1, ctx),
        KeyBinding::new("2", Choose2, ctx),
        KeyBinding::new("3", Choose3, ctx),
        KeyBinding::new("4", Choose4, ctx),
        KeyBinding::new("5", Choose5, ctx),
        KeyBinding::new("6", Choose6, ctx),
        KeyBinding::new("7", Choose7, ctx),
        KeyBinding::new("8", Choose8, ctx),
        KeyBinding::new("9", Choose9, ctx),
    ]);
}

/// One open question with the issue it sits on.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub issue: IssueId,
    pub key: SharedString,
    pub title: SharedString,
    pub question: Question,
}

/// Every open question on every open issue, oldest issue first, then in
/// the order the agent asked them.
pub fn entries(store: &IssueStore) -> Vec<Entry> {
    let mut issues: Vec<_> = store.with_open_questions().map(|(i, _)| i).collect();
    issues.sort_by(|a, b| a.added.cmp(&b.added).then_with(|| a.key.cmp(&b.key)));
    issues
        .into_iter()
        .flat_map(|i| {
            store.open_questions_of(i.id).iter().map(|q| Entry {
                issue: i.id,
                key: i.key.clone(),
                title: i.title.clone(),
                question: q.clone(),
            })
        })
        .collect()
}

/// The open questions across issues, for the sidebar count.
pub fn open_count(store: &IssueStore) -> usize {
    store.open_question_count()
}

/// The answer fields of the questions, one per question, kept across
/// frames. The inbox and the task view share them, so text typed in one
/// shows in the other. Enter in a field sends the answer.
pub struct AnswerInputs {
    events: ViewEvents,
    inputs: HashMap<(IssueId, u32), (Entity<InputState>, Subscription)>,
    /// The review feedback field of each ticket in User review.
    feedback: HashMap<IssueId, Entity<TextareaState>>,
    /// Feedback fields to empty on their next draw, after the server took
    /// the answer. Emptying a field needs the window.
    clear_feedback: std::collections::HashSet<IssueId>,
    /// Drops fields the app no longer shows whenever the store changes.
    _store: Subscription,
}

impl AnswerInputs {
    pub fn new(
        events: ViewEvents,
        store: &Entity<IssueStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = cx.observe_in(store, window, |this, store, window, cx| {
            this.prune(store.read(cx), window, cx);
        });
        Self {
            events,
            inputs: HashMap::new(),
            feedback: HashMap::new(),
            clear_feedback: Default::default(),
            _store: store,
        }
    }

    /// Drops the answer fields of questions that are no longer open, and
    /// the feedback fields of tickets that left the review status or the
    /// store. A field with focus or with unsent text stays, so nothing the
    /// owner typed is lost. The views make a field again when they need it.
    pub fn prune(&mut self, store: &IssueStore, window: &Window, cx: &App) {
        let in_use = |focus: FocusHandle, text: &str| {
            focus.contains_focused(window, cx) || !text.trim().is_empty()
        };
        self.inputs.retain(|&(issue, number), (input, _)| {
            store
                .open_questions_of(issue)
                .iter()
                .any(|q| q.number == number)
                || in_use(input.focus_handle(cx), &input.read(cx).value())
        });
        self.feedback.retain(|&issue, input| {
            store
                .get(issue)
                .is_some_and(|i| !i.deleted && i.status.review)
                || in_use(input.focus_handle(cx), &input.read(cx).value())
        });
        let feedback = &self.feedback;
        self.clear_feedback
            .retain(|issue| feedback.contains_key(issue));
    }

    /// How many answer fields and feedback fields it holds, for tests.
    #[cfg(test)]
    pub fn sizes(&self) -> (usize, usize) {
        (self.inputs.len(), self.feedback.len())
    }

    /// The field for question `number` of `issue`, made on first use. A
    /// choice gets one too, for an answer that none of its options gives.
    pub fn input(
        &mut self,
        issue: IssueId,
        q: &Question,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let number = q.number;
        if let Some((input, _)) = self.inputs.get(&(issue, number)) {
            return input.clone();
        }
        let placeholder = if q.is_choice() {
            "Or type your own answer"
        } else {
            "Your answer"
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let events = self.events.clone();
        let sub = cx.subscribe_in(&input, window, move |_, state, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let text = state.read(cx).value().to_string();
                let events = events.clone();
                // After this event: the answer clears this field, which
                // this entity holds while the event runs.
                window.defer(cx, move |window, cx| {
                    events.emit(
                        ViewEvent::AnswerQuestion {
                            issue,
                            number,
                            answer: text,
                        },
                        window,
                        cx,
                    );
                });
            }
        });
        self.inputs.insert((issue, number), (input.clone(), sub));
        input
    }

    /// The review feedback field of `issue`, made on first use.
    pub fn feedback_input(
        &mut self,
        issue: IssueId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextareaState> {
        let input = self
            .feedback
            .entry(issue)
            .or_insert_with(|| {
                cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .auto_grow(2, 6)
                        .placeholder("Feedback. Needed for Request changes")
                })
            })
            .clone();
        if self.clear_feedback.remove(&issue) {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        input
    }

    /// Empties the feedback field of `issue` on its next draw. The views
    /// that draw it observe this entity, so the notify draws them.
    pub fn clear_feedback_later(&mut self, issue: IssueId, cx: &mut Context<Self>) {
        self.clear_feedback.insert(issue);
        cx.notify();
    }

    pub fn feedback(&self, issue: IssueId) -> Option<Entity<TextareaState>> {
        self.feedback.get(&issue).cloned()
    }

    /// The feedback text of `issue`, trimmed.
    pub fn feedback_text(&self, issue: IssueId, cx: &App) -> String {
        self.feedback
            .get(&issue)
            .map(|i| i.read(cx).value().trim().to_string())
            .unwrap_or_default()
    }

    pub fn get(&self, issue: IssueId, number: u32) -> Option<Entity<InputState>> {
        self.inputs.get(&(issue, number)).map(|(i, _)| i.clone())
    }

    /// Whether any answer field has focus, so typing goes to it.
    pub fn focused(&self, window: &Window, cx: &App) -> bool {
        self.inputs
            .values()
            .any(|(i, _)| i.focus_handle(cx).is_focused(window))
            || self
                .feedback
                .values()
                .any(|i| i.focus_handle(cx).is_focused(window))
    }

    /// Empties the field after its answer was sent.
    pub fn clear(&mut self, issue: IssueId, number: u32, window: &mut Window, cx: &mut App) {
        if let Some((input, _)) = self.inputs.get(&(issue, number)) {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
    }
}

/// The border of a question card. It alone marks the selected card, so the
/// keyboard selection stays visible without a fill.
pub fn card_border(selected: bool, cx: &App) -> gpui_kit::Hsla {
    let theme = cx.theme();
    if selected {
        theme.list_active_border
    } else {
        theme.border
    }
}

/// The letter square's side. Inside the 2rem button, it leaves the same
/// space on all four sides as the button's horizontal padding and border.
const LETTER_SQUARE: gpui_kit::Rems = gpui_kit::rems(1.125);

/// One answer option: the letter in a small square, then the option's text.
/// The letter and the label are parameters so a test can draw through the
/// real button.
pub fn option_button(
    id: SharedString,
    picked: bool,
    letter: impl IntoElement,
    label: impl IntoElement,
    cx: &App,
) -> gpui_kit::component::button::Button {
    use crate::shell::widgets::{INBOX_BUTTON_GROUP, inbox_button, letter_colors};
    let rest = letter_colors(picked, false, cx);
    let hover = letter_colors(picked, true, cx);
    // The letter sits bold in a small square, so it reads as a key. The ID
    // is required: GPUI picks text color during layout, and an element with
    // no ID has no hover state to read from then. Its `group_hover` text
    // color would be ignored.
    let square = div()
        .id(SharedString::from(format!("{id}-letter")))
        .test_support()
        .flex_none()
        .size(LETTER_SQUARE)
        .flex()
        .items_center()
        .justify_center()
        .rounded(cx.theme().radius)
        .bg(rest.fill)
        .text_color(rest.letter)
        .group_hover(INBOX_BUTTON_GROUP, move |s| {
            s.bg(hover.fill).text_color(hover.letter)
        })
        .text_xs()
        .font_weight(gpui_kit::FontWeight::BOLD)
        .child(letter);
    inbox_button(id, picked, cx)
        .px_1p5()
        .toggled(picked)
        .child(h_flex().gap_1p5().child(square).child(label))
}

/// One question card: the question, its context, and the controls that
/// answer it. `input` is the free-text field. A choice shows it under its
/// options, for a custom answer.
pub fn question_card(
    issue: IssueId,
    q: &Question,
    input: Option<Entity<InputState>>,
    selected: bool,
    events: ViewEvents,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let number = q.number;
    let chosen = q.chosen();
    let id_base = format!("q-{issue}-{number}");
    let options = q.options.iter().enumerate().map(|(ix, o)| {
        let events = events.clone();
        let letter = o.letter;
        let picked = chosen == Some(letter);
        let opt_id = format!("{id_base}-opt-{letter}");
        option_button(
            SharedString::from(opt_id),
            picked,
            letter.to_string(),
            o.text.clone(),
            cx,
        )
        .when(ix < 9, |b| {
            b.tooltip(format!("Answer {letter} ({})", ix + 1))
        })
        .on_click(move |_, window, cx| {
            events.emit(
                ViewEvent::AnswerQuestion {
                    issue,
                    number,
                    answer: letter.to_string(),
                },
                window,
                cx,
            );
        })
    });
    let free_text = input.map(|input| {
        let events = events.clone();
        let send = input.clone();
        h_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .id(SharedString::from(format!("{id_base}-input")))
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .child(crate::shell::widgets::text_input(&input)),
            )
            .child(crate::shell::tooltip::with_card(
                SharedString::from(format!("{id_base}-send-tip")),
                crate::shell::widgets::inbox_button(
                    SharedString::from(format!("{id_base}-send")),
                    false,
                    cx,
                )
                .label("Answer")
                .on_click(move |_, window, cx| {
                    let text = send.read(cx).value().to_string();
                    events.emit(
                        ViewEvent::AnswerQuestion {
                            issue,
                            number,
                            answer: text,
                        },
                        window,
                        cx,
                    );
                }),
                crate::shell::tooltip::Card::text("Send the answer (Enter)"),
            ))
    });
    // A picked option shows as the selected button. Any other answer,
    // such as a custom answer to a choice, shows as text.
    let answered = q.answer.clone().filter(|_| chosen.is_none());
    v_flex()
        .id(SharedString::from(id_base.clone()))
        .test_support()
        .w_full()
        .gap_2()
        .p_3()
        .rounded(theme.radius)
        .border_1()
        .border_color(card_border(selected, cx))
        .child(
            h_flex()
                .gap_2()
                .items_start()
                .child(
                    div()
                        .flex_none()
                        .font_family(theme.mono_font_family.clone())
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(q.id.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .child(q.text.clone()),
                ),
        )
        .when(!q.context.is_empty(), |d| {
            d.child(
                div().text_color(muted).child(
                    TextView::markdown(
                        SharedString::from(format!("{id_base}-context")),
                        q.context.clone(),
                    )
                    .text_xs(),
                ),
            )
        })
        .when(q.is_choice(), |d| {
            d.child(h_flex().flex_wrap().gap_2().children(options))
        })
        .when_some(answered, |d, a| {
            d.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("Answer: {a}")),
            )
        })
        .children(free_text)
        .into_any_element()
}

pub struct InboxView {
    store: Entity<IssueStore>,
    events: ViewEvents,
    answers: Entity<AnswerInputs>,
    pub focus: FocusHandle,
    scroll: ScrollHandle,
    /// The selected question, by issue and number, so it stays put while
    /// other questions come and go.
    selected: Option<(IssueId, u32)>,
    /// Where the selection was, for when its question gets an answer and
    /// leaves the list: the next one takes its place.
    selected_ix: usize,
}

impl InboxView {
    pub fn new(
        store: Entity<IssueStore>,
        events: ViewEvents,
        answers: Entity<AnswerInputs>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        cx.observe(&answers, |_, _, cx| cx.notify()).detach();
        Self {
            store,
            events,
            answers,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            selected: None,
            selected_ix: 0,
        }
    }

    /// The selected entry's index, after questions came or went. The first
    /// question is selected when nothing else is.
    pub fn selection(&mut self, entries: &[Entry]) -> Option<usize> {
        if entries.is_empty() {
            self.selected = None;
            return None;
        }
        let ix = self
            .selected
            .and_then(|(i, n)| {
                entries
                    .iter()
                    .position(|e| e.issue == i && e.question.number == n)
            })
            .unwrap_or_else(|| self.selected_ix.min(entries.len() - 1));
        self.select(ix, entries);
        Some(ix)
    }

    fn select(&mut self, ix: usize, entries: &[Entry]) {
        if let Some(e) = entries.get(ix) {
            self.selected = Some((e.issue, e.question.number));
            self.selected_ix = ix;
        }
    }

    /// The selected question, as `(issue, number)`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected(&mut self, cx: &App) -> Option<(IssueId, u32)> {
        let entries = entries(self.store.read(cx));
        self.selection(&entries)
            .map(|ix| (entries[ix].issue, entries[ix].question.number))
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let entries = entries(self.store.read(cx));
        let Some(at) = self.selection(&entries) else {
            return;
        };
        let next = at.saturating_add_signed(delta).min(entries.len() - 1);
        self.select(next, &entries);
        self.scroll_to(next, &entries);
        cx.notify();
    }

    fn jump(&mut self, last: bool, cx: &mut Context<Self>) {
        let entries = entries(self.store.read(cx));
        if entries.is_empty() {
            return;
        }
        let ix = if last { entries.len() - 1 } else { 0 };
        self.select(ix, &entries);
        self.scroll_to(ix, &entries);
        cx.notify();
    }

    /// Scrolls the card of entry `ix` into view. Each issue adds one
    /// heading row above its first card, so the child index counts those.
    fn scroll_to(&self, ix: usize, entries: &[Entry]) {
        let headings = entries[..=ix]
            .iter()
            .enumerate()
            .filter(|(i, e)| *i == 0 || entries[i - 1].issue != e.issue)
            .count();
        self.scroll.scroll_to_item(ix + headings);
    }

    fn choose(&mut self, option: usize, window: &mut Window, cx: &mut Context<Self>) {
        let entries = entries(self.store.read(cx));
        let Some(ix) = self.selection(&entries) else {
            return;
        };
        let e = &entries[ix];
        let Some(o) = e.question.options.get(option) else {
            let msg = if e.question.is_choice() {
                format!("{} {} has no option {}", e.key, e.question.id, option + 1)
            } else {
                format!(
                    "{} {} takes text: press Enter to type the answer",
                    e.key, e.question.id
                )
            };
            self.events.emit(ViewEvent::ShowMessage(msg), window, cx);
            return;
        };
        let (issue, number, letter) = (e.issue, e.question.number, o.letter);
        self.events.emit(
            ViewEvent::AnswerQuestion {
                issue,
                number,
                answer: letter.to_string(),
            },
            window,
            cx,
        );
    }

    fn type_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = entries(self.store.read(cx));
        let Some(ix) = self.selection(&entries) else {
            return;
        };
        let e = &entries[ix];
        // A choice takes a typed answer too, when no option fits.
        let (issue, question) = (e.issue, e.question.clone());
        let input = self
            .answers
            .update(cx, |a, cx| a.input(issue, &question, window, cx));
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    /// Selects a question that just came back to the list.
    pub fn select_question(&mut self, issue: IssueId, number: u32, cx: &mut Context<Self>) {
        let entries = entries(self.store.read(cx));
        let found = entries
            .iter()
            .position(|e| e.issue == issue && e.question.number == number);
        if let Some(ix) = found {
            self.select(ix, &entries);
            self.scroll_to(ix, &entries);
            cx.notify();
        }
    }

    fn undo_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.events.emit(ViewEvent::UndoAnswer, window, cx);
    }

    fn open_issue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = entries(self.store.read(cx));
        let Some(ix) = self.selection(&entries) else {
            return;
        };
        let key = entries[ix].key.clone();
        self.events
            .emit(ViewEvent::OpenKey(key.to_string()), window, cx);
    }

    pub fn take_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        cx.notify();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn focused(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx) || self.answers.read(cx).focused(window, cx)
    }

    fn render_header(&self, open: usize, reviews: usize, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        // An empty inbox says so in the center of the view, so the header
        // shows no count then.
        let questions = match open {
            0 => None,
            1 => Some("1 open question".to_string()),
            n => Some(format!("{n} open questions")),
        };
        let to_review = (reviews > 0).then(|| format!("{reviews} to review"));
        let count = match (questions, to_review) {
            (Some(q), Some(r)) => Some(format!("{q}, {r}")),
            (q, r) => q.or(r),
        };
        h_flex()
            .h_10()
            .flex_none()
            .px_4()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                Icon::new(IconName::MessageCircleQuestionMark)
                    .small()
                    .text_color(theme.muted_foreground),
            )
            .child(div().flex_1().min_w_0().truncate().text_sm().child("Inbox"))
            .when_some(count, |this, count| {
                this.child(
                    div()
                        .id("inbox-count")
                        .test_support()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(count),
                )
            })
    }
}

impl Render for InboxView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::render_count::bump::<Self>();
        let entries = entries(self.store.read(cx));
        let selected = self.selection(&entries);
        let reviews: Vec<(IssueId, SharedString, SharedString)> = self
            .store
            .read(cx)
            .waiting_for_review()
            .into_iter()
            .map(|i| (i.id, i.key.clone(), i.title.clone()))
            .collect();
        let header = self.render_header(entries.len(), reviews.len(), cx);
        let theme = cx.theme();
        let (muted, border, radius) = (theme.muted_foreground, theme.border, theme.radius);
        let hover = theme.foreground.opacity(0.08);
        let pressed = theme.foreground.opacity(0.16);
        let mono = theme.mono_font_family.clone();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (ix, e) in entries.iter().enumerate() {
            if ix == 0 || entries[ix - 1].issue != e.issue {
                let events = self.events.clone();
                let key = e.key.clone();
                rows.push(
                    h_flex()
                        .id(SharedString::from(format!("inbox-issue-{}", e.issue)))
                        .test_support()
                        .w_full()
                        .gap_2()
                        .px_1()
                        .py_0p5()
                        .mt_2()
                        .rounded(radius)
                        .hover(move |s| s.bg(hover))
                        .active(move |s| s.bg(pressed))
                        .tooltip(crate::shell::tooltip::Card::text("Open the issue (o)").builder())
                        .on_click(move |_, window, cx| {
                            events.emit(ViewEvent::OpenKey(key.to_string()), window, cx);
                        })
                        .child(
                            div()
                                .font_family(mono.clone())
                                .text_xs()
                                .text_color(muted)
                                .child(e.key.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .child(e.title.clone()),
                        )
                        .into_any_element(),
                );
            }
            let input = Some(
                self.answers
                    .update(cx, |a, cx| a.input(e.issue, &e.question, window, cx)),
            );
            rows.push(question_card(
                e.issue,
                &e.question,
                input,
                selected == Some(ix),
                self.events.clone(),
                cx,
            ));
        }
        // Tickets in User review come after the questions, so the
        // keyboard selection and its scroll index stay the same.
        if !reviews.is_empty() {
            rows.push(
                div()
                    .id("inbox-review-heading")
                    .test_support()
                    .mt_3()
                    .px_1()
                    .text_xs()
                    .text_color(muted)
                    .child("Waiting for your review")
                    .into_any_element(),
            );
        }
        for (id, key, title) in reviews.iter().cloned() {
            let events = self.events.clone();
            let open = key.clone();
            rows.push(
                h_flex()
                    .id(SharedString::from(format!("inbox-review-{id}")))
                    .test_support()
                    .w_full()
                    .gap_2()
                    .px_1()
                    .py_0p5()
                    .rounded(radius)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .active(move |s| s.bg(pressed))
                    .tooltip(
                        crate::shell::tooltip::Card::text("Open the ticket to review it").builder(),
                    )
                    .on_click(move |_, window, cx| {
                        events.emit(ViewEvent::OpenKey(open.to_string()), window, cx);
                    })
                    .child(
                        Icon::new(IconName::ClipboardCheck)
                            .xsmall()
                            .text_color(muted),
                    )
                    .child(
                        div()
                            .font_family(mono.clone())
                            .text_xs()
                            .text_color(muted)
                            .child(key.clone()),
                    )
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(title))
                    .into_any_element(),
            );
        }
        let content = if entries.is_empty() && reviews.is_empty() {
            v_flex()
                .id("inbox-empty")
                .test_support()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_1()
                .text_sm()
                .text_color(muted)
                .child("Nothing for you")
                .child(
                    div()
                        .text_xs()
                        .child("Agent questions and tickets to review show here"),
                )
                .into_any_element()
        } else {
            v_flex()
                .id("inbox-list")
                .test_support()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .px_4()
                .pb_3()
                .gap_2()
                .children(rows)
                .into_any_element()
        };
        v_flex()
            .id("inbox-view")
            .test_support()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &InboxNext, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &InboxPrev, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &InboxTop, _, cx| this.jump(false, cx)))
            .on_action(cx.listener(|this, _: &InboxBottom, _, cx| this.jump(true, cx)))
            .on_action(
                cx.listener(|this, _: &InboxOpenIssue, window, cx| this.open_issue(window, cx)),
            )
            .on_action(cx.listener(|this, _: &InboxType, window, cx| this.type_answer(window, cx)))
            .on_action(
                cx.listener(|this, _: &InboxUndoAnswer, window, cx| this.undo_answer(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Choose1, w, cx| this.choose(0, w, cx)))
            .on_action(cx.listener(|this, _: &Choose2, w, cx| this.choose(1, w, cx)))
            .on_action(cx.listener(|this, _: &Choose3, w, cx| this.choose(2, w, cx)))
            .on_action(cx.listener(|this, _: &Choose4, w, cx| this.choose(3, w, cx)))
            .on_action(cx.listener(|this, _: &Choose5, w, cx| this.choose(4, w, cx)))
            .on_action(cx.listener(|this, _: &Choose6, w, cx| this.choose(5, w, cx)))
            .on_action(cx.listener(|this, _: &Choose7, w, cx| this.choose(6, w, cx)))
            .on_action(cx.listener(|this, _: &Choose8, w, cx| this.choose(7, w, cx)))
            .on_action(cx.listener(|this, _: &Choose9, w, cx| this.choose(8, w, cx)))
            .flex_1()
            .min_w_0()
            // The workspace draws this view cached, so GPUI lays it out as
            // its own root, where `flex_1` does nothing. It fills by size.
            .size_full()
            .border_color(border)
            .child(header)
            .child(content)
    }
}

#[cfg(test)]
pub(crate) mod tests;
