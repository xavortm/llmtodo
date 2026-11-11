//! UI integration tests for the owner inbox and the Owner questions tab.
//!
//! In the demo data LLM-103 asks a choice question and LLM-111 a free-text
//! one. Both wait on the owner, and an agent holds LLM-103.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::model::backend::{Backend, Request, Write, WriteKind};
use crate::model::data::WaitingOn;
use crate::model::scope::Scope;
use crate::shell::workspace::Workspace;

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
    (handle, view, requests)
}

/// The answers sent to the server, as `(key, question ID, answer)`.
fn answers(requests: &mut UnboundedReceiver<Request>) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    while let Ok(r) = requests.try_recv() {
        if let Request::Write {
            key,
            write: Write::Update(update),
            kind: WriteKind::Answer,
        } = r
        {
            for p in &update.description.patches {
                if let llmtodo_core::Patch::AnswerQuestion { id, answer } = p {
                    out.push((key.clone(), id.clone(), answer.clone()));
                }
            }
        }
    }
    out
}

fn inbox_keys(view: &Entity<Workspace>, cx: &TestAppContext) -> Vec<String> {
    cx.read(|cx| {
        let store = view.read(cx).store().read(cx);
        crate::inbox::entries(store)
            .iter()
            .map(|e| format!("{} {}", e.key, e.question.id))
            .collect()
    })
}

fn selected_key(view: &Entity<Workspace>, cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| {
        let inbox = view.read(cx).inbox().clone();
        let picked = inbox.update(cx, |inbox, cx| inbox.selected(cx));
        picked.map(|(id, n)| format!("{} Q{n}", view.read(cx).issue(id, cx).key))
    })
}

#[gpui_kit::test]
fn g_i_lists_open_questions_and_a_number_picks_an_option(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    // The oldest issue comes first.
    assert_eq!(inbox_keys(&view, cx), ["LLM-111 Q1", "LLM-103 Q1"]);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("g", cx);
        window.press("i", cx);
        window.render_frame(cx);
        assert_eq!(view.read(cx).scope(cx), Scope::Inbox);
        assert!(view.read(cx).inbox_shown(cx));
        assert!(view.read(cx).inbox().read(cx).focused(window, cx));
    })
    .unwrap();
    // The first question is selected, and Ctrl-N moves down.
    assert_eq!(selected_key(&view, cx).as_deref(), Some("LLM-111 Q1"));
    cx.update_window(handle, |_, window, cx| {
        // A free-text question takes no number.
        window.press("1", cx);
        assert!(view.read(cx).message().contains("takes text"));
        window.press("ctrl-n", cx);
    })
    .unwrap();
    assert_eq!(selected_key(&view, cx).as_deref(), Some("LLM-103 Q1"));

    // `2` picks b, also on a task an agent holds.
    cx.update_window(handle, |_, window, cx| {
        window.press("2", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        answers(&mut requests),
        [("LLM-103".to_string(), "Q1".to_string(), "b".to_string())]
    );
    cx.read(|cx| {
        let this = view.read(cx);
        let issue = this.issue_by_key("LLM-103", cx);
        let q = &llmtodo_core::questions(&issue.description)[0];
        assert_eq!(q.answer.as_deref(), Some("b) :assign and :move"));
        // The last open question got its answer, so the task goes back to
        // the agents.
        assert_eq!(issue.waiting_on, Some(WaitingOn::Agent));
        assert_eq!(
            this.message(),
            "Answered LLM-103 Q1: b) :assign and :move. It goes back to the agents"
        );
    });
    // The answered question leaves; the selection moves to the one left.
    assert_eq!(inbox_keys(&view, cx), ["LLM-111 Q1"]);
    assert_eq!(selected_key(&view, cx).as_deref(), Some("LLM-111 Q1"));
}

#[gpui_kit::test]
fn enter_types_a_free_text_answer(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |this, cx| this.run_command("inbox", window, cx));
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
        // The answer field takes the keys, so j and 1 are text.
        assert_eq!(view.read(cx).vim_mode(window, cx), "insert");
        window.input("Mine 1 j", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // Focus goes back to the inbox.
        assert_eq!(view.read(cx).vim_mode(window, cx), "normal");
        assert!(view.read(cx).inbox().read(cx).focused(window, cx));
    })
    .unwrap();
    assert_eq!(
        answers(&mut requests),
        [(
            "LLM-111".to_string(),
            "Q1".to_string(),
            "Mine 1 j".to_string()
        )]
    );
    assert!(inbox_keys(&view, cx).iter().all(|k| k != "LLM-111 Q1"));
    cx.read(|cx| {
        let issue = view.read(cx).issue_by_key("LLM-111", cx);
        assert_eq!(issue.waiting_on, Some(WaitingOn::Agent));
        assert!(issue.description.contains("**Answer:** Mine 1 j"));
    });
}

