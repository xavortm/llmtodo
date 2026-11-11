//! The `c` form that creates an issue: a title, a description and a line that
//! says where the issue goes. The title may carry words that set fields, such
//! as `Fix the crash status:todo label:bug priority:high assignee:me`. Enter
//! in the title, Cmd-Enter anywhere or the Create button emits
//! `CreateEvent::Submit`, and the workspace sends the issue to the server.
//! Esc, Ctrl-C or the Cancel button closes the form.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputEvent, InputState, Textarea, TextareaState},
    v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, actions, div, relative, rems,
};
use llmtodo_api_types::Priority;

pub const CONTEXT: &str = "CreateForm";

actions!(create_form, [SwitchField]);

/// What the owner typed, split into the title and the field words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    pub title: String,
    pub status: Option<String>,
    pub labels: Vec<String>,
    pub priority: Option<String>,
    pub assignee: Option<String>,
}

impl Draft {
    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        let mut title = Vec::new();
        for word in text.split_whitespace() {
            let field = word
                .split_once(':')
                .map(|(k, v)| (k.to_lowercase(), v.to_string()));
            match field {
                Some((k, v)) if k == "status" && !v.is_empty() => out.status = Some(v),
                Some((k, v)) if k == "label" && !v.is_empty() => out.labels.push(v),
                Some((k, v)) if k == "priority" && !v.is_empty() => out.priority = Some(v),
                Some((k, v)) if k == "assignee" && !v.is_empty() => out.assignee = Some(v),
                _ => title.push(word),
            }
        }
        out.title = title.join(" ");
        out
    }
}

/// The priority a word names: `urgent`, `high`, `medium`, `low` or `none`.
pub fn priority(word: &str) -> Option<Priority> {
    Some(match word.to_lowercase().as_str() {
        "urgent" => Priority::Urgent,
        "high" => Priority::High,
        "medium" => Priority::Medium,
        "low" => Priority::Low,
        "none" => Priority::None,
        _ => return None,
    })
}

pub enum CreateEvent {
    /// Create an issue from this text. The form stays open, so a refused
    /// title keeps what the owner typed.
    Submit { text: String, description: String },
}

pub struct CreateForm {
    pub input: Entity<InputState>,
    pub description: Entity<TextareaState>,
    open: bool,
    /// Where the issue goes, such as `in llmtodo, project App`.
    place: SharedString,
    /// Why the last submit did not create the issue.
    notice: Option<SharedString>,
    return_to: Option<FocusHandle>,
    fallback: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CreateEvent> for CreateForm {}

impl CreateForm {
    pub fn new(fallback: FocusHandle, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Title, then status:todo label:bug priority:high")
        });
        let description = cx
            .new(|cx| TextareaState::new(window, cx).placeholder("Add a description in Markdown"));
        let title_events =
            cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.submit(window, cx),
                InputEvent::Change => this.clear_notice(cx),
                _ => {}
            });
        // Enter types a new line here, so only Cmd-Enter submits.
        let description_events =
            cx.subscribe_in(&description, window, |this, _, event, window, cx| {
                if let InputEvent::PressEnter {
                    secondary: true, ..
                } = event
                {
                    this.submit(window, cx);
                }
            });
        Self {
            input,
            description,
            open: false,
            place: "".into(),
            notice: None,
            return_to: None,
            fallback,
            _subscriptions: vec![title_events, description_events],
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, place: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.return_to = window.focused(cx);
        self.open = true;
        self.place = place;
        self.notice = None;
        self.description
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.input.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        let handle = self.return_to.take().unwrap_or(self.fallback.clone());
        window.focus(&handle, cx);
        cx.notify();
    }

    #[cfg(test)]
    pub fn notice(&self) -> Option<SharedString> {
        self.notice.clone()
    }

    /// Says why the issue was not created, with the typed text still there.
    pub fn show_notice(&mut self, notice: SharedString, cx: &mut Context<Self>) {
        self.notice = Some(notice);
        cx.notify();
    }

    fn clear_notice(&mut self, cx: &mut Context<Self>) {
        if self.notice.take().is_some() {
            cx.notify();
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if Draft::parse(&text).title.is_empty() {
            self.show_notice("Type a title for the new issue".into(), cx);
            self.input.update(cx, |state, cx| state.focus(window, cx));
            return;
        }
        let description = self.description.read(cx).value().trim().to_string();
        cx.emit(CreateEvent::Submit { text, description });
    }

    /// Tab and Shift-Tab move between the title and the description.
    fn switch_field(&mut self, _: &SwitchField, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.focus_handle(cx).contains_focused(window, cx) {
            self.description
                .update(cx, |state, cx| state.focus(window, cx));
        } else {
            self.input.update(cx, |state, cx| state.focus(window, cx));
        }
    }
}

impl Render for CreateForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let foreground = theme.popover_foreground;
        let caption = |text: &'static str| div().text_xs().text_color(muted).child(text);
        let (hint, hint_color): (SharedString, _) = match &self.notice {
            Some(notice) => (notice.clone(), foreground),
            None => (
                "Enter in the title or Cmd-Enter creates it. Tab moves between fields".into(),
                muted,
            ),
        };
        div()
            .absolute()
            .top_12()
            .bottom_12()
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .items_start()
            .child(
                v_flex()
                    .id("create-form")
                    .test_support()
                    .key_context(CONTEXT)
                    .on_action(cx.listener(Self::switch_field))
                    // The form covers the list. A click here stays here.
                    .occlude()
                    .w(rems(46.))
                    .max_w(relative(0.92))
                    .max_h_full()
                    .p_5()
                    .gap_4()
                    .bg(theme.popover)
                    .text_color(foreground)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius)
                    .shadow_lg()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .child(div().text_base().font_medium().child("New issue"))
                            .child(div().text_sm().text_color(muted).child(self.place.clone())),
                    )
                    .child(
                        v_flex()
                            .gap_1p5()
                            .child(caption("Title"))
                            .child(crate::shell::widgets::text_input(&self.input)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .gap_1p5()
                            .child(caption("Description"))
                            .child(
                                Textarea::new(&self.description)
                                    .h(rems(20.))
                                    .flex_shrink(1.)
                                    .min_h_0(),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .justify_between()
                            .child(div().text_xs().text_color(hint_color).child(hint))
                            .child(
                                h_flex()
                                    .flex_none()
                                    .gap_2()
                                    .child(
                                        Button::new("create-cancel")
                                            .small()
                                            .ghost()
                                            .label("Cancel")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.close(window, cx)
                                            })),
                                    )
                                    .child(
                                        Button::new("create-submit")
                                            .small()
                                            .primary()
                                            .label("Create")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.submit(window, cx)
                                            })),
                                    ),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_words_leave_the_title() {
        let d = Draft::parse(
            "Fix the crash status:in-review label:bug label:ux priority:high assignee:me now",
        );
        assert_eq!(d.title, "Fix the crash now");
        assert_eq!(d.status.as_deref(), Some("in-review"));
        assert_eq!(d.labels, ["bug", "ux"]);
        assert_eq!(d.priority.as_deref(), Some("high"));
        assert_eq!(d.assignee.as_deref(), Some("me"));
        // A colon in a plain word stays in the title.
        assert_eq!(Draft::parse("Note: ship it").title, "Note: ship it");
    }

    #[test]
    fn priority_words() {
        assert_eq!(priority("Urgent"), Some(Priority::Urgent));
        assert_eq!(priority("none"), Some(Priority::None));
        assert_eq!(priority("soon"), None);
    }
}
