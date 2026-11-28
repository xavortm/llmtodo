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
        match r {
            Request::Write {
                key,
                write: Write::Review(r),
                kind: WriteKind::Review,
            } => out.push((key, r.verdict, r.feedback)),
            Request::Reviews(answers) => {
                out.extend(answers.into_iter().map(|(k, r)| (k, r.verdict, r.feedback)));
            }
            _ => {}
        }
    }
    out
}

/// The keys of each batch of review answers sent as one request.
fn sent_batches(requests: &mut UnboundedReceiver<Request>) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    while let Ok(r) = requests.try_recv() {
        if let Request::Reviews(answers) = r {
            out.push(answers.into_iter().map(|(k, _)| k).collect());
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
                summary: None,
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

/// LLM-646: an edge that can be dragged shows a grip at rest, so it looks
/// different from a plain border. The grip stays inside the panel, and it
/// grows on hover. The task header's border has no grip.
#[gpui_kit::test]
fn a_draggable_edge_shows_a_grip_and_a_plain_border_does_not(cx: &mut TestAppContext) {
    use crate::shell::drag_handle::grip;
    use gpui_kit::base::ResizeHandleState;
    let (handle, view, _) = open(cx);
    in_review(handle, &view, SECTION, cx);
    let handle_id = ("panel-resize-handle", 0usize);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let panel = window.find("review-panel").bounds();
        let rest = window
            .within(("panel-resize", 0usize))
            .find("drag-handle-grip")
            .bounds();
        assert_eq!(rest.size.width, grip(ResizeHandleState::Idle).0);
        assert!(
            rest.bottom() <= panel.bottom() + gpui_kit::px(1.) && rest.top() >= panel.top(),
            "the grip stays inside the panel: {rest:?} {panel:?}"
        );
        assert!(
            window
                .within("detail-head")
                .try_find("drag-handle-grip")
                .is_none(),
            "the header's border is a plain line"
        );

        window.hover(handle_id, cx);
        window.render_frame(cx);
        let hovered = window
            .within(("panel-resize", 0usize))
            .find("drag-handle-grip")
            .bounds();
        assert_eq!(hovered.size.width, grip(ResizeHandleState::Hovered).0);
    })
    .unwrap();
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

/// LLM-166: puts each of `keys` in User review, waiting on the owner.
fn to_review(view: &Entity<Workspace>, keys: &[&str], cx: &mut TestAppContext) {
    view.update(cx, |this, cx| {
        for key in keys {
            let id = this.store().read(cx).by_key(key).unwrap().id;
            this.store().update(cx, |store, _| {
                store.modify(id, |i| {
                    i.status = Status::new("User review", StatusType::Started, 9).with_review(true);
                    i.waiting_on = Some(WaitingOn::Owner);
                    i.claim = None;
                });
            });
        }
    });
}

/// Marks each of `keys`, as a cmd-click does.
fn mark(handle: AnyWindowHandle, view: &Entity<Workspace>, keys: &[&str], cx: &mut TestAppContext) {
    for key in keys {
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |this, cx| {
                let id = this.issue_by_key(key, cx).id;
                this.click_row(id, crate::issues::list::RowClick::Toggle, window, cx);
            })
        })
        .unwrap();
    }
    cx.run_until_parked();
}

/// Picks Approve in the menu of `key`, as a right-click does.
fn approve_from_menu(
    handle: AnyWindowHandle,
    view: &Entity<Workspace>,
    key: &str,
    cx: &mut TestAppContext,
) {
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| {
            let issue = this.issue_by_key(key, cx).id;
            let event = crate::shell::view_events::ViewEvent::AnswerReview {
                issue,
                verdict: ReviewVerdict::Approved,
            };
            this.on_view_event(event, window, cx);
        })
    })
    .unwrap();
    cx.run_until_parked();
}

/// The server answers the review write of `key`: `None` takes it,
/// `Some(message)` refuses it.
fn answer(view: &Entity<Workspace>, key: &str, refusal: Option<&str>, cx: &mut TestAppContext) {
    let event = match refusal {
        None => Event::WriteDone {
            key: key.into(),
            kind: WriteKind::Review,
            version: 0,
        },
        Some(message) => Event::WriteFailed {
            key: key.into(),
            kind: WriteKind::Review,
            message: message.into(),
            stale: false,
            summary: None,
        },
    };
    view.update(cx, |this, cx| this.apply_event(event, cx));
    cx.run_until_parked();
}