#[gpui_kit::test]
fn the_sidebar_counts_questions_and_o_opens_the_issue(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    let rows = cx.read(|cx| crate::shell::sidebar::scopes(&view.read(cx).store().read(cx).catalog));
    assert_eq!(rows[1].scope, Scope::Inbox);
    assert_eq!(
        cx.read(|cx| crate::inbox::open_count(view.read(cx).store().read(cx))),
        2
    );
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |this, cx| this.run_command("inbox", window, cx));
        window.render_frame(cx);
        window.press("o", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.read(|cx| {
        let this = view.read(cx);
        assert_eq!(this.scope(cx), Scope::All);
        let open = this.detail(cx).expect("the issue opens");
        assert_eq!(this.issue(open.issue, cx).key.as_ref(), "LLM-111");
    });
}

#[gpui_kit::test]
fn the_task_view_shows_questions_as_a_tab(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |this, cx| this.open_key("LLM-103", cx));
        window.render_frame(cx);
        let tabs = view
            .read(cx)
            .task()
            .read(cx)
            .open_tab(cx)
            .map(|(_, t)| t.into_iter().map(|t| t.title).collect::<Vec<_>>());
        assert!(
            tabs.unwrap_or_default()
                .iter()
                .any(|t| t == "Owner questions"),
            "the questions get their own tab"
        );
    })
    .unwrap();
    // The option buttons in the tab answer like the inbox.
    let issue = cx.read(|cx| view.read(cx).issue_by_key("LLM-103", cx).id);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| {
            this.answer_question(issue, 1, "a".into(), window, cx)
        });
    })
    .unwrap();
    assert_eq!(
        answers(&mut requests),
        [("LLM-103".to_string(), "Q1".to_string(), "a".to_string())]
    );
    // A bad question number changes nothing and says why.
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| {
            this.answer_question(issue, 7, "a".into(), window, cx)
        });
        assert!(view.read(cx).message().contains("no question Q7"));
    })
    .unwrap();
    assert!(answers(&mut requests).is_empty());
}

/// The owner clicks into the answer field and types, with no Enter first.
#[gpui_kit::test]
fn a_click_in_the_answer_field_takes_typing(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    let issue = cx.read(|cx| view.read(cx).issue_by_key("LLM-111", cx).id);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |this, cx| this.run_command("inbox", window, cx));
        window.render_frame(cx);
        window.click(format!("q-{issue}-1-input"), cx);
        window.render_frame(cx);
        assert_eq!(view.read(cx).vim_mode(window, cx), "insert");
        // Real keys, so key bindings get their chance to take them first.
        for key in [
            "shift-t", "y", "p", "e", "d", "space", "j", "space", "2", "g", "i", "o",
        ] {
            window.press(key, cx);
        }
    })
    .unwrap();
    cx.read(|cx| {
        let input = view.read(cx).answers().read(cx).get(issue, 1).unwrap();
        assert_eq!(input.read(cx).value().as_ref(), "Typed j 2gio");
    });
    cx.update_window(handle, |_, window, cx| window.press("enter", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        answers(&mut requests),
        [(
            "LLM-111".to_string(),
            "Q1".to_string(),
            "Typed j 2gio".to_string()
        )]
    );
}

