//! The claims list: which agent holds which task, and for how long. It opens over the window with `:claims`, `g c` or a click on
//! the claim count in the status bar. `j` / `k` and `Ctrl-N` / `Ctrl-P`
//! move, Enter opens the task, and Esc closes the list.
//!
//! Like the palette, it holds only view state. It emits `ClaimsEvent`, and
//! the workspace opens the task.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, Context, Entity, EventEmitter, FocusHandle, InteractiveElement as _, IntoElement,
    KeyBinding, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, actions, div, prelude::FluentBuilder as _,
};

use crate::model::data::{Claim, IssueId};
use crate::model::store::IssueStore;
use crate::shell::icons::HasIcon as _;

actions!(claims, [ClaimsDown, ClaimsUp, ClaimsOpen, ClaimsClose]);

pub const CONTEXT: &str = "Claims";

pub fn bind_keys(cx: &mut App) {
    let claims = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("j", ClaimsDown, claims),
        KeyBinding::new("down", ClaimsDown, claims),
        KeyBinding::new("ctrl-n", ClaimsDown, claims),
        KeyBinding::new("k", ClaimsUp, claims),
        KeyBinding::new("up", ClaimsUp, claims),
        KeyBinding::new("ctrl-p", ClaimsUp, claims),
        KeyBinding::new("enter", ClaimsOpen, claims),
        KeyBinding::new("escape", ClaimsClose, claims),
        KeyBinding::new("ctrl-c", ClaimsClose, claims),
        KeyBinding::new("q", ClaimsClose, claims),
    ]);
}

pub enum ClaimsEvent {
    /// Open this task. The list is already closed.
    Open(IssueId),
}

/// One claimed task, as the list shows it.
#[derive(Debug, Clone)]
pub struct ClaimRow {
    pub id: IssueId,
    pub key: SharedString,
    pub title: SharedString,
    pub claim: Claim,
}

/// Every claimed task that is not in the Trash, the longest held first.
pub fn claim_rows(store: &IssueStore) -> Vec<ClaimRow> {
    let mut rows: Vec<ClaimRow> = store
        .iter()
        .filter(|i| !i.deleted)
        .filter_map(|i| {
            i.claim.clone().map(|claim| ClaimRow {
                id: i.id,
                key: i.key.clone(),
                title: i.title.clone(),
                claim,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        a.claim
            .claimed_at
            .cmp(&b.claim.claimed_at)
            .then_with(|| a.key.cmp(&b.key))
    });
    rows
}

pub struct ClaimsList {
    store: Entity<IssueStore>,
    focus: FocusHandle,
    open: bool,
    selected: usize,
    /// Focus to give back when it closes.
    return_to: Option<FocusHandle>,
    /// Focus when there is nothing to give back: the list.
    fallback: FocusHandle,
}

impl EventEmitter<ClaimsEvent> for ClaimsList {}

impl ClaimsList {
    pub fn new(store: Entity<IssueStore>, fallback: FocusHandle, cx: &mut Context<Self>) -> Self {
        Self {
            store,
            focus: cx.focus_handle(),
            open: false,
            selected: 0,
            return_to: None,
            fallback,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    #[cfg(test)]
    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn rows(&self, cx: &App) -> Vec<ClaimRow> {
        claim_rows(self.store.read(cx))
    }

    /// Opens with the first row selected.
    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            self.return_to = window.focused(cx);
        }
        self.open = true;
        self.selected = 0;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        let handle = self.return_to.take().unwrap_or(self.fallback.clone());
        window.focus(&handle, cx);
        cx.notify();
    }

    fn step(&mut self, down: bool, cx: &mut Context<Self>) {
        let count = self.rows(cx).len();
        self.selected = if down {
            (self.selected + 1).min(count.saturating_sub(1))
        } else {
            self.selected.saturating_sub(1)
        };
        cx.notify();
    }

    fn open_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows(cx).into_iter().nth(ix) else {
            return;
        };
        self.close(window, cx);
        cx.emit(ClaimsEvent::Open(row.id));
    }
}

impl Render for ClaimsList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        // Claims come and go live, so the selection stays on a real row.
        self.selected = self.selected.min(rows.len().saturating_sub(1));
        let now = jiff::Timestamp::now();
        let theme = cx.theme();
        let hover = theme.list_hover;
        let pressed = theme.foreground.opacity(0.12);
        let active_bg = theme.list_active;
        let muted = theme.muted_foreground;
        let mono = theme.mono_font_family.clone();
        let title = match rows.len() {
            0 => "Claims".to_string(),
            1 => "Claims · 1 task".to_string(),
            n => format!("Claims · {n} tasks"),
        };
        let body = if rows.is_empty() {
            div()
                .px_2()
                .py_3()
                .text_sm()
                .text_color(muted)
                .child("No agent holds a task.")
                .into_any_element()
        } else {
            v_flex()
                .gap_px()
                .children(rows.into_iter().enumerate().map(|(ix, row)| {
                    let selected = ix == self.selected;
                    let age = row.claim.age(now);
                    h_flex()
                        .id(SharedString::from(format!("claim-{}", row.key)))
                        .test_support()
                        .px_2()
                        .py_1()
                        .gap_2()
                        .text_sm()
                        .rounded(theme.radius)
                        .cursor_pointer()
                        .when(selected, |this| this.bg(active_bg))
                        .when(!selected, |this| this.hover(move |s| s.bg(hover)))
                        .active(move |s| s.bg(pressed))
                        .child(crate::shell::tooltip::with_card(
                            SharedString::from(format!("claim-agent-{}", row.key)),
                            Icon::new(row.claim.actor.icon()).xsmall(),
                            crate::shell::tooltip::Card::text(format!(
                                "Claimed by {}",
                                row.claim.actor.label()
                            )),
                        ))
                        .child(
                            div()
                                .flex_none()
                                .font_family(mono.clone())
                                .child(row.key.clone()),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(row.title))
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(muted)
                                .child(row.claim.actor.model()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_family(mono.clone())
                                .text_xs()
                                .text_color(muted)
                                .child(age),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_row(ix, window, cx);
                        }))
                }))
                .into_any_element()
        };
        div()
            .absolute()
            .top_10()
            .left_4()
            .right_4()
            .flex()
            .justify_center()
            .child(
                v_flex()
                    .id("claims")
                    .key_context(CONTEXT)
                    .track_focus(&self.focus)
                    .on_action(cx.listener(|this, _: &ClaimsDown, _, cx| this.step(true, cx)))
                    .on_action(cx.listener(|this, _: &ClaimsUp, _, cx| this.step(false, cx)))
                    .on_action(cx.listener(|this, _: &ClaimsOpen, window, cx| {
                        let ix = this.selected;
                        this.open_row(ix, window, cx);
                    }))
                    .on_action(
                        cx.listener(|this, _: &ClaimsClose, window, cx| this.close(window, cx)),
                    )
                    .w(gpui_kit::px(560.))
                    .max_w_full()
                    .p_1()
                    .gap_1()
                    .bg(theme.popover)
                    .text_color(theme.popover_foreground)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius)
                    .shadow_lg()
                    .child(
                        h_flex()
                            .px_2()
                            .pt_1()
                            .justify_between()
                            .text_xs()
                            .text_color(muted)
                            .child(title)
                            .child("Enter opens · Esc closes"),
                    )
                    .child(body),
            )
    }
}