fn sent_keys(requests: &mut UnboundedReceiver<Request>) -> Vec<String> {
    sent(requests).into_iter().map(|(key, ..)| key).collect()
}

fn sending(view: &Entity<Workspace>, key: &str, cx: &mut TestAppContext) -> bool {
    cx.read(|cx| {
        let this = view.read(cx);
        let id = this.issue_by_key(key, cx).id;
        this.task().read(cx).review_sending.contains_key(&id)
    })
}

#[gpui_kit::test]
fn approve_in_the_menu_sends_every_marked_review_task(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    in_review(handle, &view, SECTION, cx);
    to_review(&view, &["LLM-104", "LLM-106", "LLM-108"], cx);
    mark(handle, &view, &["LLM-104", "LLM-105", "LLM-106"], cx);
    // The right-click menu of a marked row sends Approve for it.
    let row = cx.read(|cx| {
        let this = view.read(cx);
        let id = this.issue_by_key("LLM-105", cx).id;
        this.rows(cx)
            .iter()
            .position(|r| matches!(r, crate::shell::workspace::Row::Issue(ix) if *ix == id))
            .unwrap()
    });
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
    // LLM-518: the batch goes out as one request.
    assert_eq!(
        sent_batches(&mut requests),
        [["LLM-104", "LLM-105", "LLM-106"]]
    );
    assert_eq!(message(&view, cx), "Approving 3 tasks");
    // The open task's panel shows its answer on the way.
    assert!(sending(&view, "LLM-105", cx));

    answer(&view, "LLM-104", None, cx);
    answer(&view, "LLM-105", None, cx);
    assert_eq!(message(&view, cx), "Approving 3 tasks");
    assert!(!sending(&view, "LLM-105", cx));
    answer(&view, "LLM-106", None, cx);
    assert_eq!(message(&view, cx), "Approved 3 tasks");
    // The next review ticket outside the batch opens.
    let open = cx.read(|cx| view.read(cx).task().read(cx).detail().map(|d| d.issue));
    let next = cx.read(|cx| view.read(cx).issue_by_key("LLM-108", cx).id);
    assert_eq!(open, Some(next));
}

#[gpui_kit::test]
fn a_batch_skips_tasks_that_cannot_be_approved(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    in_review(handle, &view, SECTION, cx);
    to_review(&view, &["LLM-104", "LLM-106", "LLM-107"], cx);
    view.update(cx, |this, cx| {
        let waits = this.store().read(cx).by_key("LLM-104").unwrap().id;
        let trashed = this.store().read(cx).by_key("LLM-107").unwrap().id;
        this.store().update(cx, |store, _| {
            store.modify(waits, |i| i.waiting_on = Some(WaitingOn::Agent));
            store.modify(trashed, |i| i.deleted = true);
        });
    });
    mark(
        handle,
        &view,
        &["LLM-101", "LLM-104", "LLM-105", "LLM-106", "LLM-107"],
        cx,
    );
    approve_from_menu(handle, &view, "LLM-105", cx);
    assert_eq!(sent_keys(&mut requests), ["LLM-105", "LLM-106"]);
    let skipped = "Skipped 3 tasks: 1 not in review, 1 already approved, 1 in the Trash";
    assert_eq!(message(&view, cx), format!("Approving 2 tasks. {skipped}"));
    answer(&view, "LLM-105", None, cx);
    answer(&view, "LLM-106", None, cx);
    assert_eq!(message(&view, cx), format!("Approved 2 tasks. {skipped}"));
}

#[gpui_kit::test]
fn a_failed_answer_in_a_batch_leaves_the_others(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    in_review(handle, &view, SECTION, cx);
    to_review(&view, &["LLM-104", "LLM-106"], cx);
    mark(handle, &view, &["LLM-104", "LLM-105", "LLM-106"], cx);
    approve_from_menu(handle, &view, "LLM-105", cx);
    assert_eq!(sent_keys(&mut requests), ["LLM-104", "LLM-105", "LLM-106"]);
    answer(&view, "LLM-104", None, cx);
    answer(&view, "LLM-105", Some("the server is down"), cx);
    answer(&view, "LLM-106", None, cx);
    assert_eq!(
        message(&view, cx),
        "Approved 2 tasks. Could not approve LLM-105: the server is down"
    );
    // LLM-105 is still in review, so it stays open and shows the error.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.find("review-error");
    })
    .unwrap();
}