/// The same field in the task view's Owner questions tab takes typing too.
/// A click in the task view used to move focus to the task view after the
/// field took it, so the keys went to the task view.
#[gpui_kit::test]
fn a_click_in_the_tab_answer_field_takes_typing(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    let issue = cx.read(|cx| view.read(cx).issue_by_key("LLM-111", cx).id);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |this, cx| this.open_key("LLM-111", cx));
        window.render_frame(cx);
        let task = view.read(cx).task().clone();
        let tab = task
            .read(cx)
            .open_tab(cx)
            .and_then(|(_, tabs)| tabs.iter().position(|t| t.title == "Owner questions"))
            .expect("the questions tab");
        task.update(cx, |task, cx| task.set_tab(tab, cx));
        window.render_frame(cx);
        window.click(format!("q-{issue}-1-input"), cx);
        window.render_frame(cx);
        assert_eq!(view.read(cx).vim_mode(window, cx), "insert");
        for key in ["o", "k", "space", "j", "1"] {
            window.press(key, cx);
        }
    })
    .unwrap();
    cx.read(|cx| {
        let input = view.read(cx).answers().read(cx).get(issue, 1).unwrap();
        assert_eq!(input.read(cx).value().as_ref(), "ok j1");
    });
    cx.update_window(handle, |_, window, cx| window.press("enter", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        answers(&mut requests),
        [("LLM-111".to_string(), "Q1".to_string(), "ok j1".to_string())]
    );
}

/// An empty inbox says so once, in the center. The header count is gone.
#[gpui_kit::test]
fn the_empty_inbox_shows_the_message_once(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    let free_text = cx.read(|cx| view.read(cx).issue_by_key("LLM-111", cx).id);
    let choice = cx.read(|cx| view.read(cx).issue_by_key("LLM-103", cx).id);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |this, cx| this.run_command("inbox", window, cx));
        window.render_frame(cx);
        // With open questions the header counts them, and the center lists them.
        assert_eq!(window.find_all("inbox-count").len(), 1);
        assert!(window.try_find("inbox-empty").is_none());
        view.update(cx, |this, cx| {
            this.answer_question(free_text, 1, "done".into(), window, cx);
            this.answer_question(choice, 1, "a".into(), window, cx);
        });
        window.render_frame(cx);
        assert_eq!(window.find_all("inbox-empty").len(), 1);
        assert!(window.try_find("inbox-count").is_none());
    })
    .unwrap();
    assert!(inbox_keys(&view, cx).is_empty());
}

/// Each option shows its letter in a small square, and a click on the
/// option button answers with that letter.
#[gpui_kit::test]
fn option_buttons_show_the_letter_in_a_square(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    let issue = cx.read(|cx| view.read(cx).issue_by_key("LLM-103", cx).id);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |this, cx| this.run_command("inbox", window, cx));
        window.render_frame(cx);
        let button = format!("q-{issue}-1-opt-b");
        let square = window
            .within(button.clone())
            .find(format!("{button}-letter"));
        let outer = window.find(button.clone());
        assert!(outer.bounds().contains(&square.bounds().center()));
        assert_eq!(square.bounds().size.width, square.bounds().size.height);
        window.click(button, cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        answers(&mut requests),
        [("LLM-103".to_string(), "Q1".to_string(), "b".to_string())]
    );
}

/// A choice takes a custom answer too: Enter focuses its field, and the
/// typed text is the answer.
#[gpui_kit::test]
fn a_choice_takes_a_typed_custom_answer(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |this, cx| this.run_command("inbox", window, cx));
        window.render_frame(cx);
        window.press("j", cx);
    })
    .unwrap();
    assert_eq!(selected_key(&view, cx).as_deref(), Some("LLM-103 Q1"));
    cx.update_window(handle, |_, window, cx| {
        window.press("enter", cx);
        assert_eq!(view.read(cx).vim_mode(window, cx), "insert");
        for key in ["n", "o", "n", "e"] {
            window.press(key, cx);
        }
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        answers(&mut requests),
        [("LLM-103".to_string(), "Q1".to_string(), "none".to_string())]
    );
    cx.read(|cx| {
        let issue = view.read(cx).issue_by_key("LLM-103", cx);
        let q = &llmtodo_core::questions(&issue.description)[0];
        assert_eq!(q.answer.as_deref(), Some("none"));
        assert_eq!(q.chosen(), None);
    });
}

