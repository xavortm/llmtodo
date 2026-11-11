//! The review panel at the top of the task view.
//!
//! A ticket in a review status shows its `+++ Review` section: the steps as a
//! numbered list and the checks as a list. Under them sit a feedback field
//! and the Approve and Request changes buttons. Earlier rounds show below.
//! A ticket that left its review status shows only its rounds, so an answer
//! changes the panel at once.
//!
//! The panel sends nothing itself. A button emits `AnswerReview`, and the
//! workspace sends it with the feedback field's text.

use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, input::Textarea, v_flex,
};
use gpui_kit::{
    AnyElement, App, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _,
};
use llmtodo_api_types::{ReviewAnswer, ReviewVerdict};

use crate::model::data::{Issue, WaitingOn};
use crate::shell::tooltip::{Card, with_card};
use crate::shell::view_events::{ViewEvent, ViewEvents};
use crate::shell::widgets::{filled_button, inbox_button};

/// What the panel needs besides the issue.
pub struct PanelState<'a> {
    pub answers: &'a [ReviewAnswer],
    pub feedback: Option<Entity<gpui_kit::component::input::TextareaState>>,
    /// The verdict on its way to the server, for this issue.
    pub sending: Option<ReviewVerdict>,
    pub error: Option<SharedString>,
    /// The owner dragged the panel to a height, so it fills its wrapper
    /// instead of following its content.
    pub fill: bool,
}

/// The tallest the panel gets, in rem. Past it, the panel scrolls.
const PANEL_MAX_REM: f32 = 22.;

pub fn verdict_text(v: ReviewVerdict) -> &'static str {
    match v {
        ReviewVerdict::Approved => "Approved",
        ReviewVerdict::ChangesRequested => "Changes requested",
    }
}

/// The panel, or None when the ticket is not in a review status and has no
/// rounds.
pub fn panel(
    issue: &Issue,
    state: PanelState<'_>,
    events: ViewEvents,
    cx: &App,
) -> Option<AnyElement> {
    let in_review = issue.status.review;
    if !in_review && state.answers.is_empty() {
        return None;
    }
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let mono = theme.mono_font_family.clone();
    let heading = |text: &'static str| {
        div()
            .text_xs()
            .text_color(muted)
            .font_weight(gpui_kit::FontWeight::MEDIUM)
            .child(text)
    };
    let mut body: Vec<AnyElement> = Vec::new();
    // An approval with an open PR waits on an agent to merge it.
    let approved_waiting = in_review
        && issue.waiting_on == Some(WaitingOn::Agent)
        && state
            .answers
            .last()
            .is_some_and(|a| a.verdict == ReviewVerdict::Approved);
    if in_review {
        match llmtodo_core::review(&issue.description) {
            Some(r) if !r.steps.is_empty() || !r.checks.is_empty() => {
                body.push(heading("Steps to reproduce").into_any_element());
                for (ix, step) in r.steps.iter().enumerate() {
                    body.push(
                        h_flex()
                            .id(("review-step", ix))
                            .test_support()
                            .items_start()
                            .gap_2()
                            .child(
                                div()
                                    .flex_none()
                                    .w_5()
                                    .font_family(mono.clone())
                                    .text_color(muted)
                                    .child(format!("{}.", ix + 1)),
                            )
                            .child(div().flex_1().min_w_0().child(step.clone()))
                            .into_any_element(),
                    );
                }
                body.push(heading("What to check").into_any_element());
                for (ix, check) in r.checks.iter().enumerate() {
                    body.push(
                        h_flex()
                            .id(("review-check", ix))
                            .test_support()
                            .items_start()
                            .gap_2()
                            .child(
                                div()
                                    .flex_none()
                                    .w_5()
                                    .pt_0p5()
                                    .child(Icon::new(IconName::Check).xsmall().text_color(muted)),
                            )
                            .child(div().flex_1().min_w_0().child(check.clone()))
                            .into_any_element(),
                    );
                }
            }
            _ => body.push(
                div()
                    .id("review-no-section")
                    .test_support()
                    .text_color(muted)
                    .child("This ticket has no review steps. You can still answer.")
                    .into_any_element(),
            ),
        }
        if approved_waiting {
            body.push(
                div()
                    .id("review-waiting-merge")
                    .test_support()
                    .text_color(muted)
                    .child("You approved it. An agent will merge its PR and close it.")
                    .into_any_element(),
            );
        } else {
            body.push(controls(issue, &state, events, cx));
        }
    }
    if let Some(error) = &state.error {
        body.push(
            div()
                .id("review-error")
                .test_support()
                .text_color(theme.danger)
                .child(error.clone())
                .into_any_element(),
        );
    }
    let rounds = rounds(state.answers, cx);
    let title = if in_review { "Review" } else { "Review rounds" };
    let round_now = state.answers.len() + 1;
    Some(
        v_flex()
            .id("review-panel")
            .test_support()
            .flex_none()
            .w_full()
            .min_w_0()
            .map(|this| {
                if state.fill {
                    this.size_full()
                } else {
                    this.max_h(gpui_kit::rems(PANEL_MAX_REM))
                }
            })
            .overflow_y_scroll()
            .px_3()
            .py_2()
            .gap_1p5()
            .text_sm()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Icon::new(IconName::ClipboardCheck)
                            .xsmall()
                            .text_color(muted),
                    )
                    .child(div().font_weight(gpui_kit::FontWeight::MEDIUM).child(title))
                    .when(in_review && !approved_waiting, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(format!("Round {round_now}")),
                        )
                    }),
            )
            .children(body)
            .children(rounds)
            .into_any_element(),
    )
}