#[gpui_kit::test]
fn approve_on_a_task_outside_the_marks_approves_it_alone(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    in_review(handle, &view, SECTION, cx);
    to_review(&view, &["LLM-104", "LLM-106"], cx);
    mark(handle, &view, &["LLM-104", "LLM-106"], cx);
    approve_from_menu(handle, &view, "LLM-105", cx);
    assert_eq!(sent_keys(&mut requests), ["LLM-105"]);
    assert_eq!(message(&view, cx), "Approving LLM-105");
    answer(&view, "LLM-105", None, cx);
    assert_eq!(
        message(&view, cx),
        "Approved LLM-105. An agent will merge its PR and close it"
    );

    // One mark is not a batch either.
    mark(handle, &view, &["LLM-104"], cx);
    approve_from_menu(handle, &view, "LLM-106", cx);
    assert_eq!(sent_keys(&mut requests), ["LLM-106"]);
    assert_eq!(message(&view, cx), "Approving LLM-106");
}

// LLM-168: Approve opens the next review ticket only when a task panel was
// open at the moment of the Approve. With no panel, only the selection
// moves.

fn selected(view: &Entity<Workspace>, cx: &mut TestAppContext) -> Option<IssueId> {
    cx.read(|cx| view.read(cx).list().read(cx).selected_issue())
}

fn open_task(view: &Entity<Workspace>, cx: &mut TestAppContext) -> Option<IssueId> {
    cx.read(|cx| view.read(cx).task().read(cx).detail().map(|d| d.issue))
}

fn id_of(view: &Entity<Workspace>, key: &str, cx: &mut TestAppContext) -> IssueId {
    cx.read(|cx| view.read(cx).issue_by_key(key, cx).id)
}

fn close_panel(view: &Entity<Workspace>, cx: &mut TestAppContext) {
    view.update(cx, |this, cx| {
        this.task().update(cx, |task, cx| task.close(cx))
    });
    cx.run_until_parked();
    assert_eq!(open_task(view, cx), None);
}