/// How far apart two colors look once each is drawn on the page.
pub(crate) fn lightness_gap(page: gpui_kit::Hsla, a: gpui_kit::Hsla, b: gpui_kit::Hsla) -> f32 {
    (page.blend(a).l - page.blend(b).l).abs()
}

/// Hover turns an inbox button into the page's inverse: nearly white in the
/// dark theme, nearly black in the light one. The text stays readable, and
/// a press looks different from a hover.
#[gpui_kit::test]
fn inbox_buttons_invert_on_hover_in_both_themes(cx: &mut TestAppContext) {
    use gpui_kit::component::{ActiveTheme as _, ThemeMode};
    cx.update(crate::init);
    cx.update(|cx| {
        for (mode, inverse_is_light) in [(ThemeMode::Dark, true), (ThemeMode::Light, false)] {
            crate::shell::theme::set_mode(mode, cx);
            let page = cx.theme().background;
            let colors = crate::shell::widgets::inbox_button_colors(cx);
            assert_eq!(colors.hover_bg.l > 0.5, inverse_is_light, "{mode:?}");
            assert!(lightness_gap(page, colors.hover_bg, colors.hover_fg) > 0.5);
            assert!(lightness_gap(page, colors.hover_bg, colors.pressed_bg) > 0.05);
            assert!(lightness_gap(page, colors.pressed_bg, colors.hover_fg) > 0.3);
        }
    });
}

/// A probe that records the text color GPUI hands its children during
/// layout. GPUI shapes text in that phase and paints the run colors from it,
/// so this is the color the glyphs get. The test platform has no text system,
/// so a test cannot read glyphs from the scene.
pub(crate) struct TextColorProbe(pub(crate) ProbeCell);

impl gpui_kit::Element for TextColorProbe {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui_kit::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui_kit::GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::App,
    ) -> (gpui_kit::LayoutId, ()) {
        self.0.set(Some(window.text_style().color));
        (
            window.request_layout(gpui_kit::Style::default(), [], cx),
            (),
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui_kit::GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        _: gpui_kit::Bounds<gpui_kit::Pixels>,
        _: &mut (),
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&gpui_kit::GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        _: gpui_kit::Bounds<gpui_kit::Pixels>,
        _: &mut (),
        _: &mut (),
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::App,
    ) {
    }
}

impl gpui_kit::IntoElement for TextColorProbe {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

pub(crate) type ProbeCell = std::rc::Rc<std::cell::Cell<Option<gpui_kit::Hsla>>>;

/// One real option button with a probe in place of its letter and its label.
struct ProbedOption {
    letter: ProbeCell,
    label: ProbeCell,
}

impl gpui_kit::Render for ProbedOption {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{ParentElement as _, Styled as _};
        gpui_kit::div()
            .size_full()
            .child(crate::inbox::option_button(
                "probed-option".into(),
                false,
                TextColorProbe(self.letter.clone()),
                TextColorProbe(self.label.clone()),
                cx,
            ))
    }
}

