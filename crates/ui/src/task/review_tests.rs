//! LLM-138: the review panel in the task view, its two buttons, and the
//! inbox list of tickets that wait for review.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};
use llmtodo_api_types::ReviewVerdict;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::model::backend::{Backend, Event, Request, Write, WriteKind};
use crate::model::data::{IssueId, Status, StatusType, WaitingOn};
use crate::shell::workspace::Workspace;

const SECTION: &str = "Context.\n\n+++ Review\n### Steps to reproduce\n1. Open the inbox.\n2. Hover a button.\n\n### What to check\n- The text turns black.\n+++\n";

fn open(
    cx: &mut TestAppContext,
) -> (
    AnyWindowHandle,
    Entity<Workspace>,
    UnboundedReceiver<Request>,
) {
    cx.update(crate::init);
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| Workspace::new(window, cx))
        })
        .unwrap()
    });
    let (backend, requests) = Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    cx.run_until_parked();
    (handle, view, requests)
}

/// Puts LLM-105 in User review with `description`, waiting on the owner,
/// and opens it.
fn in_review(
    handle: AnyWindowHandle,
    view: &Entity<Workspace>,
    description: &str,
    cx: &mut TestAppContext,
) -> IssueId {
    let id = cx.read(|cx| view.read(cx).store().read(cx).by_key("LLM-105").unwrap().id);
    let description = description.to_string();
    view.update(cx, |this, cx| {
        this.store().update(cx, |store, _| {
            store.modify(id, |i| {
                i.status = Status::new("User review", StatusType::Started, 9).with_review(true);
                i.description = description.into();
                i.waiting_on = Some(WaitingOn::Owner);
                i.claim = None;
            });
        });
    });
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.run_command("open llm-105", window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    id
}

fn message(view: &Entity<Workspace>, cx: &mut TestAppContext) -> String {
    cx.read(|cx| view.read(cx).message().to_string())
}

/// The review answers sent to the server.
fn sent(requests: &mut UnboundedReceiver<Request>) -> Vec<(String, ReviewVerdict, String)> {
    let mut out = Vec::new();
    while let Ok(r) = requests.try_recv() {
        if let Request::Write {
            key,
            write: Write::Review(r),
            kind: WriteKind::Review,
        } = r
        {
            out.push((key, r.verdict, r.feedback));
        }
    }
    out
}

#[gpui_kit::test]
fn the_panel_shows_steps_checks_and_both_buttons(cx: &mut TestAppContext) {
    let (handle, view, _) = open(cx);
    in_review(handle, &view, SECTION, cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.find("review-panel");
        window.find("review-feedback");
        window.find("review-approve");
        window.find("review-request-changes");
        assert!(window.try_find("review-no-section").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_ticket_without_a_section_can_still_be_answered(cx: &mut TestAppContext) {
    let (handle, view, _) = open(cx);
    in_review(handle, &view, "Only context.\n", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.find("review-no-section");
        window.find("review-approve");
    })
    .unwrap();
}

#[gpui_kit::test]
fn other_tickets_show_no_panel(cx: &mut TestAppContext) {
    let (handle, view, _) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.run_command("open llm-105", window, cx));
        window.render_frame(cx);
        assert!(window.try_find("review-panel").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn request_changes_needs_feedback_and_approve_sends(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    in_review(handle, &view, SECTION, cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("review-request-changes", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        message(&view, cx),
        "Write what to change on LLM-105 in the feedback field first"
    );
    assert!(sent(&mut requests).is_empty());

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("review-approve", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(message(&view, cx), "Approving LLM-105");
    assert_eq!(
        sent(&mut requests),
        [(
            "LLM-105".to_string(),
            ReviewVerdict::Approved,
            String::new()
        )]
    );
    // While it sends, a second click sends nothing.
    cx.update_window(handle, |_, window, cx| {
        window.click("review-approve", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(sent(&mut requests).is_empty());
}

#[gpui_kit::test]
fn a_refused_answer_shows_the_error_in_the_panel(cx: &mut TestAppContext) {
    let (handle, view, _) = open(cx);
    in_review(handle, &view, SECTION, cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("review-approve", cx);
    })
    .unwrap();
    cx.run_until_parked();
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::WriteFailed {
                key: "LLM-105".into(),
                kind: WriteKind::Review,
                message: "the server is down".into(),
                stale: false,
            },
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(
        message(&view, cx),
        "Could not send the review of LLM-105: the server is down"
    );
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.find("review-error");
        // The buttons work again.
        window.click("review-approve", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(message(&view, cx), "Approving LLM-105");
}

#[gpui_kit::test]
fn the_inbox_lists_tickets_waiting_for_review(cx: &mut TestAppContext) {
    let (handle, view, _) = open(cx);
    let id = in_review(handle, &view, SECTION, cx);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.run_command("inbox", window, cx));
        window.render_frame(cx);
        window.find("inbox-review-heading");
        window.click(format!("inbox-review-{id}"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let open = cx.read(|cx| view.read(cx).task().read(cx).detail().map(|d| d.issue));
    assert_eq!(open, Some(id));
}

/// LLM-147: Approve is filled with the text color of the page, so it is
/// white with dark text in the dark theme, and the inverse in the light
/// theme. Hover and press change the fill.
#[gpui_kit::test]
fn approve_is_filled_in_both_themes(cx: &mut TestAppContext) {
    use gpui_kit::component::{ActiveTheme as _, ThemeMode};
    cx.update(crate::init);
    cx.update(|cx| {
        for (mode, fill_is_light) in [(ThemeMode::Dark, true), (ThemeMode::Light, false)] {
            crate::shell::theme::set_mode(mode, cx);
            let theme = cx.theme();
            assert_eq!(theme.primary.l > 0.5, fill_is_light, "{mode:?}");
            assert_eq!(theme.primary_foreground.l > 0.5, !fill_is_light, "{mode:?}");
            assert_ne!(theme.primary_hover, theme.primary, "{mode:?}");
            assert_ne!(theme.primary_active, theme.primary_hover, "{mode:?}");
        }
    });
}

/// LLM-146: a drag on the review panel's bottom edge sets its height, the
/// settings keep it, and a double click on the edge goes back to the
/// content height.
#[gpui_kit::test]
fn a_drag_on_the_bottom_edge_resizes_the_review_panel(cx: &mut TestAppContext) {
    use crate::task::panel_resize::REVIEW;
    let (handle, view, _) = open(cx);
    in_review(handle, &view, SECTION, cx);
    let saved = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            view.read(cx)
                .current_settings(cx)
                .panel_heights
                .get(REVIEW)
                .copied()
        })
    };
    let handle_id = ("panel-resize-handle", 0usize);
    let before = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let panel = window.find("review-panel").bounds();
            let edge = window.find(handle_id).bounds();
            assert!(
                (edge.bottom() - panel.bottom()).abs() <= gpui_kit::px(1.),
                "the handle sits on the panel's bottom edge: {edge:?} {panel:?}"
            );
            let from = edge.center();
            let to = gpui_kit::point(from.x, from.y + gpui_kit::px(120.));
            window.drag(from, to, cx);
            window.render_frame(cx);
            panel
        })
        .unwrap();
    let after = cx
        .update_window(handle, |_, window, _| window.find("review-panel").bounds())
        .unwrap();
    assert!(
        after.size.height > before.size.height + gpui_kit::px(100.),
        "{before:?} -> {after:?}"
    );
    assert!(saved(cx).is_some(), "the settings keep the height");

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.double_click(handle_id, cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        saved(cx),
        None,
        "a double click goes back to the content height"
    );
    let reset = cx
        .update_window(handle, |_, window, _| window.find("review-panel").bounds())
        .unwrap();
    assert_eq!(reset.size.height, before.size.height);
}

/// The state of the Approve button in [`approve_draws_filled_in_every_state`].
#[derive(Clone, Copy, Debug, PartialEq)]
enum ButtonState {
    Rest,
    Hovered,
    Pressed,
    Focused,
    Disabled,
}

/// The real Approve button, with a probe that records its text color.
struct ProbedApprove {
    text: crate::inbox::tests::ProbeCell,
    disabled: bool,
    focus: gpui_kit::FocusHandle,
}

impl gpui_kit::Render for ProbedApprove {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::component::ActiveTheme as _;
        use gpui_kit::{ParentElement as _, Styled as _};
        gpui_kit::div()
            .size_full()
            .p_4()
            .bg(cx.theme().background)
            .child(
                crate::shell::widgets::filled_button("probed-approve", self.disabled, cx)
                    .track_focus(&self.focus)
                    .child(crate::inbox::tests::TextColorProbe(self.text.clone())),
            )
    }
}

/// The drawn fill and the text color of the probed button.
fn drawn(
    window: &mut gpui_kit::Window,
    text: &crate::inbox::tests::ProbeCell,
) -> (gpui_kit::Hsla, gpui_kit::Hsla) {
    let bounds = window
        .find("probed-approve")
        .bounds()
        .scale(window.scale_factor());
    let fill = window
        .painted_quads()
        .iter()
        .find(|q| q.bounds == bounds)
        .and_then(|q| q.background.as_solid())
        .expect("a filled quad at the button's bounds");
    (fill, text.get().expect("the probe was laid out"))
}

/// LLM-154: Approve is drawn filled in every state, in both themes. At rest
/// and with focus it shows the primary fill. Hover and press change the
/// fill. Disabled keeps the fill, only dimmer, and the text stays readable.
/// The checks read the drawn quad and the color the text is shaped with.
#[gpui_kit::test]
fn approve_draws_filled_in_every_state(cx: &mut TestAppContext) {
    use crate::inbox::tests::lightness_gap;
    use crate::shell::widgets::FILLED_DISABLED_OPACITY;
    use gpui_kit::component::{ActiveTheme as _, ThemeMode};
    use gpui_kit::{InputEvent as _, MouseButton, MouseDownEvent};
    cx.update(crate::init);
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        for state in [
            ButtonState::Rest,
            ButtonState::Hovered,
            ButtonState::Pressed,
            ButtonState::Focused,
            ButtonState::Disabled,
        ] {
            let text: crate::inbox::tests::ProbeCell = Default::default();
            let (handle, view, theme) = cx.update(|cx| {
                crate::shell::theme::set_mode(mode, cx);
                let theme = cx.theme().clone();
                let probe = text.clone();
                let (handle, view) =
                    gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                        cx.new(|cx| ProbedApprove {
                            text: probe,
                            disabled: state == ButtonState::Disabled,
                            focus: cx.focus_handle(),
                        })
                    })
                    .unwrap();
                (handle, view, theme)
            });
            let (fill, fg) = cx
                .update_window(handle, |_, window, cx| {
                    window.render_frame(cx);
                    match state {
                        ButtonState::Rest | ButtonState::Disabled => {}
                        ButtonState::Hovered => window.hover("probed-approve", cx),
                        ButtonState::Pressed => {
                            window.hover("probed-approve", cx);
                            let position = window.find("probed-approve").bounds().center();
                            let down = MouseDownEvent {
                                button: MouseButton::Left,
                                position,
                                modifiers: Default::default(),
                                click_count: 1,
                                first_mouse: false,
                            };
                            window.dispatch_event(down.to_platform_input(), cx);
                        }
                        ButtonState::Focused => {
                            let focus = view.read(cx).focus.clone();
                            window.focus(&focus, cx);
                        }
                    }
                    window.render_frame(cx);
                    drawn(window, &text)
                })
                .unwrap();
            let page = theme.background;
            let expected_fill = match state {
                ButtonState::Rest | ButtonState::Focused => theme.primary,
                ButtonState::Hovered => theme.primary_hover,
                ButtonState::Pressed => theme.primary_active,
                ButtonState::Disabled => theme.primary.opacity(FILLED_DISABLED_OPACITY),
            };
            assert_eq!(fill, expected_fill, "{mode:?} {state:?}: fill");
            assert_eq!(fg, theme.primary_foreground, "{mode:?} {state:?}: text");
            assert!(
                lightness_gap(page, fill, page) > 0.3,
                "{mode:?} {state:?}: the fill stands out from the page"
            );
            let text_on_fill = (page.blend(fill).blend(fg).l - page.blend(fill).l).abs();
            assert!(
                text_on_fill > 0.4,
                "{mode:?} {state:?}: the text is readable on the fill: {text_on_fill}"
            );
            if state != ButtonState::Disabled {
                assert!(
                    lightness_gap(page, fill, page) > 0.6,
                    "{mode:?} {state:?}: a full fill, not a tint"
                );
            }
        }
    }
}

/// LLM-154: in the review panel, with focus in the empty Feedback field,
/// Approve shows the primary fill and the primary text color.
#[gpui_kit::test]
fn approve_in_the_panel_is_filled_while_feedback_has_focus(cx: &mut TestAppContext) {
    use gpui_kit::component::{ActiveTheme as _, ThemeMode};
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        let (handle, view, _) = open(cx);
        cx.update(|cx| crate::shell::theme::set_mode(mode, cx));
        in_review(handle, &view, SECTION, cx);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("review-feedback", cx);
            window.render_frame(cx);
            let theme = cx.theme().clone();
            let bounds = window
                .find("review-approve")
                .bounds()
                .scale(window.scale_factor());
            let fill = window
                .painted_quads()
                .iter()
                .find(|q| q.bounds == bounds)
                .and_then(|q| q.background.as_solid())
                .expect("a filled quad at Approve's bounds");
            assert_eq!(fill, theme.primary, "{mode:?}");
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn the_task_menu_puts_approve_first_on_a_review_ticket(cx: &mut TestAppContext) {
    use crate::shell::workspace::Row;
    let (handle, view, mut requests) = open(cx);
    in_review(handle, &view, SECTION, cx);
    let row_of = |key: &str, cx: &mut TestAppContext| {
        cx.read(|cx| {
            let this = view.read(cx);
            this.rows(cx)
                .iter()
                .position(
                    |r| matches!(r, Row::Issue(ix) if this.issue(*ix, cx).key.as_ref() == key),
                )
                .unwrap()
        })
    };
    let row = row_of("LLM-105", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.right_click(("row", row), cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.within("popup-menu").click(0, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(message(&view, cx), "Approving LLM-105");
    assert_eq!(
        sent(&mut requests),
        [(
            "LLM-105".to_string(),
            ReviewVerdict::Approved,
            String::new()
        )]
    );

    // Another ticket has no Approve: its first item is the Status menu.
    let other = row_of("LLM-101", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.right_click(("row", other), cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window
            .within("popup-menu")
            .click(crate::shell::tests::TRASH_ITEM, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(sent(&mut requests).is_empty());
}