#[gpui_kit::test]
fn approve_in_the_panel_opens_the_next_review_ticket(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    in_review(handle, &view, SECTION, cx);
    to_review(&view, &["LLM-106"], cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("review-approve", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(sent_keys(&mut requests), ["LLM-105"]);
    answer(&view, "LLM-105", None, cx);
    let next = id_of(&view, "LLM-106", cx);
    assert_eq!(selected(&view, cx), Some(next));
    assert_eq!(open_task(&view, cx), Some(next));
}

#[gpui_kit::test]
fn approve_in_the_menu_with_a_panel_open_opens_the_next_one(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    in_review(handle, &view, SECTION, cx);
    to_review(&view, &["LLM-106"], cx);
    approve_from_menu(handle, &view, "LLM-105", cx);
    assert_eq!(sent_keys(&mut requests), ["LLM-105"]);
    answer(&view, "LLM-105", None, cx);
    let next = id_of(&view, "LLM-106", cx);
    assert_eq!(open_task(&view, cx), Some(next));
}

#[gpui_kit::test]
fn approve_with_no_panel_moves_only_the_selection(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    in_review(handle, &view, SECTION, cx);
    to_review(&view, &["LLM-106"], cx);
    close_panel(&view, cx);
    approve_from_menu(handle, &view, "LLM-105", cx);
    assert_eq!(sent_keys(&mut requests), ["LLM-105"]);
    answer(&view, "LLM-105", None, cx);
    assert_eq!(selected(&view, cx), Some(id_of(&view, "LLM-106", cx)));
    assert_eq!(open_task(&view, cx), None, "no task panel opens");
}

#[gpui_kit::test]
fn a_batch_with_no_panel_moves_only_the_selection(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    in_review(handle, &view, SECTION, cx);
    to_review(&view, &["LLM-104", "LLM-106", "LLM-108"], cx);
    mark(handle, &view, &["LLM-104", "LLM-105", "LLM-106"], cx);
    close_panel(&view, cx);
    approve_from_menu(handle, &view, "LLM-105", cx);
    assert_eq!(sent_keys(&mut requests), ["LLM-104", "LLM-105", "LLM-106"]);
    for key in ["LLM-104", "LLM-105", "LLM-106"] {
        answer(&view, key, None, cx);
    }
    assert_eq!(message(&view, cx), "Approved 3 tasks");
    assert_eq!(selected(&view, cx), Some(id_of(&view, "LLM-108", cx)));
    assert_eq!(open_task(&view, cx), None, "no task panel opens");
}

fn agy() -> llmtodo_api_types::Actor {
    llmtodo_api_types::Actor {
        id: llmtodo_core::ActorId::from_u128(7),
        kind: llmtodo_api_types::ActorKind::Agent,
        name: "antigravity/reviewer/high".into(),
        harness: Some("antigravity".into()),
        model: Some("reviewer".into()),
        effort: Some("high".into()),
        is_owner: false,
    }
}

fn the_owner() -> llmtodo_api_types::Actor {
    llmtodo_api_types::Actor {
        id: llmtodo_core::ActorId::from_u128(1),
        kind: llmtodo_api_types::ActorKind::Human,
        name: "owner".into(),
        harness: None,
        model: None,
        effort: None,
        is_owner: true,
    }
}

fn answer_by(
    round: i64,
    verdict: ReviewVerdict,
    feedback: &str,
    author: llmtodo_api_types::Actor,
) -> llmtodo_api_types::ReviewAnswer {
    llmtodo_api_types::ReviewAnswer {
        round,
        verdict,
        feedback: feedback.into(),
        author: Some(author),
        created_at: jiff::Timestamp::from_second(1_790_000_000 + round).unwrap(),
    }
}

/// Gives `id` the review answers of a fetch.
fn set_reviews(
    view: &Entity<Workspace>,
    id: IssueId,
    reviews: Vec<llmtodo_api_types::ReviewAnswer>,
    cx: &mut TestAppContext,
) {
    view.update(cx, |this, cx| {
        this.store().update(cx, |store, _| {
            store.set_detail(id, Vec::new(), reviews, Default::default(), Vec::new());
        });
    });
    cx.run_until_parked();
}

/// LLM-593: agy's notes from this round sit above the buttons, marked with
/// its name. Earlier rounds follow, oldest first.
#[gpui_kit::test]
fn agy_notes_sit_above_the_buttons(cx: &mut TestAppContext) {
    let (handle, view, _) = open(cx);
    let id = in_review(handle, &view, SECTION, cx);
    set_reviews(
        &view,
        id,
        vec![
            answer_by(1, ReviewVerdict::ChangesRequested, "1. blocking: x.", agy()),
            answer_by(
                2,
                ReviewVerdict::ChangesRequested,
                "Bigger text.",
                the_owner(),
            ),
            answer_by(
                3,
                ReviewVerdict::Passed,
                "1. nit: a name.\n2. should-fix: a test.",
                agy(),
            ),
        ],
        cx,
    );
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let notes = window.find("review-agent-notes");
        let approve = window.find("review-approve");
        assert!(notes.bounds().bottom() <= approve.bounds().top());
        window.find(("review-agent-note", 0usize));
        window.find(("review-agent-note", 1usize));
        assert!(window.try_find(("review-agent-note", 2usize)).is_none());
        // The pass shows once: above the buttons, not in the rounds.
        assert!(window.try_find(("review-round", 3usize)).is_none());
        let first = window.find(("review-round", 1usize));
        let second = window.find(("review-round", 2usize));
        assert!(approve.bounds().bottom() <= first.bounds().top());
        assert!(first.bounds().bottom() <= second.bounds().top());
    })
    .unwrap();
}

/// A pass with no notes says so, and there is nothing to number.
#[gpui_kit::test]
fn a_pass_without_notes_says_so(cx: &mut TestAppContext) {
    let (handle, view, _) = open(cx);
    let id = in_review(handle, &view, SECTION, cx);
    set_reviews(
        &view,
        id,
        vec![answer_by(1, ReviewVerdict::Passed, "", agy())],
        cx,
    );
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.find("review-agent-notes");
        assert!(window.try_find(("review-agent-note", 0usize)).is_none());
        assert!(window.try_find(("review-round", 1usize)).is_none());
    })
    .unwrap();
}