/// The pointer on an inbox button changes the color its text is shaped with.
/// The letter takes the opposite of the hovered button, so it stays visible.
#[gpui_kit::test]
fn hovering_an_option_changes_the_drawn_text_color(cx: &mut TestAppContext) {
    use gpui_kit::component::ThemeMode;
    cx.update(crate::init);
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        let letter: ProbeCell = Default::default();
        let label: ProbeCell = Default::default();
        let (handle, hover_bg, hover_fg) = cx.update(|cx| {
            crate::shell::theme::set_mode(mode, cx);
            let colors = crate::shell::widgets::inbox_button_colors(cx);
            let (letter, label) = (letter.clone(), label.clone());
            let (handle, _view) = gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|_| ProbedOption { letter, label })
            })
            .unwrap();
            (handle, colors.hover_bg, colors.hover_fg)
        });
        let (rest_label, hovered) = cx
            .update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                let rest_label = label.get().unwrap();
                window.hover("probed-option", cx);
                window.render_frame(cx);
                (rest_label, (letter.get().unwrap(), label.get().unwrap()))
            })
            .unwrap();
        assert_ne!(
            rest_label, hover_fg,
            "{mode:?}: rest text is not the hover text"
        );
        assert_eq!(hovered.1, hover_fg, "{mode:?}: label on the hovered button");
        assert_eq!(
            hovered.0, hover_bg,
            "{mode:?}: letter in the hovered square"
        );
    }
}

/// The letter square has the same space on the left, top and bottom. Every
/// option has the same gap to the next one.
#[gpui_kit::test]
fn option_buttons_are_evenly_spaced(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    let issue = cx.read(|cx| view.read(cx).issue_by_key("LLM-103", cx).id);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |this, cx| this.run_command("inbox", window, cx));
        window.render_frame(cx);
        let button = |letter: char, window: &mut gpui_kit::Window| {
            window.find(format!("q-{issue}-1-opt-{letter}")).bounds()
        };
        let (a, b) = (button('a', window), button('b', window));
        let square = window.find(format!("q-{issue}-1-opt-a-letter")).bounds();
        let left = square.left() - a.left();
        assert_eq!(square.top() - a.top(), left, "top inset equals left inset");
        assert_eq!(
            a.bottom() - square.bottom(),
            left,
            "bottom inset equals left inset"
        );
        assert_eq!(b.top(), a.top(), "options share a row");
        assert_eq!(
            b.left() - a.right(),
            gpui_kit::px(8.),
            "gap between options"
        );
        // The square and the button are drawn with one corner radius.
        let scale = window.scale_factor();
        let radius_of = |bounds: gpui_kit::Bounds<gpui_kit::Pixels>, window: &gpui_kit::Window| {
            window
                .painted_quads()
                .iter()
                .find(|q| q.bounds == bounds.scale(scale))
                .map(|q| q.corner_radii.top_left)
                .expect("a quad at these bounds")
        };
        assert_eq!(radius_of(square, window), radius_of(a, window));
    })
    .unwrap();
}

/// Every text field shares one height.
#[gpui_kit::test]
fn the_answer_field_matches_its_button(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    let issue = cx.read(|cx| view.read(cx).issue_by_key("LLM-103", cx).id);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |this, cx| this.run_command("inbox", window, cx));
        window.render_frame(cx);
        let field = window.find(format!("q-{issue}-1-input")).bounds();
        let send = window.find(format!("q-{issue}-1-send")).bounds();
        assert_eq!(field.size.height, gpui_kit::px(32.));
        assert_eq!(send.size.height, field.size.height);
    })
    .unwrap();
}

/// The selected question has no fill, so its border carries the selection.
#[gpui_kit::test]
fn the_selected_card_has_a_stronger_border(cx: &mut TestAppContext) {
    use gpui_kit::component::{ActiveTheme as _, ThemeMode};
    cx.update(crate::init);
    cx.update(|cx| {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            crate::shell::theme::set_mode(mode, cx);
            let page = cx.theme().background;
            let plain = crate::inbox::card_border(false, cx);
            let selected = crate::inbox::card_border(true, cx);
            assert!(
                lightness_gap(page, page, selected) > lightness_gap(page, page, plain) + 0.1,
                "{mode:?}"
            );
        }
    });
}