/// The feedback field and the two buttons.
fn controls(issue: &Issue, state: &PanelState<'_>, events: ViewEvents, cx: &App) -> AnyElement {
    let id = issue.id;
    let sending = state.sending;
    let button = |verdict: ReviewVerdict, label: &'static str, tip: &'static str| {
        let events = events.clone();
        let busy = sending == Some(verdict);
        let element = match verdict {
            ReviewVerdict::Approved => "review-approve",
            ReviewVerdict::ChangesRequested => "review-request-changes",
        };
        with_card(
            SharedString::from(format!("{element}-tip")),
            match verdict {
                ReviewVerdict::Approved => filled_button(element, sending.is_some(), cx),
                ReviewVerdict::ChangesRequested => {
                    inbox_button(element, false, cx).disabled(sending.is_some())
                }
            }
            .label(if busy { "Sending…" } else { label })
            .on_click(move |_, window, cx| {
                events.emit(ViewEvent::AnswerReview { issue: id, verdict }, window, cx);
            }),
            Card::text(tip),
        )
    };
    v_flex()
        .gap_1p5()
        .pt_1()
        .children(state.feedback.as_ref().map(|input| {
            div()
                .id("review-feedback")
                .test_support()
                .w_full()
                .child(Textarea::new(input).small())
        }))
        .child(
            h_flex()
                .gap_2()
                .child(button(
                    ReviewVerdict::Approved,
                    "Approve",
                    "Close the ticket. With an open PR, an agent merges it first",
                ))
                .child(button(
                    ReviewVerdict::ChangesRequested,
                    "Request changes",
                    "Send the feedback to an agent. The ticket goes back to Todo",
                )),
        )
        .into_any_element()
}

/// The earlier rounds, newest first.
fn rounds(answers: &[ReviewAnswer], cx: &App) -> Vec<AnyElement> {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    answers
        .iter()
        .rev()
        .map(|a| {
            let who = a.author.as_ref().map_or_else(
                || "someone".to_string(),
                |actor| {
                    if actor.is_owner {
                        "you".to_string()
                    } else {
                        actor.name.clone()
                    }
                },
            );
            let when = a
                .created_at
                .to_zoned(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d %H:%M")
                .to_string();
            v_flex()
                .id(("review-round", a.round as usize))
                .test_support()
                .w_full()
                .gap_0p5()
                .pt_1()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .child(format!("Round {}: {}", a.round, verdict_text(a.verdict))),
                        )
                        .child(div().text_color(muted).child(format!("by {who}, {when}"))),
                )
                .when(!a.feedback.is_empty(), |d| {
                    d.child(div().text_xs().child(a.feedback.clone()))
                })
                .into_any_element()
        })
        .collect()
}
