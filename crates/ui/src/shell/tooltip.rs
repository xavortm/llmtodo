//! Card tooltips (LLM-81). Every tooltip in the app is a card: an optional
//! small heading, wrapped body text and label / value rows. The card wraps
//! at [`MAX_WIDTH`], and GPUI Kit's tooltip keeps it inside the window.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AnyView, App, ElementId, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};

use crate::model::data::Claim;

/// The widest a card grows before its text wraps, in rems.
pub const MAX_WIDTH: f32 = 20.;

/// The content of one card tooltip.
/// The most blockers one tooltip lists.
const BLOCKERS_SHOWN: usize = 8;
/// The most sessions one cost card lists.
const COST_SESSIONS_SHOWN: usize = 8;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Card {
    pub heading: Option<SharedString>,
    pub body: Option<SharedString>,
    pub rows: Vec<(SharedString, SharedString)>,
}

impl Card {
    /// A card with body text only, such as "Copy ID".
    pub fn text(body: impl Into<SharedString>) -> Self {
        Self {
            body: Some(body.into()),
            ..Self::default()
        }
    }

    /// A card that starts with a small heading.
    pub fn titled(heading: impl Into<SharedString>) -> Self {
        Self {
            heading: Some(heading.into()),
            ..Self::default()
        }
    }

    pub fn body(mut self, body: impl Into<SharedString>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Adds a `label  value` row. A missing value adds nothing.
    pub fn row(
        mut self,
        label: impl Into<SharedString>,
        value: Option<impl Into<SharedString>>,
    ) -> Self {
        if let Some(value) = value {
            self.rows.push((label.into(), value.into()));
        }
        self
    }

    /// The card for a claimed task: who holds it, with which model, where
    /// the work happens and when it started.
    pub fn claim(claim: &Claim, now: jiff::Timestamp) -> Self {
        let agent = match &claim.actor {
            crate::model::data::Assignee::Agent(agent) => Some(agent),
            crate::model::data::Assignee::Person(_) => None,
        };
        let started = claim
            .claimed_at
            .to_zoned(jiff::tz::TimeZone::system())
            .strftime("%b %-d, %H:%M")
            .to_string();
        Self::titled(format!("Locked by {}", claim.actor.label()))
            .body("The task is read-only while the claim is held.")
            .row("Agent", agent.map(|a| a.name.clone()))
            .row("Harness", agent.and_then(|a| a.harness.clone()))
            .row("Model", agent.and_then(|a| a.model.clone()))
            .row("Effort", agent.and_then(|a| a.effort.clone()))
            .row("Branch", claim.branch.clone())
            .row(
                "Worktree",
                claim
                    .worktree
                    .as_deref()
                    .map(crate::model::data::shorten_home),
            )
            .row("Pane", claim.pane.clone())
            .row(
                "Started",
                Some(format!("{started}, {} ago", claim.age(now))),
            )
    }

    pub fn analyzing(mark: &crate::model::data::Analyzing, now: jiff::Timestamp) -> Self {
        let agent = match &mark.actor {
            crate::model::data::Assignee::Agent(agent) => Some(agent),
            crate::model::data::Assignee::Person(_) => None,
        };
        let local = |t: jiff::Timestamp| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%b %-d, %H:%M")
                .to_string()
        };
        Self::titled(format!("Analyzed by {}", mark.actor.label()))
            .body("The agent reads or fills in the task. You can still edit it.")
            .row("Agent", agent.map(|a| a.name.clone()))
            .row("Model", agent.and_then(|a| a.model.clone()))
            .row(
                "Started",
                Some(format!("{}, {} ago", local(mark.started_at), mark.age(now))),
            )
            .row("Ends", Some(local(mark.expires_at)))
    }

    /// The card on a task's cost: the total and, once the server sent the
    /// sessions, what each one cost. A closed task's cost is final.
    pub fn cost(
        total: f64,
        closed: bool,
        breakdown: Option<&crate::task::history::CostBreakdown>,
    ) -> Self {
        let total = crate::model::data::format_cost(total);
        let mut card = Self::titled(if closed {
            format!("Final cost: {total}")
        } else {
            format!("Cost so far: {total}")
        });
        let Some(breakdown) = breakdown else {
            return card;
        };
        if !breakdown.unpriced.is_empty() {
            card = card.body(format!(
                "Leaves out models with no price: {}.",
                breakdown.unpriced.join(", ")
            ));
        }
        for s in breakdown.sessions.iter().take(COST_SESSIONS_SHOWN) {
            card = card.row(
                crate::model::data::format_cost(s.cost),
                Some(s.detail.clone()),
            );
        }
        if let Some(more) = breakdown
            .sessions
            .len()
            .checked_sub(COST_SESSIONS_SHOWN)
            .filter(|n| *n > 0)
        {
            card = card.row("", Some(format!("and {more} more")));
        }
        card
    }

    /// The card's element. Tests find it by the `tooltip-card` ID.
    /// The card on a task that waits for open work: the tasks it waits for,
    /// at most [`BLOCKERS_SHOWN`] of them.
    pub fn blockers(blockers: &[crate::model::data::Blocker], depth: u32) -> Self {
        let heading = match blockers.len() {
            1 => "Waits for 1 open task".to_string(),
            n => format!("Waits for {n} open tasks"),
        };
        let mut card = Self::titled(heading);
        if depth > 1 {
            card = card.body(format!("The longest chain is {depth} tasks long."));
        }
        for b in blockers.iter().take(BLOCKERS_SHOWN) {
            card = card.row(b.key.clone(), Some(b.title.clone()));
        }
        if let Some(more) = blockers
            .len()
            .checked_sub(BLOCKERS_SHOWN)
            .filter(|n| *n > 0)
        {
            card = card.row("", Some(format!("and {more} more")));
        }
        card
    }

    pub fn render(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let multi = self.heading.is_some() || !self.rows.is_empty();
        v_flex()
            .id("tooltip-card")
            .test_support()
            .max_w(gpui_kit::rems(MAX_WIDTH))
            .gap_1()
            .when(multi, |d| d.py_1())
            .when_some(self.heading.clone(), |d, heading| {
                d.child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.popover_foreground)
                        .child(heading),
                )
            })
            .when_some(self.body.clone(), |d, body| {
                d.child(div().w_full().whitespace_normal().child(body))
            })
            .when(!self.rows.is_empty(), |d| {
                d.child(v_flex().gap_0p5().text_xs().children(self.rows.iter().map(
                    |(label, value)| {
                        h_flex()
                            .items_start()
                            .gap_2()
                            .child(
                                div()
                                    .w_16()
                                    .flex_none()
                                    .text_color(muted)
                                    .child(label.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .whitespace_normal()
                                    .child(value.clone()),
                            )
                    },
                )))
            })
            .into_any_element()
    }

    /// Builds the tooltip view, for `.tooltip(...)` on any element.
    pub fn build(&self, window: &mut Window, cx: &mut App) -> AnyView {
        let card = self.clone();
        Tooltip::element(move |_, cx| card.render(cx)).build(window, cx)
    }

    /// The builder closure that `.tooltip(...)` takes.
    pub fn builder(self) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        move |window, cx| self.build(window, cx)
    }
}

/// Wraps an element, such as a GPUI Kit `Button`, that only takes a plain
/// text tooltip, so it shows a card instead.
pub fn with_card(id: impl Into<ElementId>, child: impl IntoElement, card: Card) -> AnyElement {
    div()
        .id(id)
        .test_support()
        .flex_none()
        .child(child)
        .tooltip(card.builder())
        .into_any_element()
}