/// Hovering an option and the Answer button draws without trouble.
#[gpui_kit::test]
fn inbox_buttons_draw_while_hovered(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    let issue = cx.read(|cx| view.read(cx).issue_by_key("LLM-103", cx).id);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |this, cx| this.run_command("inbox", window, cx));
        window.render_frame(cx);
        for id in [format!("q-{issue}-1-opt-b"), format!("q-{issue}-1-send")] {
            window.hover(id, cx);
            window.render_frame(cx);
        }
    })
    .unwrap();
}

/// The undo requests sent to the server, as `(key, question ID)`.
fn undos(requests: &mut UnboundedReceiver<Request>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    while let Ok(r) = requests.try_recv() {
        if let Request::Write {
            key,
            write: Write::UndoAnswer { id },
            kind: WriteKind::UndoAnswer,
        } = r
        {
            out.push((key, id));
        }
    }
    out
}

/// Opens the inbox, moves to LLM-103 Q1 and answers it with `2`.
fn answer_llm_103(
    handle: AnyWindowHandle,
    view: &Entity<Workspace>,
    requests: &mut UnboundedReceiver<Request>,
    cx: &mut TestAppContext,
) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |this, cx| this.run_command("inbox", window, cx));
        window.render_frame(cx);
        window.press("j", cx);
        window.press("2", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(answers(requests).len(), 1);
}

fn write_done(view: &Entity<Workspace>, kind: WriteKind, cx: &mut TestAppContext) {
    view.update(cx, |this, cx| {
        this.apply_event(
            crate::model::backend::Event::WriteDone {
                key: "LLM-103".into(),
                kind,
                version: 9,
            },
            cx,
        );
    });
}

fn is_answered(view: &Entity<Workspace>, cx: &TestAppContext) -> bool {
    cx.read(|cx| {
        let issue = view.read(cx).issue_by_key("LLM-103", cx);
        llmtodo_core::questions(&issue.description)[0]
            .answer
            .is_some()
    })
}

/// `u` asks the server to undo. The question comes back, selected, when
/// the server's copy arrives.
#[gpui_kit::test]
fn u_takes_back_the_last_answer(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    answer_llm_103(handle, &view, &mut requests, cx);
    write_done(&view, WriteKind::Answer, cx);
    cx.update_window(handle, |_, window, cx| {
        assert_eq!(window.find_all("answer-undo-button").len(), 1);
        window.press("u", cx);
        window.render_frame(cx);
        assert!(window.try_find("answer-undo-button").is_none());
        assert_eq!(view.read(cx).message(), "Undoing the answer to LLM-103 Q1");
    })
    .unwrap();
    assert_eq!(
        undos(&mut requests),
        [("LLM-103".to_string(), "Q1".to_string())]
    );
    // Nothing changes until the server agrees.
    assert!(is_answered(&view, cx));

    let mut server = cx.read(|cx| view.read(cx).issue_by_key("LLM-103", cx).clone());
    server.description = llmtodo_core::questions::answer(&server.description, "Q1", "")
        .unwrap()
        .into();
    server.open_questions = 1;
    server.waiting_on = Some(WaitingOn::Owner);
    view.update(cx, |this, cx| this.receive_issue(server, cx));
    write_done(&view, WriteKind::UndoAnswer, cx);

    assert!(!is_answered(&view, cx));
    assert_eq!(selected_key(&view, cx).as_deref(), Some("LLM-103 Q1"));
    cx.read(|cx| {
        let this = view.read(cx);
        assert_eq!(this.message(), "Reopened LLM-103 Q1");
        assert_eq!(
            this.issue_by_key("LLM-103", cx).waiting_on,
            Some(WaitingOn::Owner)
        );
    });
}

/// The offer ends after a few seconds, and `u` then says so.
#[gpui_kit::test]
fn the_undo_offer_ends_after_a_few_seconds(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    answer_llm_103(handle, &view, &mut requests, cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(9));
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("answer-undo-button").is_none());
        window.press("u", cx);
        assert_eq!(view.read(cx).message(), "No answer to undo");
    })
    .unwrap();
    assert!(undos(&mut requests).is_empty());
}