/// LLM-593: a ticket that agy reviews first, in In Review, is not in the
/// owner's inbox. A ticket agy passed on shows its badge there and on its
/// row in the list.
#[gpui_kit::test]
fn in_review_stays_out_of_the_inbox_and_rows_show_the_badge(cx: &mut TestAppContext) {
    use crate::model::data::LastReview;
    let (handle, view, _) = open(cx);
    let id = in_review(handle, &view, SECTION, cx);
    let other = cx.read(|cx| view.read(cx).store().read(cx).by_key("LLM-106").unwrap().id);
    view.update(cx, |this, cx| {
        this.store().update(cx, |store, _| {
            store.modify(id, |i| {
                i.last_review = Some(LastReview {
                    round: 1,
                    verdict: ReviewVerdict::Passed,
                    notes: 3,
                    agent: Some("agy".into()),
                });
            });
            store.modify(other, |i| {
                i.status = Status::new("In Review", StatusType::Started, 8)
                    .with_role(true, Some(llmtodo_api_types::ReviewRole::Agent));
                i.description = SECTION.into();
                i.waiting_on = Some(WaitingOn::Reviewer);
                i.claim = None;
            });
        });
    });
    cx.run_until_parked();
    let row_badge = crate::shell::widgets::issue_part("row-review", id);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(row_badge.clone()).is_some());
        view.update(cx, |this, cx| this.run_command("inbox", window, cx));
        window.render_frame(cx);
        window.find(format!("inbox-review-{id}"));
        window.find(crate::shell::widgets::issue_part("inbox-review-badge", id));
        assert!(window.try_find(format!("inbox-review-{other}")).is_none());
    })
    .unwrap();
}

/// LLM-645: an approved ticket that waits on an agent to merge its PR
/// says so on its row and in the task header, and sits in its own group
/// under "group by status", out of the User review group. A new answer
/// that asks for changes puts it back.
#[gpui_kit::test]
fn an_approved_ticket_shows_it_waits_for_its_merge(cx: &mut TestAppContext) {
    use crate::issues::list::{GroupBy, Row};
    use crate::issues::rows::APPROVED_GROUP;
    use crate::model::data::LastReview;
    let (handle, view, _) = open(cx);
    let id = in_review(handle, &view, SECTION, cx);
    // Grouped once, before the answer: the regroup must come from the
    // update itself, as when the server answers an Approve.
    view.update(cx, |this, cx| {
        this.list()
            .update(cx, |list, cx| list.group_by(GroupBy::Status, cx));
    });
    cx.run_until_parked();
    let set = |verdict: ReviewVerdict, waiting: WaitingOn, cx: &mut TestAppContext| {
        view.update(cx, |this, cx| {
            let mut issue = this.store().read(cx).get(id).unwrap().clone();
            issue.last_review = Some(LastReview {
                round: 2,
                verdict,
                notes: 0,
                agent: None,
            });
            issue.waiting_on = Some(waiting);
            this.upsert_issue(issue, cx);
        });
        cx.run_until_parked();
    };
    // The group title each row sits under.
    let group_of = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let mut title = None;
            for row in view.read(cx).list().read(cx).rows() {
                match row {
                    Row::Header { title: t, .. } => title = Some(t.to_string()),
                    Row::Issue(i) if *i == id => return title,
                    _ => {}
                }
            }
            None
        })
    };
    let row_badge = crate::shell::widgets::issue_part("row-review", id);

    set(ReviewVerdict::Approved, WaitingOn::Agent, cx);
    assert!(cx.read(|cx| {
        view.read(cx)
            .store()
            .read(cx)
            .get(id)
            .unwrap()
            .approved_waiting()
    }));
    assert_eq!(group_of(cx).as_deref(), Some(APPROVED_GROUP));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(row_badge.clone()).is_some(),
            "the row says Approved"
        );
        window.find("detail-approved");
    })
    .unwrap();

    set(ReviewVerdict::ChangesRequested, WaitingOn::Agent, cx);
    assert_eq!(group_of(cx).as_deref(), Some("User review"));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("detail-approved").is_none());
    })
    .unwrap();
}

/// LLM-645: the owner may approve a ticket that a reviewer agent still
/// has, in a status such as In Review. It shows the Approved badge there
/// too, and it is no drop target.
#[test]
fn an_approval_during_an_agent_review_shows_the_badge() {
    use crate::model::data::LastReview;
    let mut issue = crate::model::demo::issues(&crate::model::demo::catalog())
        .into_iter()
        .next()
        .expect("demo issues");
    issue.status = Status::new("In Review", StatusType::Started, 8)
        .with_role(true, Some(llmtodo_api_types::ReviewRole::Agent));
    issue.waiting_on = Some(WaitingOn::Agent);
    issue.last_review = Some(LastReview {
        round: 2,
        verdict: ReviewVerdict::Approved,
        notes: 0,
        agent: None,
    });
    assert!(issue.approved_waiting());
    assert!(issue.review_badge().is_some_and(|b| b.approved));
    // Waiting on the owner again, it is the owner's turn: no badge.
    issue.waiting_on = Some(WaitingOn::Owner);
    assert!(!issue.approved_waiting());
    assert!(issue.review_badge().is_none());
}