/// A click sends the undo too. When the server refuses, the status bar
/// gives its reason and the answer stays.
#[gpui_kit::test]
fn a_refused_undo_says_why_and_keeps_the_answer(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    answer_llm_103(handle, &view, &mut requests, cx);
    write_done(&view, WriteKind::Answer, cx);
    cx.update_window(handle, |_, window, cx| {
        window.click("answer-undo-button", cx);
    })
    .unwrap();
    assert_eq!(undos(&mut requests).len(), 1);
    view.update(cx, |this, cx| {
        this.apply_event(
            crate::model::backend::Event::WriteFailed {
                key: "LLM-103".into(),
                kind: WriteKind::UndoAnswer,
                message: "an agent already acted on LLM-103 after the answer".into(),
                stale: false,
            },
            cx,
        );
    });
    assert!(is_answered(&view, cx));
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).message(),
            "Could not undo: an agent already acted on LLM-103 after the answer"
        );
    });
}

/// An answer the server refused has nothing to undo.
#[gpui_kit::test]
fn a_failed_answer_ends_the_undo_offer(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    answer_llm_103(handle, &view, &mut requests, cx);
    view.update(cx, |this, cx| {
        this.apply_event(
            crate::model::backend::Event::WriteFailed {
                key: "LLM-103".into(),
                kind: WriteKind::Answer,
                message: "offline".into(),
                stale: false,
            },
            cx,
        );
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("answer-undo-button").is_none());
    })
    .unwrap();
}

/// Drops the text of every issue, as a snapshot does, and puts in the
/// server's question list instead. The versions move on, so the store
/// cannot keep the text it held.
fn drop_descriptions(view: &Entity<Workspace>, cx: &mut TestAppContext) {
    view.update(cx, |this, cx| {
        let (issues, catalog) = this.issues_and_catalog(cx);
        let list = issues
            .iter()
            .map(|i| (i.id, llmtodo_core::open_questions(&i.description)))
            .filter(|(_, qs)| !qs.is_empty())
            .collect();
        let issues = issues
            .into_iter()
            .map(|mut i| {
                i.description = "".into();
                i.description_loaded = false;
                i.version += 1;
                i
            })
            .collect();
        this.store()
            .update(cx, |store, _| store.set_open_questions(list, |_| false));
        this.load(catalog, issues, cx);
    });
}

/// The inbox reads the server's question list. An answer leaves the list
/// at once, before the server's copy comes, though the app holds no text.
#[gpui_kit::test]
fn an_answer_works_without_the_description(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    drop_descriptions(&view, cx);
    assert_eq!(inbox_keys(&view, cx), ["LLM-111 Q1", "LLM-103 Q1"]);
    answer_llm_103(handle, &view, &mut requests, cx);
    // The answer above was sent as a patch.
    assert_eq!(inbox_keys(&view, cx), ["LLM-111 Q1"]);
    cx.read(|cx| {
        let this = view.read(cx);
        let issue = this.issue_by_key("LLM-103", cx);
        assert!(!issue.description_loaded);
        assert_eq!(issue.open_questions, 0);
        assert_eq!(issue.waiting_on, Some(WaitingOn::Agent));
        assert_eq!(
            this.message(),
            "Answered LLM-103 Q1: b) :assign and :move. It goes back to the agents"
        );
    });

    // A refused answer: the server's copy, with the text, brings it back.
    let mut server = cx.read(|cx| view.read(cx).issue_by_key("LLM-103", cx).clone());
    server.description = crate::model::demo::issues(&crate::model::demo::catalog())
        .into_iter()
        .find(|i| i.key.as_ref() == "LLM-103")
        .unwrap()
        .description;
    server.description_loaded = true;
    server.open_questions = 1;
    view.update(cx, |this, cx| this.receive_issue(server, cx));
    write_done(&view, WriteKind::Answer, cx);
    assert_eq!(inbox_keys(&view, cx), ["LLM-111 Q1", "LLM-103 Q1"]);
}
