//! UI integration tests for vim navigation. They open a headless window and
//! send real key events through GPUI's dispatch, key contexts included.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, SharedString, TestAppContext, WindowBounds,
    WindowOptions, px, size,
};

use crate::shell::workspace::{Row, Workspace};
use crate::task::task_view::DetailMode;
use crate::task::vim_editor::VimMode;

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    open_sized(cx, 1200., 760.)
}

fn open_sized(
    cx: &mut TestAppContext,
    width: f32,
    height: f32,
) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(width), px(height)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .expect("open test window")
    })
}

#[gpui_kit::test]
fn vim_keys_move_open_and_go_back(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let last = cx.read(|cx| view.read(cx).visible_count(cx) - 1);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("j", cx);
        window.press("j", cx);
        assert_eq!(view.read(cx).selected(cx), 2);
        window.press("k", cx);
        assert_eq!(view.read(cx).selected(cx), 1);

        window.press("shift-g", cx);
        assert_eq!(view.read(cx).selected(cx), last);
        window.press("g", cx);
        window.press("g", cx);
        assert_eq!(view.read(cx).selected(cx), 0);

        window.press("enter", cx);
        let detail = view
            .read(cx)
            .detail(cx)
            .expect("Enter opens the detail pane");
        assert_eq!(
            view.read(cx).issue(detail.issue, cx).key.as_ref(),
            "LLM-101"
        );
        assert_eq!(detail.mode, DetailMode::Preview);

        // The preview follows the selection.
        window.press("j", cx);
        assert_eq!(
            view.read(cx).detail(cx).map(|d| d.issue),
            Some(view.read(cx).issue_by_key("LLM-102", cx).id)
        );

        window.press("escape", cx);
        assert!(view.read(cx).detail(cx).is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn vim_keys_do_not_fire_while_typing(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("/", cx);
        assert_eq!(view.read(cx).vim_mode(window, cx), "insert");

        // `j` and `k` are text here, not moves.
        window.input("jk", cx);
        assert_eq!(view.read(cx).filter_text(cx).as_ref(), "jk");
        assert_eq!(view.read(cx).selected(cx), 0);

        // Escape leaves the filter and keeps its text.
        window.press("escape", cx);
        assert_eq!(view.read(cx).vim_mode(window, cx), "normal");
        assert_eq!(view.read(cx).filter_text(cx).as_ref(), "jk");
    })
    .unwrap();
}

#[gpui_kit::test]
fn palette_runs_a_command_and_edit_round_trips(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press(":", cx);
        assert!(view.read(cx).palette_open(cx));
        window.input("status done", cx);
        window.press("enter", cx);
    })
    .unwrap();

    // Input events reach subscribers when the update that emitted them ends.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(!view.read(cx).palette_open(cx));
        assert_eq!(
            view.read(cx).issue_by_key("LLM-101", cx).status.name,
            "Done"
        );

        window.press("enter", cx);
        window.press("e", cx);
        assert_eq!(
            view.read(cx).detail(cx).map(|d| d.mode),
            Some(DetailMode::Editing)
        );
        // The editor opens in vim normal mode, and the list keys are off.
        assert_eq!(view.read(cx).vim_mode(window, cx), "insert");
        assert_eq!(view.read(cx).editor_vim(cx), VimMode::Normal);
        window.press("i", cx);
        assert_eq!(view.read(cx).editor_vim(cx), VimMode::Insert);
        window.input("j", cx);
        assert_eq!(view.read(cx).selected(cx), 0);

        // Esc leaves insert mode, a second Esc saves and closes.
        window.press("escape", cx);
        assert_eq!(view.read(cx).editor_vim(cx), VimMode::Normal);
        assert_eq!(
            view.read(cx).detail(cx).map(|d| d.mode),
            Some(DetailMode::Editing)
        );
        window.press("escape", cx);
        assert_eq!(
            view.read(cx).detail(cx).map(|d| d.mode),
            Some(DetailMode::Preview)
        );
        assert!(
            view.read(cx)
                .issue_by_key("LLM-101", cx)
                .description
                .starts_with('j')
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn editor_folds_plus_sections(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let expected = cx.read(|cx| {
        crate::task::fold_highlighter::fold_ranges(
            &view.read(cx).issue_by_key("LLM-101", cx).description,
        )
    });
    assert_eq!(expected.len(), 2, "fixture has two sections");

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.press("e", cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let editor = view.read(cx).editor(cx).read(cx);
        for fold in &expected {
            assert!(
                editor.display_map().is_fold_candidate(fold.start_line),
                "line {} should fold",
                fold.start_line
            );
        }
        // A line that is not a `+++` opener is not a fold candidate.
        assert!(!editor.display_map().is_fold_candidate(0));

        // The gutter fold icons are painted by a custom element with no
        // test IDs, so a test cannot click them. The owner checks the mouse
        // path in the preview.
    })
    .unwrap();
}

/// LLM-173: the folds follow each edit. A new first line moves every
/// section down one line.
#[gpui_kit::test]
fn editor_folds_follow_a_typed_line(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let expected = cx.read(|cx| {
        crate::task::fold_highlighter::fold_ranges(
            &view.read(cx).issue_by_key("LLM-101", cx).description,
        )
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.press("e", cx);
        window.press("i", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let editor = view.read(cx).editor(cx).read(cx);
        assert!(editor.value().starts_with('\n'));
        for fold in &expected {
            let map = editor.display_map();
            assert!(map.is_fold_candidate(fold.start_line + 1));
            assert!(!map.is_fold_candidate(fold.start_line));
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn editor_normal_mode_edits_without_typing(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let text = |cx: &mut gpui_kit::App| view.read(cx).editor(cx).read(cx).value().to_string();

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // LLM-104 has a short description.
        window.press("j", cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("enter", cx);
        window.press("e", cx);
        let before = text(cx);
        assert!(before.starts_with("Show claims in the sidebar.\n"));

        // Unbound letters do not type in normal mode.
        window.press("q", cx);
        window.press("z", cx);
        assert_eq!(text(cx), before);

        // `x` deletes, `u` undoes.
        window.press("x", cx);
        assert!(text(cx).starts_with("how claims"));
        window.press("u", cx);
        assert_eq!(text(cx), before);

        // `dd` removes the first line, `p` puts it back below.
        window.press("d", cx);
        window.press("d", cx);
        assert!(text(cx).starts_with("\n+++ Agent notes"));
        window.press("p", cx);
        assert!(text(cx).starts_with("\nShow claims in the sidebar.\n+++"));

        // `o` opens a line and enters insert mode, where letters type.
        window.press("g", cx);
        window.press("g", cx);
        window.press("o", cx);
        assert_eq!(view.read(cx).editor_vim(cx), VimMode::Insert);
        window.press("q", cx);
        assert!(text(cx).starts_with("\nq\n"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn question_mark_toggles_help(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("?", cx);
        assert!(view.read(cx).help_open());
        window.press("escape", cx);
        assert!(!view.read(cx).help_open());

        // Help works from the editor's normal mode too.
        window.press("enter", cx);
        window.press("e", cx);
        window.press("?", cx);
        assert!(view.read(cx).help_open());
        window.press("?", cx);
        assert!(!view.read(cx).help_open());
    })
    .unwrap();
}

#[gpui_kit::test]
fn filter_keeps_first_match_selected_and_ctrl_n_moves(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("/", cx);
        window.input("theme", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // Filtering resets the selection to the first match.
        assert_eq!(view.read(cx).selected(cx), 0);
        assert!(view.read(cx).visible_count(cx) >= 2);
        window.press("ctrl-n", cx);
        assert_eq!(view.read(cx).selected(cx), 1);
        window.press("ctrl-p", cx);
        assert_eq!(view.read(cx).selected(cx), 0);
        window.press("down", cx);
        assert_eq!(view.read(cx).selected(cx), 1);
        window.press("enter", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let detail = view.read(cx).detail(cx).expect("Enter opens the match");
        assert_eq!(
            view.read(cx).issue(detail.issue, cx).key,
            view.read(cx).visible_issue(1, cx).key
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn palette_ctrl_n_picks_a_suggestion_and_sorts(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press(":", cx);
        window.input("sort", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // Suggestions: sort status, sort added, sort key. The first is
        // selected.
        assert_eq!(view.read(cx).palette_selected(cx), 0);
        window.press("ctrl-n", cx);
        window.press("ctrl-n", cx);
        window.press("ctrl-p", cx);
        assert_eq!(view.read(cx).palette_selected(cx), 1);
        window.press("enter", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(!view.read(cx).palette_open(cx));
        // Sorted by date added, newest first.
        let first = view.read(cx).visible_issue(0, cx).added;
        let last = view
            .read(cx)
            .visible_issue(view.read(cx).visible_count(cx) - 1, cx)
            .added;
        assert!(first > last);

        window.press(":", cx);
        window.input("sort status", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let count = view.read(cx).visible_count(cx);
        assert_eq!(
            view.read(cx).visible_issue(0, cx).status.name,
            "In Progress"
        );
        assert_eq!(
            view.read(cx).visible_issue(count - 1, cx).status.name,
            "Done"
        );
    })
    .unwrap();
}

/// LLM-120: Enter on `a` selected `assign me`, so a short typed command
/// made the owner the assignee.
#[gpui_kit::test]
fn palette_partial_word_does_not_assign(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let unassigned = cx.read(|cx| {
        (0..view.read(cx).visible_count(cx))
            .find(|ix| view.read(cx).visible_issue(*ix, cx).assignee.is_none())
            .expect("a task with no assignee")
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        for _ in 0..unassigned {
            window.press("j", cx);
        }
    })
    .unwrap();

    run_palette(handle, "a", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let this = view.read(cx);
        assert!(!this.palette_open(cx));
        assert!(this.visible_issue(unassigned, cx).assignee.is_none());
        assert_eq!(this.message(), "Unknown command: a");
    })
    .unwrap();

    run_palette(handle, "assign", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            view.read(cx)
                .visible_issue(unassigned, cx)
                .assignee
                .is_some()
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn ctrl_c_cancels_palette_and_filter(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press(":", cx);
        assert!(view.read(cx).palette_open(cx));
        window.press("ctrl-c", cx);
        assert!(!view.read(cx).palette_open(cx));
        assert_eq!(view.read(cx).vim_mode(window, cx), "normal");

        window.press("/", cx);
        assert_eq!(view.read(cx).vim_mode(window, cx), "insert");
        window.press("ctrl-c", cx);
        assert_eq!(view.read(cx).vim_mode(window, cx), "normal");
    })
    .unwrap();
}

#[gpui_kit::test]
fn ctrl_c_closes_help(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("?", cx);
        assert!(view.read(cx).help_open());
        window.press("ctrl-c", cx);
        assert!(!view.read(cx).help_open());
        // In the list, Ctrl-C with nothing to cancel does nothing.
        window.press("ctrl-c", cx);
        assert_eq!(view.read(cx).vim_mode(window, cx), "normal");
    })
    .unwrap();
}

#[gpui_kit::test]
fn group_by_label_adds_headers_that_open_and_close(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run_palette(handle, "group label", cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let rows = view.read(cx).rows(cx).to_vec();
        assert!(
            matches!(rows[0], Row::Header { .. }),
            "first row is a header"
        );
        let headers = rows
            .iter()
            .filter(|r| matches!(r, Row::Header { .. }))
            .count();
        assert!(headers >= 6, "one header per used label, and No label");
        // Grouping keeps the selected issue selected.
        assert_eq!(
            view.read(cx).selected_issue_key(cx).as_deref(),
            Some("LLM-101")
        );

        // gg lands on the first header. Enter closes its group.
        window.press("g", cx);
        window.press("g", cx);
        assert_eq!(view.read(cx).selected(cx), 0);
        let before = view.read(cx).rows(cx).len();
        window.press("enter", cx);
        let rows = view.read(cx).rows(cx).to_vec();
        assert!(matches!(
            rows[0],
            Row::Header {
                collapsed: true,
                ..
            }
        ));
        assert!(rows.len() < before, "the closed group hides its issues");
        assert_eq!(view.read(cx).selected(cx), 0, "the header stays selected");
        // Enter again opens it.
        window.press("enter", cx);
        assert_eq!(view.read(cx).rows(cx).len(), before);

        // za on a task closes its group and selects the header.
        window.press("j", cx);
        window.press("z", cx);
        window.press("a", cx);
        assert!(matches!(
            view.read(cx).rows(cx)[0],
            Row::Header {
                collapsed: true,
                ..
            }
        ));
        assert_eq!(view.read(cx).selected(cx), 0);
        // zM closes all, zR opens all.
        window.press("z", cx);
        window.press("shift-m", cx);
        assert!(view.read(cx).rows(cx).iter().all(|r| matches!(
            r,
            Row::Header {
                collapsed: true,
                ..
            }
        )));
        window.press("z", cx);
        window.press("shift-r", cx);
        assert_eq!(view.read(cx).rows(cx).len(), before);
    })
    .unwrap();
}

#[gpui_kit::test]
fn dragging_a_task_onto_a_status_group_changes_its_status(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run_palette(handle, "group status", cx);

    let (from, to) = cx.read(|cx| {
        let rows = view.read(cx).rows(cx);
        let llm_101 = view.read(cx).issue_by_key("LLM-101", cx).id;
        // LLM-101 is In Progress. Drop it on the Done header.
        let from = rows
            .iter()
            .position(|r| *r == Row::Issue(llm_101))
            .expect("LLM-101 row");
        let to = rows
            .iter()
            .position(|r| matches!(r, Row::Header { title, .. } if title.as_ref() == "Done"))
            .expect("Done header");
        (from, to)
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.drag_to(("row", from), ("row", to), cx);
    })
    .unwrap();
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).issue_by_key("LLM-101", cx).status.name,
            "Done"
        )
    });
}

#[gpui_kit::test]
fn settings_are_saved_and_restored(cx: &mut TestAppContext) {
    use crate::settings::SettingsStore;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ui.json");
    cx.update(crate::init);
    let open_with = |cx: &mut TestAppContext| {
        let store = SettingsStore::at(path.clone());
        let saved = store.load();
        cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
                cx.new(|cx| {
                    let mut this = Workspace::with_settings(saved, store, window, cx);
                    this.load_demo(cx);
                    this
                })
            })
            .unwrap()
        })
    };

    let (handle, view) = open_with(cx);
    run_palette(handle, "group status", cx);
    run_palette(handle, "sort added", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("g", cx);
        window.press("g", cx);
        window.press("enter", cx);
    })
    .unwrap();
    let saved = cx.read(|cx| view.read(cx).current_settings(cx));
    assert_eq!(saved.group, "status");
    assert_eq!(saved.sort, "added");
    assert_eq!(saved.collapsed_groups, ["status:In Progress"]);
    assert!(path.exists(), "saved on change, not only on quit");

    // A new window starts from the saved file.
    let (_, view) = open_with(cx);
    cx.read(|cx| {
        let restored = view.read(cx).current_settings(cx);
        assert_eq!(restored.group, saved.group);
        assert_eq!(restored.sort, saved.sort);
        assert_eq!(restored.collapsed_groups, saved.collapsed_groups);
    });
}

#[gpui_kit::test]
fn enter_twice_moves_into_the_task_and_jk_scroll(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // LLM-102 has a long description.
        window.press("j", cx);
        window.press("enter", cx);
        assert!(!view.read(cx).detail_focused(window, cx));
        window.press("enter", cx);
        assert!(view.read(cx).detail_focused(window, cx));
        window.render_frame(cx);

        // j and k scroll the description, not the list.
        window.press("j", cx);
        window.press("j", cx);
        window.render_frame(cx);
        assert_eq!(view.read(cx).selected(cx), 1);
        let scrolled = view.read(cx).preview_offset(cx);
        assert!(scrolled < gpui_kit::px(0.), "description scrolled down");
        window.press("k", cx);
        window.render_frame(cx);
        assert!(view.read(cx).preview_offset(cx) > scrolled);
        window.press("shift-g", cx);
        window.render_frame(cx);
        assert!(view.read(cx).preview_offset(cx) < scrolled);
        window.press("g", cx);
        window.press("g", cx);
        window.render_frame(cx);
        assert_eq!(view.read(cx).preview_offset(cx), gpui_kit::px(0.));

        // `:` still opens the palette from the task view.
        window.press(":", cx);
        assert!(view.read(cx).palette_open(cx));
        window.press("escape", cx);
        assert!(view.read(cx).detail_focused(window, cx));

        // Esc leaves the task view and keeps it open.
        window.press("escape", cx);
        assert!(!view.read(cx).detail_focused(window, cx));
        assert!(view.read(cx).detail(cx).is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn ctrl_a_h_and_l_move_between_panes(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-a", cx);
        window.press("l", cx);
        assert!(
            view.read(cx).detail_focused(window, cx),
            "Ctrl-A l opens and enters the task"
        );
        window.press("ctrl-a", cx);
        window.press("h", cx);
        assert!(!view.read(cx).detail_focused(window, cx));
        window.press("l", cx);
        assert!(view.read(cx).detail_focused(window, cx));
        window.press("h", cx);
        assert!(!view.read(cx).detail_focused(window, cx));

        // From the editor's normal mode too.
        window.press("e", cx);
        window.press("ctrl-a", cx);
        window.press("h", cx);
        assert!(!view.read(cx).detail_focused(window, cx));
        window.press("ctrl-a", cx);
        window.press("l", cx);
        assert!(view.read(cx).detail_focused(window, cx));
        assert_eq!(view.read(cx).editor_vim(cx), VimMode::Normal);
    })
    .unwrap();
}

#[gpui_kit::test]
fn clicking_a_task_opens_it_to_the_side(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(view.read(cx).detail(cx).is_none());
        window.click(("row", 3usize), cx);
        assert_eq!(view.read(cx).selected(cx), 3);
        let detail = view.read(cx).detail(cx).expect("a click opens the task");
        assert_eq!(
            view.read(cx).issue(detail.issue, cx).key.as_ref(),
            "LLM-104"
        );
        // Focus stays in the list.
        assert!(!view.read(cx).detail_focused(window, cx));
        window.press("j", cx);
        assert_eq!(view.read(cx).selected(cx), 4);

        // Clicking another task switches the task view to it.
        window.click(("row", 1usize), cx);
        let detail = view.read(cx).detail(cx).unwrap();
        assert_eq!(
            view.read(cx).issue(detail.issue, cx).key.as_ref(),
            "LLM-102"
        );
    })
    .unwrap();
}

fn run_palette(handle: AnyWindowHandle, command: &str, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press(":", cx);
        window.input(command, cx);
        window.press("enter", cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn task_id_copies_by_click_and_palette(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click(("row-key", 2usize), cx);
    })
    .unwrap();
    let copied = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some("LLM-103"));
    cx.read(|cx| assert_eq!(view.read(cx).message(), "Copied LLM-103"));

    // `:copy` copies the selected task. The click above selected LLM-103,
    // so move to the next one first.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("j", cx);
    })
    .unwrap();
    run_palette(handle, "copy", cx);
    let copied = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some("LLM-104"));
}

#[gpui_kit::test]
fn label_command_toggles_a_label(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let has_bug = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            view.read(cx)
                .issue_by_key("LLM-101", cx)
                .labels
                .iter()
                .any(|l| l.name == "bug")
        })
    };
    assert!(!has_bug(cx));
    run_palette(handle, "label bug", cx);
    assert!(has_bug(cx));
    run_palette(handle, "label bug", cx);
    assert!(!has_bug(cx));
}

#[gpui_kit::test]
fn delete_moves_to_trash_and_restore_brings_back(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let count = cx.read(|cx| view.read(cx).visible_count(cx));

    run_palette(handle, "delete", cx);
    cx.read(|cx| {
        assert_eq!(view.read(cx).visible_count(cx), count - 1);
        assert!(view.read(cx).issue_by_key("LLM-101", cx).deleted);
        // The next task takes the selection.
        assert_eq!(
            view.read(cx).selected_issue_key(cx).as_deref(),
            Some("LLM-102")
        );
    });

    run_palette(handle, "restore", cx);
    cx.read(|cx| {
        assert_eq!(view.read(cx).visible_count(cx), count);
        assert_eq!(
            view.read(cx).selected_issue_key(cx).as_deref(),
            Some("LLM-101")
        );
    });
}

#[gpui_kit::test]
fn narrow_window_stacks_the_task_view_below(cx: &mut TestAppContext) {
    let (handle, view) = open_sized(cx, 760., 900.);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(view.read(cx).stacked(window));
        window.press("enter", cx);
        window.render_frame(cx);
        assert!(view.read(cx).detail(cx).is_some());
        // Keys work the same in the stacked layout.
        window.press("enter", cx);
        assert!(view.read(cx).detail_focused(window, cx));
    })
    .unwrap();

    let (handle, view) = open_sized(cx, 1200., 760.);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(!view.read(cx).stacked(window));
    })
    .unwrap();
}

#[gpui_kit::test]
fn plus_sections_become_tabs(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let tab_title = |view: &Entity<Workspace>, cx: &gpui_kit::App| {
        let (ix, tabs) = view.read(cx).open_tab(cx).expect("a task is open");
        tabs[ix].title.clone()
    };

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // LLM-101 has a description and two sections.
        window.press("enter", cx);
        let (_, tabs) = view.read(cx).open_tab(cx).unwrap();
        let titles: Vec<_> = tabs.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Description", "Agent notes", "Work log"]);
        assert_eq!(tab_title(&view, cx), "Description");

        // gt / gT work from the list and from inside the task view.
        window.press("g", cx);
        window.press("t", cx);
        assert_eq!(tab_title(&view, cx), "Agent notes");
        window.press("enter", cx);
        assert!(view.read(cx).detail_focused(window, cx));
        window.press("g", cx);
        window.press("t", cx);
        assert_eq!(tab_title(&view, cx), "Work log");
        window.press("g", cx);
        window.press("t", cx);
        assert_eq!(tab_title(&view, cx), "Description", "gt wraps around");
        window.press("g", cx);
        window.press("shift-t", cx);
        assert_eq!(tab_title(&view, cx), "Work log");
    })
    .unwrap();

    run_palette(handle, "tab agent", cx);
    cx.read(|cx| assert_eq!(tab_title(&view, cx), "Agent notes"));
}

#[gpui_kit::test]
fn edit_preview_toggle_switches_modes(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let mode = |cx: &gpui_kit::App| view.read(cx).detail(cx).map(|d| d.mode);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
        assert_eq!(mode(cx), Some(DetailMode::Preview));
        window.click("mode-edit", cx);
        assert_eq!(mode(cx), Some(DetailMode::Editing));
        window.render_frame(cx);
        window.click("mode-preview", cx);
        assert_eq!(mode(cx), Some(DetailMode::Preview));
        assert!(view.read(cx).detail_focused(window, cx));
    })
    .unwrap();
}

#[gpui_kit::test]
fn focus_outline_shows_for_keyboard_focus_only(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.press("enter", cx);
        assert!(
            view.read(cx).show_focus_outline(window, cx),
            "keyboard focus"
        );
        window.press("escape", cx);
        window.render_frame(cx);

        // A click into the task view focuses it without the outline.
        window.click("preview", cx);
        assert!(view.read(cx).detail_focused(window, cx));
        assert!(!view.read(cx).show_focus_outline(window, cx), "mouse focus");
        // The next key brings the outline back.
        window.press("j", cx);
        assert!(view.read(cx).show_focus_outline(window, cx));
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_claimed_task_is_read_only_until_released(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    // LLM-102 starts claimed by Claude.
    cx.read(|cx| assert!(view.read(cx).claim_of("LLM-102", cx).is_some()));

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("j", cx);
        window.press("enter", cx);
        window.render_frame(cx);
        // E is refused with a message, and the preview stays.
        window.press("e", cx);
        assert_eq!(
            view.read(cx).detail(cx).map(|d| d.mode),
            Some(DetailMode::Preview)
        );
        assert!(view.read(cx).message().contains("locked"));
    })
    .unwrap();

    // Status changes are refused too.
    run_palette(handle, "status done", cx);
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).issue_by_key("LLM-102", cx).status.name,
            "In Progress"
        )
    });

    // Once released, the task can be edited.
    run_palette(handle, "release", cx);
    cx.read(|cx| assert!(view.read(cx).claim_of("LLM-102", cx).is_none()));
    run_palette(handle, "status done", cx);
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).issue_by_key("LLM-102", cx).status.name,
            "Done"
        )
    });

    // A live update with a new claim locks it again and says so.
    view.update(cx, |this, cx| {
        let mut issue = this.issue_by_key("LLM-102", cx).clone();
        issue.claim = Some(crate::model::demo::claude_claim("LLM-102"));
        this.upsert_issue(issue, cx);
    });
    cx.read(|cx| {
        assert!(view.read(cx).claim_of("LLM-102", cx).is_some());
        assert_eq!(view.read(cx).message(), "claude-code opus-5.5 took LLM-102");
    });
}

/// Opens LLM-104, which no agent holds, in the built-in editor.
fn edit_llm_104(handle: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("enter", cx);
        window.press("e", cx);
    })
    .unwrap();
}

fn set_editor_text(
    handle: AnyWindowHandle,
    view: &Entity<Workspace>,
    text: &str,
    cx: &mut TestAppContext,
) {
    cx.update_window(handle, |_, window, cx| {
        let editor = view.read(cx).editor(cx).clone();
        editor.update(cx, |s, cx| s.set_value(text, window, cx));
    })
    .unwrap();
}

fn llm_104(view: &Entity<Workspace>, cx: &mut TestAppContext) -> crate::model::data::Issue {
    cx.read(|cx| view.read(cx).issue_by_key("LLM-104", cx).clone())
}

/// An agent claims LLM-104 from elsewhere.
fn claim_llm_104(view: &Entity<Workspace>, cx: &mut TestAppContext) {
    let mut issue = llm_104(view, cx);
    issue.claim = Some(crate::model::demo::claude_claim("LLM-104"));
    view.update(cx, |this, cx| this.receive_issue(issue, cx));
}

fn release_llm_104(view: &Entity<Workspace>, cx: &mut TestAppContext) {
    let mut issue = llm_104(view, cx);
    issue.claim = None;
    view.update(cx, |this, cx| this.upsert_issue(issue, cx));
}

fn description_saved(view: &Entity<Workspace>, version: i64, cx: &mut TestAppContext) {
    view.update(cx, |this, cx| {
        this.apply_event(
            crate::model::backend::Event::WriteDone {
                key: "LLM-104".into(),
                kind: crate::model::backend::WriteKind::Description,
                version,
            },
            cx,
        );
    });
}

/// LLM-104: the toggle goes away while the task is locked, and comes back.
#[gpui_kit::test]
fn a_locked_task_hides_the_edit_preview_toggle(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    edit_llm_104(handle, cx);
    run_palette(handle, "q", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("mode-edit").is_some());
        assert!(window.try_find("mode-preview").is_some());
    })
    .unwrap();

    claim_llm_104(&view, cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("mode-edit").is_none());
        assert!(window.try_find("mode-preview").is_none());
    })
    .unwrap();

    release_llm_104(&view, cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("mode-edit").is_some());
        assert!(window.try_find("mode-preview").is_some());
    })
    .unwrap();
}

/// LLM-104: a claim closes the editor without a save, and `e` brings the
/// owner's text back beside the server's version after the claim ends.
#[gpui_kit::test]
fn a_claim_during_an_edit_keeps_the_text_and_shows_the_preview(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let (backend, mut requests) = crate::model::backend::Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    let original = llm_104(&view, cx).description;
    edit_llm_104(handle, cx);
    set_editor_text(handle, &view, "Owner text.\n", cx);

    claim_llm_104(&view, cx);
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        let this = view.read(cx);
        assert_eq!(this.detail(cx).unwrap().mode, DetailMode::Preview);
        assert!(this.detail_focused(window, cx), "the preview takes focus");
        assert!(this.message().contains("unsaved text is kept"));
    })
    .unwrap();
    assert!(sent_descriptions(&mut requests).is_empty());
    assert_eq!(llm_104(&view, cx).description, original);

    // The task stays locked, so e is refused and the text waits.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("e", cx);
        assert_eq!(view.read(cx).detail(cx).unwrap().mode, DetailMode::Preview);
    })
    .unwrap();

    release_llm_104(&view, cx);
    cx.read(|cx| assert!(view.read(cx).message().contains("Press e to get")));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("e", cx);
        assert_eq!(
            view.read(cx).editor(cx).read(cx).value().as_ref(),
            "Owner text.\n"
        );
    })
    .unwrap();
    assert!(in_conflict(&view, cx));
}

#[gpui_kit::test]
fn a_claim_during_an_edit_without_changes_keeps_no_text(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    edit_llm_104(handle, cx);

    // The agent's claim brings its own new description. The editor still
    // holds the old one, which is not an edit by the owner.
    let mut issue = llm_104(&view, cx);
    issue.claim = Some(crate::model::demo::claude_claim("LLM-104"));
    issue.description = "Agent text.\n".into();
    issue.version += 1;
    view.update(cx, |this, cx| this.receive_issue(issue, cx));
    cx.read(|cx| {
        let this = view.read(cx);
        assert_eq!(this.detail(cx).unwrap().mode, DetailMode::Preview);
        assert!(!this.message().contains("unsaved"), "{}", this.message());
    });

    release_llm_104(&view, cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("e", cx);
        assert_eq!(
            view.read(cx).editor(cx).read(cx).value().as_ref(),
            "Agent text.\n"
        );
    })
    .unwrap();
    assert!(!in_conflict(&view, cx));
}

/// LLM-104: the answer to an older save must not drop the newer text, and
/// a save that waits in the queue must not go out to the locked task.
#[gpui_kit::test]
fn a_claim_during_a_save_keeps_the_newest_text(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let (backend, mut requests) = crate::model::backend::Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    edit_llm_104(handle, cx);
    set_editor_text(handle, &view, "First.\n", cx);
    run_palette(handle, "w", cx);
    set_editor_text(handle, &view, "Second.\n", cx);
    run_palette(handle, "w", cx);
    set_editor_text(handle, &view, "Third.\n", cx);
    assert_eq!(sent_descriptions(&mut requests).len(), 1);

    // The claim waits for the write, so both land in one event.
    claim_llm_104(&view, cx);
    let version = llm_104(&view, cx).version + 1;
    description_saved(&view, version, cx);
    cx.read(|cx| {
        assert_eq!(view.read(cx).detail(cx).unwrap().mode, DetailMode::Preview);
    });
    assert!(sent_descriptions(&mut requests).is_empty());

    release_llm_104(&view, cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("e", cx);
        assert_eq!(
            view.read(cx).editor(cx).read(cx).value().as_ref(),
            "Third.\n"
        );
    })
    .unwrap();
    assert!(in_conflict(&view, cx));
}

/// The server refuses a description save as stale, then the app gets the
/// server's copy, as the backend fetches it after a refused write.
fn refuse_as_stale(view: &Entity<Workspace>, server_text: &str, cx: &mut TestAppContext) {
    let mut server = llm_104(view, cx);
    view.update(cx, |this, cx| {
        this.apply_event(
            crate::model::backend::Event::WriteFailed {
                key: "LLM-104".into(),
                kind: crate::model::backend::WriteKind::Description,
                message: "stale".into(),
                stale: true,
            },
            cx,
        );
    });
    let message = cx.read(|cx| view.read(cx).message().to_string());
    server.description = server_text.to_string().into();
    server.version += 1;
    view.update(cx, |this, cx| this.upsert_issue(server, cx));
    view.update(cx, |this, _| this.set_message(message));
}

fn in_conflict(view: &Entity<Workspace>, cx: &mut TestAppContext) -> bool {
    cx.read(|cx| view.read(cx).task_view().read(cx).conflict)
}

#[gpui_kit::test]
fn a_stale_save_opens_the_conflict_view_and_a_merge_builds_on_the_server(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let (backend, mut requests) = crate::model::backend::Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    let base = llm_104(&view, cx).version;
    edit_llm_104(handle, cx);
    set_editor_text(handle, &view, "Owner text.\n", cx);
    run_palette(handle, "w", cx);
    assert_eq!(
        sent_descriptions(&mut requests),
        [("Owner text.\n".into(), Some(base))]
    );

    // An agent changed the text first. The open editor turns into the
    // conflict view, with the owner's text still in it.
    refuse_as_stale(&view, "Agent text.\n", cx);
    assert!(in_conflict(&view, cx));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let this = view.read(cx);
        assert_eq!(this.editor(cx).read(cx).value().as_ref(), "Owner text.\n");
        assert!(this.message().contains(":mine"), "{}", this.message());
        assert_eq!(
            this.issue_by_key("LLM-104", cx).description.as_ref(),
            "Agent text.\n"
        );
    })
    .unwrap();

    // The owner merges by hand. Esc saves on top of the server's version,
    // so no force is needed.
    set_editor_text(handle, &view, "Owner text.\nAgent text.\n", cx);
    cx.update_window(handle, |_, window, cx| {
        window.press("escape", cx);
    })
    .unwrap();
    assert_eq!(
        sent_descriptions(&mut requests),
        [("Owner text.\nAgent text.\n".into(), Some(base + 1))]
    );
    assert!(!in_conflict(&view, cx));
    cx.read(|cx| {
        let detail = view.read(cx).detail(cx).unwrap();
        assert_eq!(detail.mode, DetailMode::Preview);
    });
}

#[gpui_kit::test]
fn keep_mine_overwrites_the_server_version(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let (backend, mut requests) = crate::model::backend::Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    edit_llm_104(handle, cx);
    set_editor_text(handle, &view, "Owner text.\n", cx);
    run_palette(handle, "w", cx);
    sent_descriptions(&mut requests);
    refuse_as_stale(&view, "Agent text.\n", cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("conflict-mine", cx);
    })
    .unwrap();
    // A forced save carries no base version.
    assert_eq!(
        sent_descriptions(&mut requests),
        [("Owner text.\n".into(), None)]
    );
    cx.read(|cx| {
        assert_eq!(view.read(cx).detail(cx).unwrap().mode, DetailMode::Preview);
    });
}

#[gpui_kit::test]
fn the_conflict_view_comes_back_on_e_and_theirs_drops_the_draft(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    edit_llm_104(handle, cx);
    set_editor_text(handle, &view, "Owner text.\n", cx);
    // :q saves and closes the editor, so the refusal finds no editor.
    run_palette(handle, "q", cx);
    refuse_as_stale(&view, "Agent text.\n", cx);
    cx.read(|cx| {
        let message = view.read(cx).message();
        assert!(message.contains("press e to compare"), "{message}");
    });

    // e brings back the owner's text, beside the server's.
    cx.update_window(handle, |_, window, cx| {
        window.press("e", cx);
        window.render_frame(cx);
        assert_eq!(
            view.read(cx).editor(cx).read(cx).value().as_ref(),
            "Owner text.\n"
        );
    })
    .unwrap();
    assert!(in_conflict(&view, cx));

    // Keep theirs drops the owner's text for good.
    cx.update_window(handle, |_, window, cx| {
        window.click("conflict-theirs", cx);
    })
    .unwrap();
    cx.read(|cx| {
        let this = view.read(cx);
        assert_eq!(this.detail(cx).unwrap().mode, DetailMode::Preview);
        assert_eq!(this.message(), "Kept the server's version of LLM-104");
        assert_eq!(
            this.issue_by_key("LLM-104", cx).description.as_ref(),
            "Agent text.\n"
        );
    });
    cx.update_window(handle, |_, window, cx| {
        window.press("e", cx);
        assert_eq!(
            view.read(cx).editor(cx).read(cx).value().as_ref(),
            "Agent text.\n"
        );
    })
    .unwrap();
    assert!(!in_conflict(&view, cx));
}

/// A window that keeps unsaved text in `dir`, as the real app does. A
/// second call on the same folder is the app after a restart.
fn open_with_drafts(
    dir: &std::path::Path,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<Workspace>) {
    let (handle, view) = open(cx);
    let drafts = crate::model::drafts::DraftStore::open(dir);
    view.update(cx, |this, _| this.set_drafts(drafts));
    (handle, view)
}

/// LLM-111: the text from a refused save is there after a restart, and a
/// save ends it.
#[gpui_kit::test]
fn a_refused_save_survives_a_restart_and_a_save_ends_it(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (handle, view) = open_with_drafts(dir.path(), cx);
    edit_llm_104(handle, cx);
    set_editor_text(handle, &view, "Owner text.\n", cx);
    run_palette(handle, "q", cx);
    refuse_as_stale(&view, "Agent text.\n", cx);

    let (handle, view) = open_with_drafts(dir.path(), cx);
    let (backend, _requests) = crate::model::backend::Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    edit_llm_104(handle, cx);
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).editor(cx).read(cx).value().as_ref(),
            "Owner text.\n"
        );
    });
    assert!(in_conflict(&view, cx));

    run_palette(handle, "w", cx);
    let version = llm_104(&view, cx).version + 1;
    description_saved(&view, version, cx);
    let after = crate::model::drafts::DraftStore::open(dir.path());
    assert!(!after.contains_key("LLM-104"));
}

/// LLM-111: the text a claim set aside is there after a restart, and
/// picking the server's version ends it.
#[gpui_kit::test]
fn text_kept_by_a_claim_survives_a_restart_and_theirs_ends_it(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (handle, view) = open_with_drafts(dir.path(), cx);
    edit_llm_104(handle, cx);
    set_editor_text(handle, &view, "Owner text.\n", cx);
    claim_llm_104(&view, cx);

    let (handle, view) = open_with_drafts(dir.path(), cx);
    edit_llm_104(handle, cx);
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).editor(cx).read(cx).value().as_ref(),
            "Owner text.\n"
        );
    });
    assert!(in_conflict(&view, cx));

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("conflict-theirs", cx);
    })
    .unwrap();
    let after = crate::model::drafts::DraftStore::open(dir.path());
    assert!(!after.contains_key("LLM-104"));
}

/// Moves the file's change time on, so a check sees a new write even when
/// two writes land in the same clock tick.
fn touch_later(path: &std::path::Path, text: &str, step: u64) {
    std::fs::write(path, text).unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(step);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(later)
        .unwrap();
}

#[gpui_kit::test]
fn shift_e_opens_editor_and_saves_each_write(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let (backend, mut requests) = crate::model::backend::Backend::detached();
    let dir = tempfile::tempdir().unwrap();
    let launched = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = launched.clone();
    view.update(cx, |this, _| {
        this.attach_backend(backend);
        this.set_external(
            dir.path().to_path_buf(),
            std::sync::Arc::new(move |path: &std::path::Path| {
                seen.lock().unwrap().push(path.to_path_buf());
                Ok(())
            }),
        );
    });
    let issue = llm_104(&view, cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("shift-e", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let path = dir.path().join("LLM-104.md");
    assert_eq!(*launched.lock().unwrap(), std::slice::from_ref(&path));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        issue.description.as_ref()
    );
    cx.read(|cx| {
        let message = view.read(cx).message();
        assert!(message.contains("Each write saves it"), "{message}");
    });

    // Nothing written yet, so nothing saves.
    view.update(cx, |this, cx| this.poll_external(cx));
    assert!(sent_descriptions(&mut requests).is_empty());

    // The editor writes the file: the text goes out on the version it
    // started from, and the list shows it at once.
    touch_later(&path, "From nvim.\n", 1);
    view.update(cx, |this, cx| this.poll_external(cx));
    assert_eq!(
        sent_descriptions(&mut requests),
        [("From nvim.\n".into(), Some(issue.version))]
    );
    assert_eq!(llm_104(&view, cx).description.as_ref(), "From nvim.\n");

    // A write that changes nothing saves nothing.
    touch_later(&path, "From nvim.\n", 2);
    view.update(cx, |this, cx| this.poll_external(cx));
    assert!(sent_descriptions(&mut requests).is_empty());

    // After the server answers, the next write builds on the new version.
    view.update(cx, |this, cx| {
        this.apply_event(
            crate::model::backend::Event::WriteDone {
                key: "LLM-104".into(),
                kind: crate::model::backend::WriteKind::Description,
                version: issue.version + 1,
            },
            cx,
        );
    });
    touch_later(&path, "From nvim, again.\n", 3);
    view.update(cx, |this, cx| this.poll_external(cx));
    assert_eq!(
        sent_descriptions(&mut requests),
        [("From nvim, again.\n".into(), Some(issue.version + 1))]
    );
}

/// LLM-170: an idle check draws nothing, the editor's exit ends the edit
/// and deletes its file, and the check stops with the last edit.
#[gpui_kit::test]
fn the_editor_exit_ends_the_edit_and_the_checks(cx: &mut TestAppContext) {
    use crate::model::external_editor::done_marker;
    let (handle, view) = open(cx);
    let dir = tempfile::tempdir().unwrap();
    view.update(cx, |this, _| {
        this.set_external(
            dir.path().to_path_buf(),
            std::sync::Arc::new(|_: &std::path::Path| Ok(())),
        );
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("shift-e", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let path = dir.path().join("LLM-104.md");
    assert!(path.exists());
    assert!(cx.read(|cx| view.read(cx).external_polling()));

    // Idle checks notify nobody.
    let notified = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = notified.clone();
    let _watch = cx.update(|cx| cx.observe(&view, move |_, _| count.set(count.get() + 1)));
    for _ in 0..5 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(500));
        cx.run_until_parked();
    }
    assert_eq!(notified.get(), 0);
    assert!(cx.read(|cx| view.read(cx).external_polling()));

    // The owner quits the editor: the shell touches the marker.
    touch_later(&done_marker(&path), "", 10);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(500));
    cx.run_until_parked();
    assert!(notified.get() > 0);
    assert!(!path.exists());
    assert!(!done_marker(&path).exists());
    cx.read(|cx| {
        let this = view.read(cx);
        assert!(!this.external_polling());
        assert_eq!(this.message(), "Closed LLM-104 in $EDITOR");
    });

    // A later Shift+E starts the checks again.
    cx.update_window(handle, |_, window, cx| window.press("shift-e", cx))
        .unwrap();
    cx.run_until_parked();
    assert!(cx.read(|cx| view.read(cx).external_polling()));
}

#[gpui_kit::test]
fn shift_e_refuses_a_locked_task(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let dir = tempfile::tempdir().unwrap();
    view.update(cx, |this, _| {
        this.set_external(
            dir.path().to_path_buf(),
            std::sync::Arc::new(|_: &std::path::Path| panic!("a locked task must not open")),
        );
    });
    let locked = cx.read(|cx| {
        let this = view.read(cx);
        (0..this.visible_count(cx))
            .find(|&n| this.visible_issue(n, cx).claim.is_some())
            .expect("the demo has a claimed task")
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        for _ in 0..locked {
            window.press("j", cx);
        }
        window.press("shift-e", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    cx.read(|cx| assert!(view.read(cx).message().contains("is locked")));
}

#[gpui_kit::test]
fn statuses_and_labels_come_from_the_server(cx: &mut TestAppContext) {
    use llmtodo_client::types::{Issue, Label, Team};
    let (_, view) = open(cx);
    let team: Team = serde_json::from_value(serde_json::json!({
        "id": "01a1c453-1260-7779-be1c-619faeb9d420", "key": "ALX", "name": "alx",
        "created_at": "2026-11-01T10:00:00Z",
        "statuses": [
            {"id": "01a1c453-1260-7779-be1c-619faeb9d421", "team": "ALX", "name": "Todo", "type": "unstarted", "color": "#fff", "position": 1},
            {"id": "01a1c453-1260-7779-be1c-619faeb9d422", "team": "ALX", "name": "In Review", "type": "started", "color": "#fff", "position": 3},
            {"id": "01a1c453-1260-7779-be1c-619faeb9d423", "team": "ALX", "name": "Duplicate", "type": "canceled", "color": "#fff", "position": 6}
        ]
    }))
    .unwrap();
    let label: Label = serde_json::from_value(serde_json::json!({
        "id": "01a1c453-1260-7779-be1c-619faeb9d424", "name": "fe", "group": "Area",
        "full_name": "Area/fe", "color": "#eb5757", "description": null, "is_group": false
    }))
    .unwrap();
    let issue = |n: u32, status: &str, kind: &str| -> Issue {
        serde_json::from_value(serde_json::json!({
            "id": format!("01a1c453-1260-7779-be1c-619faeb9d5{n:02}"),
            "key": format!("ALX-{n}"), "team": "ALX", "title": format!("Issue {n}"),
            "status": {"id": "01a1c453-1260-7779-be1c-619faeb9d421", "name": status, "type": kind, "color": "#fff"},
            "priority": "none", "assignee": null, "labels": ["Area/fe"], "project": null,
            "milestone": null, "parent": null, "wanted_executor": {}, "description": "Body",
            "description_version": 3, "claim": null, "created_by": null,
            "created_at": "2026-11-02T10:00:00Z", "updated_at": "2026-11-02T10:00:00Z",
            "completed_at": null, "deleted_at": null
        }))
        .unwrap()
    };
    view.update(cx, |this, cx| {
        this.apply_event(
            crate::model::backend::Event::Snapshot(Box::new(
                crate::model::backend::Snapshot {
                    catalog: crate::model::backend::CatalogData {
                        teams: vec![team],
                        labels: vec![label],
                        ..Default::default()
                    },
                    questions: Vec::new(),
                    issues: vec![
                        issue(9, "Duplicate", "canceled"),
                        issue(10, "In Review", "started"),
                        issue(2, "Todo", "unstarted"),
                    ],
                }
                .load(),
            )),
            cx,
        );
        // Keys sort by number, not as text.
        let keys: Vec<_> = (0..3)
            .map(|n| this.visible_issue(n, cx).key.to_string())
            .collect();
        assert_eq!(keys, ["ALX-2", "ALX-9", "ALX-10"]);
        let review = this.visible_issue(2, cx);
        assert_eq!(review.status.name, "In Review");
        assert_eq!(review.version, 3);
        assert_eq!(review.labels[0].color, crate::model::data::LabelColor::Red);
        assert!(this.visible_issue(1, cx).status.is_closed());
    });
    run_palette_on(&view, "group status", cx);
    cx.read(|cx| {
        let headers: Vec<_> = view
            .read(cx)
            .rows(cx)
            .iter()
            .filter_map(|r| match r {
                Row::Header { title, .. } => Some(title.to_string()),
                _ => None,
            })
            .collect();
        // Active work first, closed last, real names.
        assert_eq!(headers, ["In Review", "Todo", "Duplicate"]);
    });
}

fn run_palette_on(view: &Entity<Workspace>, command: &str, cx: &mut TestAppContext) {
    let command = command.to_string();
    let view = view.clone();
    cx.update(|cx| {
        let window = cx.windows()[0];
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |this, cx| this.run_command(&command, window, cx))
            })
            .unwrap();
    });
}

#[gpui_kit::test]
fn the_task_view_keeps_its_size_while_moving_through_tasks(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
        window.render_frame(cx);
        let first = view.read(cx).detail_size(cx).expect("task view laid out");
        // LLM-102 has a long description and a claim bar; the others differ.
        for _ in 0..6 {
            window.press("j", cx);
            window.render_frame(cx);
            assert_eq!(view.read(cx).detail_size(cx), Some(first));
        }
        // The list keeps at least its minimum width.
        let rem = window.rem_size();
        assert!(window.viewport_size().width - first >= rem * 30.);
    })
    .unwrap();
}

/// Reloads the list in a new order, as a snapshot from the server does.
/// `keep` picks the issues the new snapshot still holds.
fn reload_reordered(
    view: &Entity<Workspace>,
    keep: impl Fn(&crate::model::data::Issue) -> bool,
    cx: &mut TestAppContext,
) {
    view.update(cx, |this, cx| {
        let (mut issues, catalog) = this.issues_and_catalog(cx);
        issues.retain(|i| keep(i));
        issues.reverse();
        this.load(catalog, issues, cx);
    });
}

fn row_of(view: &Entity<Workspace>, key: &str, cx: &mut TestAppContext) -> usize {
    cx.read(|cx| {
        let this = view.read(cx);
        this.rows(cx)
            .iter()
            .position(|r| matches!(r, Row::Issue(ix) if this.issue(*ix, cx).key.as_ref() == key))
            .expect("row for key")
    })
}

fn deleted_keys(view: &Entity<Workspace>, cx: &mut TestAppContext) -> Vec<String> {
    cx.read(|cx| {
        let (issues, _) = view.read(cx).issues_and_catalog(cx);
        issues
            .iter()
            .filter(|i| i.deleted)
            .map(|i| i.key.to_string())
            .collect()
    })
}

/// The index of "Move to Trash" in the task menu, separators included.
pub(crate) const TRASH_ITEM: usize = 7;

#[gpui_kit::test]
fn the_task_menu_acts_on_its_task_after_a_snapshot_reorders_the_list(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let row = row_of(&view, "LLM-101", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.right_click(("row", row), cx);
        window.render_frame(cx);
    })
    .unwrap();

    // While the menu is open, a snapshot puts the issues in a new order.
    reload_reordered(&view, |_| true, cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.within("popup-menu").click(TRASH_ITEM, cx);
    })
    .unwrap();
    assert_eq!(deleted_keys(&view, cx), ["LLM-101"]);
}

#[gpui_kit::test]
fn the_task_menu_does_nothing_when_its_task_is_gone(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let row = row_of(&view, "LLM-101", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.right_click(("row", row), cx);
        window.render_frame(cx);
    })
    .unwrap();

    // The new snapshot is shorter and no longer holds LLM-101.
    reload_reordered(
        &view,
        |i| i.key.as_ref() != "LLM-101" && i.key.as_ref() < "LLM-105",
        cx,
    );

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.within("popup-menu").click(TRASH_ITEM, cx);
    })
    .unwrap();
    assert!(deleted_keys(&view, cx).is_empty());
}

#[gpui_kit::test]
fn a_save_that_fails_offline_keeps_the_owners_text(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let text = |cx: &mut gpui_kit::App| view.read(cx).editor(cx).read(cx).value().to_string();
    // The server's copy of LLM-104, from before the owner's edit.
    let server_copy = cx.read(|cx| {
        let (issues, _) = view.read(cx).issues_and_catalog(cx);
        issues
            .into_iter()
            .find(|i| i.key.as_ref() == "LLM-104")
            .expect("LLM-104")
    });

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // LLM-104 is free to edit.
        window.press("j", cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("enter", cx);
        window.press("e", cx);
        let editor = view.read(cx).editor(cx).clone();
        editor.update(cx, |s, cx| s.set_value("Owner text.\n", window, cx));
    })
    .unwrap();
    run_palette(handle, "q", cx);

    // The server restarts, so the save fails. On reconnect the app fetches
    // the issue again and gets the old text back.
    view.update(cx, |this, cx| {
        this.apply_event(
            crate::model::backend::Event::WriteFailed {
                key: "LLM-104".into(),
                kind: crate::model::backend::WriteKind::Description,
                message: "the server is offline, so nothing was saved".into(),
                stale: false,
            },
            cx,
        );
        assert!(this.message().contains("press e"), "{}", this.message());
        this.upsert_issue(server_copy, cx);
    });

    // E opens the editor with the owner's text, and :w sends it again.
    cx.update_window(handle, |_, window, cx| {
        window.press("e", cx);
        assert_eq!(text(cx), "Owner text.\n");
    })
    .unwrap();
    run_palette(handle, "w", cx);
    cx.read(|cx| {
        let this = view.read(cx);
        assert_eq!(this.message(), "Saved LLM-104");
        let (issues, _) = this.issues_and_catalog(cx);
        let issue = issues.iter().find(|i| i.key.as_ref() == "LLM-104").unwrap();
        assert_eq!(issue.description.as_ref(), "Owner text.\n");
    });
}

/// The description saves the app sent, as body and base version.
fn sent_descriptions(
    requests: &mut tokio::sync::mpsc::UnboundedReceiver<crate::model::backend::Request>,
) -> Vec<(String, Option<i64>)> {
    let mut out = Vec::new();
    while let Ok(request) = requests.try_recv() {
        if let crate::model::backend::Request::Write {
            write: crate::model::backend::Write::Update(update),
            kind: crate::model::backend::WriteKind::Description,
            ..
        } = request
        {
            out.push((
                update.description.body.clone().unwrap_or_default(),
                update.description.base_version,
            ));
        }
    }
    out
}

#[gpui_kit::test]
fn a_second_save_waits_for_the_first_and_builds_on_it(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let (backend, mut requests) = crate::model::backend::Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    let base = cx.read(|cx| {
        let (issues, _) = view.read(cx).issues_and_catalog(cx);
        issues
            .into_iter()
            .find(|i| i.key.as_ref() == "LLM-104")
            .unwrap()
            .version
    });
    let set_text = |text: &'static str, cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            let editor = view.read(cx).editor(cx).clone();
            editor.update(cx, |s, cx| s.set_value(text, window, cx));
        })
        .unwrap();
    };

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // LLM-104 is free to edit.
        window.press("j", cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("enter", cx);
        window.press("e", cx);
    })
    .unwrap();

    // Two saves, the second before the server answers the first.
    set_text("One.\n", cx);
    run_palette(handle, "w", cx);
    set_text("One. Two.\n", cx);
    run_palette(handle, "w", cx);
    assert_eq!(
        sent_descriptions(&mut requests),
        [("One.\n".into(), Some(base))]
    );

    // The server accepts the first save. The second goes out on top of it.
    view.update(cx, |this, cx| {
        this.apply_event(
            crate::model::backend::Event::WriteDone {
                key: "LLM-104".into(),
                kind: crate::model::backend::WriteKind::Description,
                version: base + 1,
            },
            cx,
        );
    });
    assert_eq!(
        sent_descriptions(&mut requests),
        [("One. Two.\n".into(), Some(base + 1))]
    );

    // The server answers the second save, and nothing more goes out.
    view.update(cx, |this, cx| {
        this.apply_event(
            crate::model::backend::Event::WriteDone {
                key: "LLM-104".into(),
                kind: crate::model::backend::WriteKind::Description,
                version: base + 2,
            },
            cx,
        );
    });
    assert!(sent_descriptions(&mut requests).is_empty());
}

#[gpui_kit::test]
fn a_snapshot_without_the_task_being_edited_keeps_the_editor(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let (backend, mut requests) = crate::model::backend::Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // LLM-104 is free to edit.
        window.press("j", cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("enter", cx);
        window.press("e", cx);
        let editor = view.read(cx).editor(cx).clone();
        editor.update(cx, |s, cx| s.set_value("Owner text.\n", window, cx));
    })
    .unwrap();
    // Opening the task fetched its git panel; only the snapshot's fetch counts.
    while requests.try_recv().is_ok() {}

    // An agent moves LLM-104 to the Trash, so the next snapshot leaves it out.
    reload_reordered(&view, |i| i.key.as_ref() != "LLM-104", cx);

    cx.read(|cx| {
        let this = view.read(cx);
        let detail = this.detail(cx).expect("the editor stays open");
        assert_eq!(detail.mode, DetailMode::Editing);
        assert_eq!(this.issue(detail.issue, cx).key.as_ref(), "LLM-104");
        assert_eq!(this.editor(cx).read(cx).value().as_ref(), "Owner text.\n");
    });
    // The app asks the server for the issue, which still returns trashed ones.
    let fetched: Vec<String> = std::iter::from_fn(|| requests.try_recv().ok())
        .filter_map(|r| match r {
            crate::model::backend::Request::Fetch { key } => Some(key),
            _ => None,
        })
        .collect();
    assert_eq!(fetched, ["LLM-104"]);
}

#[gpui_kit::test]
fn colon_r_and_enter_reloads_and_never_releases_a_claim(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    // LLM-102 starts claimed by Claude.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("j", cx);
    })
    .unwrap();
    cx.read(|cx| assert!(view.read(cx).claim_of("LLM-102", cx).is_some()));

    // `:r` runs reload, the first suggestion.
    run_palette(handle, "r", cx);
    cx.read(|cx| {
        assert!(view.read(cx).claim_of("LLM-102", cx).is_some());
        assert_eq!(view.read(cx).message(), "No server to reload from");
    });

    // A part of the word is not enough to end an agent's claim.
    run_palette(handle, "rel", cx);
    cx.read(|cx| assert!(view.read(cx).claim_of("LLM-102", cx).is_some()));

    // The whole word still works.
    run_palette(handle, "release", cx);
    cx.read(|cx| assert!(view.read(cx).claim_of("LLM-102", cx).is_none()));
}

/// A catalog event carries no issues. The app maps the issues it holds
/// to the new names, keeps their text, and fetches nothing.
#[gpui_kit::test]
fn a_catalog_event_renames_held_issues_without_a_fetch(cx: &mut TestAppContext) {
    use llmtodo_client::types::{Issue, Label, Team};
    let (_, view) = open(cx);
    let (backend, mut requests) = crate::model::backend::Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    let team = |status: &str| -> Team {
        serde_json::from_value(serde_json::json!({
            "id": "01a1c453-1260-7779-be1c-619faeb9d420", "key": "ALX", "name": "alx",
            "created_at": "2026-11-01T10:00:00Z",
            "statuses": [
                {"id": "01a1c453-1260-7779-be1c-619faeb9d421", "team": "ALX", "name": status, "type": "unstarted", "color": "#fff", "position": 1}
            ]
        }))
        .unwrap()
    };
    let label = |name: &str| -> Label {
        serde_json::from_value(serde_json::json!({
            "id": "01a1c453-1260-7779-be1c-619faeb9d424", "name": name, "group": null,
            "full_name": name, "color": "#eb5757", "description": null, "is_group": false
        }))
        .unwrap()
    };
    let issue = |n: u32, description: Option<&str>, version: i64| -> Issue {
        serde_json::from_value(serde_json::json!({
            "id": format!("01a1c453-1260-7779-be1c-619faeb9d5{n:02}"),
            "key": format!("ALX-{n}"), "team": "ALX", "title": format!("Issue {n}"),
            "status": {"id": "01a1c453-1260-7779-be1c-619faeb9d421", "name": "Todo", "type": "unstarted", "color": "#fff"},
            "priority": "none", "assignee": null, "labels": ["fe"], "project": null,
            "milestone": null, "parent": null, "wanted_executor": {}, "description": description,
            "description_version": version, "claim": null, "created_by": null,
            "created_at": "2026-11-02T10:00:00Z", "updated_at": "2026-11-02T10:00:00Z",
            "completed_at": null, "deleted_at": null
        }))
        .unwrap()
    };
    let snapshot = |issues: Vec<Issue>| {
        crate::model::backend::Event::Snapshot(Box::new(
            crate::model::backend::Snapshot {
                catalog: crate::model::backend::CatalogData {
                    teams: vec![team("Todo")],
                    labels: vec![label("fe")],
                    ..Default::default()
                },
                issues,
                questions: Vec::new(),
            }
            .load(),
        ))
    };
    let by_key = |this: &Workspace, k: &str, cx: &gpui_kit::App| this.issue_by_key(k, cx).clone();
    view.update(cx, |this, cx| {
        this.apply_event(snapshot(vec![issue(1, None, 2), issue(2, None, 2)]), cx);
        // A single-issue fetch brings the text.
        this.apply_event(
            crate::model::backend::Event::Issue(Box::new(issue(1, Some("One"), 2))),
            cx,
        );
    });
    while requests.try_recv().is_ok() {}

    // The owner renames the status and the label.
    view.update(cx, |this, cx| {
        this.apply_event(
            crate::model::backend::Event::Catalog(Box::new(crate::model::backend::CatalogData {
                teams: vec![team("Ready")],
                labels: vec![label("frontend")],
                ..Default::default()
            })),
            cx,
        );
        let one = by_key(this, "ALX-1", cx);
        assert_eq!(one.status.name, "Ready");
        assert_eq!(one.labels[0].name, "frontend");
        assert_eq!(
            (one.description.as_ref(), one.description_loaded),
            ("One", true)
        );
        assert!(!by_key(this, "ALX-2", cx).description_loaded);
    });
    assert!(
        std::iter::from_fn(|| requests.try_recv().ok())
            .next()
            .is_none(),
        "a catalog change fetches no issue"
    );

    // A snapshot keeps held text at the same version and drops it at a
    // newer one.
    view.update(cx, |this, cx| {
        this.apply_event(snapshot(vec![issue(1, None, 2)]), cx);
        assert_eq!(by_key(this, "ALX-1", cx).description.as_ref(), "One");
        this.apply_event(snapshot(vec![issue(1, None, 3)]), cx);
        let one = by_key(this, "ALX-1", cx);
        assert!(!one.description_loaded);
        assert_eq!(one.version, 3);
    });
}

#[gpui_kit::test]
fn a_team_move_replaces_the_key_in_place(cx: &mut TestAppContext) {
    let (_handle, view) = open(cx);
    let count = cx.read(|cx| view.read(cx).visible_count(cx));
    // An agent moves LLM-101 to team ALX, so it comes back as ALX-3.
    view.update(cx, |this, cx| {
        let mut issue = this.issue_by_key("LLM-101", cx).clone();
        issue.key = "ALX-3".into();
        issue.team = "ALX".into();
        this.upsert_issue(issue, cx);
    });
    cx.read(|cx| {
        let this = view.read(cx);
        assert_eq!(this.visible_count(cx), count, "no ghost row");
        let keys: Vec<String> = (0..this.visible_count(cx))
            .map(|n| this.visible_issue(n, cx).key.to_string())
            .collect();
        assert!(keys.contains(&"ALX-3".to_string()));
        assert!(!keys.contains(&"LLM-101".to_string()));
    });
}

#[gpui_kit::test]
fn an_earlier_answer_does_not_undo_a_newer_change(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let (backend, _requests) = crate::model::backend::Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    // LLM-101 is free to edit and has no `bug` label.
    let before = cx.read(|cx| view.read(cx).issue_by_key("LLM-101", cx).clone());
    let has_bug = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let issue = view.read(cx).issue_by_key("LLM-101", cx);
            issue.labels.iter().any(|l| l.name == "bug")
        })
    };

    // The owner moves LLM-101 to Done and adds `bug` right away.
    run_palette(handle, "status done", cx);
    run_palette(handle, "label bug", cx);
    assert!(has_bug(cx));

    // The answer to the status write comes first. It has no `bug` yet.
    let mut first = before.clone();
    first.status = cx.read(|cx| view.read(cx).issue_by_key("LLM-101", cx).status.clone());
    view.update(cx, |this, cx| {
        this.receive_issue(first.clone(), cx);
        this.apply_event(
            crate::model::backend::Event::WriteDone {
                key: "LLM-101".into(),
                kind: crate::model::backend::WriteKind::Status,
                version: first.version,
            },
            cx,
        );
    });
    assert!(
        has_bug(cx),
        "the tag stays while the label write is on its way"
    );
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).issue_by_key("LLM-101", cx).status.name,
            "Done"
        )
    });

    // The label write's answer has both changes, and it lands.
    let mut second = first.clone();
    second.labels = cx.read(|cx| view.read(cx).issue_by_key("LLM-101", cx).labels.clone());
    second.title = "Server title".into();
    view.update(cx, |this, cx| {
        this.receive_issue(second, cx);
        this.apply_event(
            crate::model::backend::Event::WriteDone {
                key: "LLM-101".into(),
                kind: crate::model::backend::WriteKind::Labels,
                version: first.version,
            },
            cx,
        );
    });
    cx.read(|cx| {
        let issue = view.read(cx).issue_by_key("LLM-101", cx);
        assert_eq!(issue.title.as_ref(), "Server title");
        assert!(issue.labels.iter().any(|l| l.name == "bug"));
    });

    // With no write on its way, a live copy applies at once.
    let mut live = before;
    live.title = "Agent title".into();
    view.update(cx, |this, cx| this.receive_issue(live, cx));
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).issue_by_key("LLM-101", cx).title.as_ref(),
            "Agent title"
        )
    });
}

#[gpui_kit::test]
fn restore_works_after_a_snapshot_left_out_the_trashed_task(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let (backend, mut requests) = crate::model::backend::Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    let trashed = cx.read(|cx| view.read(cx).issue_by_key("LLM-101", cx).clone());
    let count = cx.read(|cx| view.read(cx).visible_count(cx));

    run_palette(handle, "delete", cx);
    view.update(cx, |this, cx| {
        this.apply_event(
            crate::model::backend::Event::WriteDone {
                key: "LLM-101".into(),
                kind: crate::model::backend::WriteKind::Trash,
                version: 1,
            },
            cx,
        )
    });
    // An agent's change triggers a snapshot, which leaves out the Trash.
    reload_reordered(&view, |i| !i.deleted, cx);
    while requests.try_recv().is_ok() {}

    run_palette(handle, "restore", cx);
    let restores: Vec<String> = std::iter::from_fn(|| requests.try_recv().ok())
        .filter_map(|r| match r {
            crate::model::backend::Request::Write {
                key,
                write: crate::model::backend::Write::Restore,
                ..
            } => Some(key),
            _ => None,
        })
        .collect();
    assert_eq!(restores, ["LLM-101"]);

    // The server's answer adds the task back.
    view.update(cx, |this, cx| {
        this.receive_issue(trashed, cx);
        this.apply_event(
            crate::model::backend::Event::WriteDone {
                key: "LLM-101".into(),
                kind: crate::model::backend::WriteKind::Restore,
                version: 1,
            },
            cx,
        );
    });
    cx.read(|cx| {
        assert_eq!(view.read(cx).visible_count(cx), count);
        assert!(!view.read(cx).issue_by_key("LLM-101", cx).deleted);
    });
}

/// A workspace on demo data that saves its settings to `path`.
fn open_with_settings(
    path: &std::path::Path,
    settings: crate::settings::UiSettings,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<Workspace>) {
    let store = crate::settings::SettingsStore::at(path.to_path_buf());
    cx.update(crate::init);
    cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
            cx.new(|cx| {
                let mut this = Workspace::with_settings(settings, store, window, cx);
                this.load_demo(cx);
                this
            })
        })
        .unwrap()
    })
}

#[gpui_kit::test]
fn a_window_drag_saves_the_frame_once_it_stops(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ui.json");
    let store = crate::settings::SettingsStore::at(path.clone());
    let (_, view) = open_with_settings(&path, Default::default(), cx);
    cx.run_until_parked();
    let writes_before = std::fs::metadata(&path).ok().map(|m| m.modified().unwrap());

    // A drag of the window edge moves it every frame.
    view.update(cx, |this, cx| {
        for x in 0..50 {
            this.window_moved([x as f32, 0., 900., 700.], cx);
        }
    });
    assert_ne!(
        store.load().window,
        Some([49., 0., 900., 700.]),
        "no file write while the drag goes on"
    );
    assert_eq!(
        std::fs::metadata(&path).ok().map(|m| m.modified().unwrap()),
        writes_before
    );

    // The drag ends, and the last frame is saved.
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(store.load().window, Some([49., 0., 900., 700.]));
}

#[gpui_kit::test]
fn saved_tabs_drop_tasks_that_are_gone_and_first_tabs(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ui.json");
    let settings = crate::settings::UiSettings {
        open_tabs: [("LLM-999".to_string(), 2), ("LLM-101".to_string(), 1)]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    let (handle, view) = open_with_settings(&path, settings, cx);
    let tabs = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let tabs = view.read(cx).current_settings(cx).open_tabs;
            tabs.into_keys().collect::<Vec<_>>()
        })
    };
    // LLM-999 is not in the data, so its entry goes.
    assert_eq!(tabs(cx), ["LLM-101"]);

    // Back on the first tab, the entry goes too: missing means the first.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    run_palette(handle, "tab 1", cx);
    assert!(tabs(cx).is_empty());
}

/// The open task shows its git panel once the server sends it, with the
/// branch, the PR and a toggle past the third commit.
#[gpui_kit::test]
fn the_git_panel_shows_branch_pr_and_commits(cx: &mut TestAppContext) {
    use llmtodo_api_types::{GitPr, IssueGit};

    let (handle, view) = open(cx);
    let id = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click(("row", 3usize), cx);
            window.render_frame(cx);
            // Nothing to show before the server sends the panel.
            assert!(window.try_find("git-panel").is_none());
            view.read(cx).detail(cx).unwrap().issue
        })
        .unwrap();

    let git = IssueGit {
        branch: Some("fix/git-panel".into()),
        worktree: Some("/work/llmtodo-git".into()),
        host: Some("laptop".into()),
        pr: Some(GitPr {
            url: "https://github.com/o/r/pull/7".into(),
            title: Some("#7 Git panel".into()),
            number: Some(7),
            state: Some("open".into()),
        }),
        head: None,
        commits: (1..=5).map(|n| format!("abc000{n} commit {n}")).collect(),
        repo_url: None,
    };
    view.update(cx, |this, cx| {
        this.store().update(cx, |store, cx| {
            store.set_git(id, git);
            cx.notify();
        })
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("git-panel").is_some());
        let task = view.read(cx).task_view().clone();
        assert!(!task.read(cx).git_expanded());
        window.click("git-more", cx);
        window.render_frame(cx);
        assert!(task.read(cx).git_expanded());
    })
    .unwrap();
}

/// A long commit list, open on a short window, keeps the task view usable:
/// the toggle sits in the panel header within the task view, the preview
/// keeps room to scroll, and a second click closes the list.
#[gpui_kit::test]
fn an_open_commit_list_keeps_its_toggle_and_the_preview_in_reach(cx: &mut TestAppContext) {
    use llmtodo_api_types::IssueGit;

    let (handle, view) = open_sized(cx, 1200., 480.);
    let id = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click(("row", 3usize), cx);
            window.render_frame(cx);
            view.read(cx).detail(cx).unwrap().issue
        })
        .unwrap();
    let git = IssueGit {
        branch: Some("fix/long".into()),
        commits: (1..=20).map(|n| format!("abc{n:04} commit {n}")).collect(),
        ..Default::default()
    };
    view.update(cx, |this, cx| {
        this.store().update(cx, |store, cx| {
            store.set_git(id, git);
            cx.notify();
        })
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("git-more", cx);
        window.render_frame(cx);
        let task = view.read(cx).task_view().clone();
        assert!(task.read(cx).git_expanded());
        let bottom = window.viewport_size().height;
        let toggle = window.find("git-more").bounds();
        let panel = window.find("git-panel").bounds();
        let preview = window.find("preview").bounds();
        assert!(
            toggle.bottom() <= bottom,
            "toggle {toggle:?} below the window bottom {bottom:?}"
        );
        // The toggle is pinned above the commits, in the panel's top half.
        assert!(toggle.top() < panel.top() + panel.size.height / 2.);
        assert!(
            preview.size.height >= px(80.),
            "the preview kept only {preview:?}"
        );
        window.click("git-more", cx);
        window.render_frame(cx);
        assert!(!task.read(cx).git_expanded());
    })
    .unwrap();
}

/// With a known remote, each commit hash links to its commit page, and a
/// click opens it in the browser. With no remote, the hash stays text.
#[gpui_kit::test]
fn commit_hashes_link_to_the_remote(cx: &mut TestAppContext) {
    use llmtodo_api_types::IssueGit;

    let (handle, view) = open(cx);
    // Row 3 in the demo list: find one with no linked project repo, so only
    // the worktree's remote counts.
    let id = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click(("row", 3usize), cx);
            window.render_frame(cx);
            view.read(cx).detail(cx).unwrap().issue
        })
        .unwrap();
    let project_repo = cx.read(|cx| {
        let store = view.read(cx).store().read(cx);
        let project = store.get(id).and_then(|i| i.project.clone());
        project.and_then(|p| store.catalog.project_repo(&p).map(str::to_string))
    });
    let send = |repo_url: Option<&str>, cx: &mut TestAppContext| {
        let git = IssueGit {
            commits: vec!["4c87f0c send the hash".into(), "nothash subject".into()],
            repo_url: repo_url.map(Into::into),
            ..Default::default()
        };
        view.update(cx, |this, cx| {
            this.store().update(cx, |store, cx| {
                store.set_git(id, git);
                cx.notify();
            })
        });
    };
    send(Some("https://github.com/o/r"), cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // A line that does not start with a hash gets no link.
        assert!(window.try_find(("git-commit", 1usize)).is_none());
        window.click(("git-commit", 0usize), cx);
        window.render_frame(cx);
    })
    .unwrap();
    let base = project_repo
        .clone()
        .unwrap_or("https://github.com/o/r".into());
    assert_eq!(cx.opened_url(), Some(format!("{base}/commit/4c87f0c")));

    send(None, cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.try_find(("git-commit", 0usize)).is_some(),
            project_repo.is_some()
        );
    })
    .unwrap();
}

/// Sends one mouse event to the window and draws a frame.
pub fn mouse(
    window: &mut gpui_kit::Window,
    event: impl gpui_kit::InputEvent,
    cx: &mut gpui_kit::App,
) {
    window.dispatch_event(event.to_platform_input(), cx);
    window.render_frame(cx);
}

#[gpui_kit::test]
fn a_drag_drops_its_task_after_a_snapshot_shifts_the_rows(cx: &mut TestAppContext) {
    use gpui_kit::{MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, point, px};
    let (handle, view) = open(cx);
    run_palette(handle, "group status", cx);
    let done_header = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            view.read(cx)
                .rows(cx)
                .iter()
                .position(|r| matches!(r, Row::Header { title, .. } if title.as_ref() == "Done"))
                .expect("Done header")
        })
    };
    // The task just above the Done header is in an open status.
    let from_row = done_header(cx) - 1;
    let dragged = cx.read(|cx| match view.read(cx).rows(cx)[from_row] {
        Row::Issue(id) => view.read(cx).issue(id, cx).clone(),
        Row::Header { .. } => panic!("an issue row above the Done header"),
    });
    assert_ne!(dragged.status.name, "Done");
    assert_ne!(dragged.key.as_ref(), "LLM-101");
    let left = Some(MouseButton::Left);

    // The owner picks it up.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let from = window.find(("row", from_row)).bounds().center();
        let modifiers = Default::default();
        mouse(
            window,
            MouseMoveEvent {
                position: from,
                pressed_button: None,
                modifiers,
            },
            cx,
        );
        mouse(
            window,
            MouseDownEvent {
                button: MouseButton::Left,
                position: from,
                modifiers,
                click_count: 1,
                first_mouse: false,
            },
            cx,
        );
        for dy in [4., 8., 12.] {
            let position = point(from.x, from.y + px(dy));
            mouse(
                window,
                MouseMoveEvent {
                    position,
                    pressed_button: left,
                    modifiers,
                },
                cx,
            );
        }
    })
    .unwrap();

    // Mid-drag, a snapshot drops LLM-101 from the top, so every row below
    // it shifts up by one.
    reload_reordered(&view, |i| i.key.as_ref() != "LLM-101", cx);
    let neighbour = cx.read(|cx| match view.read(cx).rows(cx)[from_row] {
        Row::Issue(id) => Some(view.read(cx).issue(id, cx).clone()),
        Row::Header { .. } => None,
    });

    // The drop on the Done header moves the dragged task, not whatever
    // sits in its old row now.
    let to_row = done_header(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let to = window.find(("row", to_row)).bounds().center();
        let modifiers = Default::default();
        mouse(
            window,
            MouseMoveEvent {
                position: to,
                pressed_button: left,
                modifiers,
            },
            cx,
        );
        mouse(
            window,
            MouseUpEvent {
                button: MouseButton::Left,
                position: to,
                modifiers,
                click_count: 1,
            },
            cx,
        );
    })
    .unwrap();
    cx.read(|cx| {
        let this = view.read(cx);
        assert_eq!(this.issue(dragged.id, cx).status.name, "Done");
        if let Some(neighbour) = &neighbour {
            assert_eq!(this.issue(neighbour.id, cx).status, neighbour.status);
        }
    });
}

/// LLM-65, LLM-142: the title row sits on the filter bar's line. The meta
/// line sits right under the title, in the same block. The Edit and
/// Preview buttons sit in the description panel's header, below the head.
#[gpui_kit::test]
fn task_view_layout(cx: &mut TestAppContext) {
    let (handle, _view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
        let filter = window.find("filter-bar").bounds();
        let header = window.find("detail-header").bounds();
        assert_eq!(filter.bottom(), header.bottom());
        let head = window.find("detail-head").bounds();
        let meta = window.find("detail-meta").bounds();
        assert_eq!(meta.top(), header.bottom());
        assert!(meta.bottom() <= head.bottom());
        for id in ["labels-group", "cost-line", "deps-add"] {
            let item = window.find(id).bounds();
            assert!(
                item.top() >= meta.top() && item.bottom() <= meta.bottom(),
                "{id} {item:?} outside {meta:?}"
            );
        }
        let toggle = window.find("mode-toggle").bounds();
        let preview = window.find("preview").bounds();
        assert!(toggle.top() >= head.bottom());
        assert!(toggle.bottom() <= preview.top());
    })
    .unwrap();
}

/// LLM-142: on a narrow task view the meta line wraps and stays inside
/// the task view.
#[gpui_kit::test]
fn meta_line_wraps_when_narrow(cx: &mut TestAppContext) {
    let (handle, _view) = open_sized(cx, 420., 760.);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
        let meta = window.find("detail-meta").bounds();
        let head = window.find("detail-head").bounds();
        let one_line = window.find("cost-line").bounds().size.height;
        assert!(meta.right() <= head.right(), "{meta:?} past {head:?}");
        assert!(
            meta.size.height > one_line * 1.5,
            "the meta line did not wrap: {meta:?}"
        );
        for id in ["labels-group", "cost-line", "deps-add", "mode-toggle"] {
            let item = window.find(id).bounds();
            assert!(item.right() <= head.right(), "{id} {item:?} past {head:?}");
        }
    })
    .unwrap();
}

/// LLM-63: the claim timer reads in whole minutes.
#[test]
fn claim_age_reads_in_minutes() {
    use crate::model::data::claim_age;
    use jiff::SignedDuration as D;
    assert_eq!(claim_age(D::from_secs(30)), "<1m");
    assert_eq!(claim_age(D::from_mins(12)), "12m");
    assert_eq!(claim_age(D::from_mins(65)), "1h 05m");
    assert_eq!(claim_age(D::from_mins(60 * 26 + 3)), "1d 02h");
    // A server clock a little ahead never shows a negative time.
    assert_eq!(claim_age(D::from_secs(-90)), "<1m");
}

/// LLM-63: a claimed row shows how long it has been claimed.
#[gpui_kit::test]
fn claimed_row_shows_its_age(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let id = view.update(cx, |this, cx| {
        let mut issue = this.issue_by_key("LLM-102", cx).clone();
        // The demo claim started 65 minutes ago.
        issue.claim = Some(crate::model::demo::claude_claim("LLM-102"));
        let id = issue.id;
        this.upsert_issue(issue, cx);
        id
    });
    let claim = cx.read(|cx| view.read(cx).claim_of("LLM-102", cx).cloned().unwrap());
    assert_eq!(claim.age(jiff::Timestamp::now()), "1h 05m");
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let badge = crate::shell::widgets::issue_part("llm-age", id);
        assert!(window.try_find(badge).is_some());
    })
    .unwrap();
}

/// LLM-98: the LLM status column shows Locked for a claim and Analyzing
/// for a live mark, and nothing once the mark expires. Both badges sit
/// inside the column's cell.
#[gpui_kit::test]
fn llm_status_column_shows_locked_and_analyzing(cx: &mut TestAppContext) {
    use gpui_kit::SharedString;
    let (handle, view) = open(cx);
    let ids = view.update(cx, |this, cx| {
        let mut locked = this.issue_by_key("LLM-102", cx).clone();
        locked.claim = Some(crate::model::demo::claude_claim("LLM-102"));
        // A claim wins over a mark from another session.
        locked.analyzing = Some(crate::model::demo::analyzing(
            crate::model::demo::claude(),
            "LLM-102",
        ));
        let mut reading = this.issue_by_key("LLM-101", cx).clone();
        reading.claim = None;
        reading.analyzing = Some(crate::model::demo::analyzing(
            crate::model::demo::claude(),
            "LLM-101",
        ));
        let mut expired = this.issue_by_key("LLM-104", cx).clone();
        expired.claim = None;
        let mut mark = crate::model::demo::analyzing(crate::model::demo::claude(), "LLM-104");
        mark.expires_at = jiff::Timestamp::now() - jiff::SignedDuration::from_mins(1);
        expired.analyzing = Some(mark);
        let ids = (locked.id, reading.id, expired.id);
        this.upsert_issue(locked, cx);
        this.upsert_issue(reading, cx);
        this.upsert_issue(expired, cx);
        ids
    });
    let (locked, reading, expired) = ids;
    // The badges grow in on a spring.
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let find = |name, id| window.try_find(crate::shell::widgets::issue_part(name, id));
        assert!(find("llm-locked", locked).is_some());
        assert!(find("llm-analyzing", locked).is_none());
        assert!(find("llm-analyzing", reading).is_some());
        assert!(find("llm-locked", reading).is_none());
        assert!(find("llm-analyzing", expired).is_none());
        // The header names the column, and the badge stays in its cell.
        let header = window.within("column-header").find("col-llm").bounds();
        let badge = window
            .find(crate::shell::widgets::issue_part("llm-analyzing", reading))
            .bounds();
        assert!(badge.left() >= header.left() - px(0.5));
        assert!(badge.right() <= header.right() + px(0.5));
    })
    .unwrap();
    // The selection starts on LLM-101. Its task view shows the Analyzing
    // bar, and no claim makes it read-only.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
        let bar = SharedString::from(format!("detail-analyzing-{reading}"));
        assert!(window.try_find(bar).is_some());
    })
    .unwrap();
    cx.read(|cx| assert!(view.read(cx).claim_of("LLM-101", cx).is_none()));
}

/// The keys on the list, top to bottom.
fn listed_keys(view: &Entity<Workspace>, cx: &TestAppContext) -> Vec<String> {
    cx.read(|cx| {
        let this = view.read(cx);
        (0..this.list().read(cx).visible_count())
            .map(|n| this.visible_issue(n, cx).key.to_string())
            .collect()
    })
}

/// LLM-108: a click on the LLM status header puts Locked first, then
/// Analyzing, then the rest. A second click reverses the order.
#[gpui_kit::test]
fn llm_status_header_sorts_locked_then_analyzing(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("sort-llm", cx);
        window.render_frame(cx);
    })
    .unwrap();
    let keys = listed_keys(&view, cx);
    assert_eq!(keys[..4], ["LLM-102", "LLM-103", "LLM-105", "LLM-101"]);
    // The label and the arrow stay inside the column.
    cx.update_window(handle, |_, window, _| {
        let cell = window.within("column-header").find("col-llm").bounds();
        let button = window.find("sort-llm").bounds();
        assert!(button.right() <= cell.right() + px(0.5));
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.click("sort-llm", cx);
        window.render_frame(cx);
    })
    .unwrap();
    let keys = listed_keys(&view, cx);
    assert_eq!(keys[keys.len() - 3..], ["LLM-105", "LLM-103", "LLM-102"]);
    // The saved settings keep the sort.
    let saved = cx.read(|cx| view.read(cx).current_settings(cx));
    assert_eq!(saved.sort, "llm");
}

/// LLM-108: `:filter llm` fills the filter box, so the owner sees what
/// narrows the list, and `:filter` alone clears it.
#[gpui_kit::test]
fn filter_command_narrows_to_tasks_with_an_agent(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run_palette(handle, "filter llm", cx);
    assert_eq!(listed_keys(&view, cx), ["LLM-102", "LLM-103", "LLM-105"]);
    let box_text = |view: &Entity<Workspace>, cx: &TestAppContext| {
        cx.read(|cx| view.read(cx).list().read(cx).filter.read(cx).value())
    };
    assert_eq!(box_text(&view, cx), "llm:any");
    run_palette(handle, "sort llm", cx);
    assert_eq!(listed_keys(&view, cx), ["LLM-102", "LLM-103", "LLM-105"]);
    run_palette(handle, "filter", cx);
    assert_eq!(box_text(&view, cx), "");
    assert!(listed_keys(&view, cx).len() > 3);
}

/// LLM-108: a mark expires with no server event, so the clock tick takes
/// the task off a list that filters by the LLM status.
#[gpui_kit::test]
fn an_expired_mark_leaves_the_llm_filter(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run_palette(handle, "filter llm", cx);
    view.update(cx, |this, cx| {
        let mut reading = this.issue_by_key("LLM-105", cx).clone();
        let mark = reading.analyzing.as_mut().unwrap();
        mark.expires_at = jiff::Timestamp::now() + jiff::SignedDuration::from_millis(50);
        this.upsert_issue(reading, cx);
    });
    std::thread::sleep(std::time::Duration::from_millis(60));
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(16));
    cx.run_until_parked();
    assert_eq!(listed_keys(&view, cx), ["LLM-102", "LLM-103"]);
}

/// LLM-20: `:wait` sets and clears who a task waits on, and the list and
/// the task view show the mark only while it is set.
#[gpui_kit::test]
fn wait_command_sets_and_clears_waiting_on(cx: &mut TestAppContext) {
    use crate::model::data::WaitingOn;
    let (handle, view) = open(cx);
    let id = cx.read(|cx| view.read(cx).issue_by_key("LLM-101", cx).id);
    let waiting = |cx: &mut TestAppContext| {
        cx.read(|cx| view.read(cx).issue_by_key("LLM-101", cx).waiting_on)
    };
    let row_mark = crate::shell::widgets::issue_part("waiting", id);
    let detail_mark = gpui_kit::SharedString::from(format!("detail-waiting-{id}"));
    assert_eq!(waiting(cx), None);

    // The selection starts on LLM-101. Open it, so the task view shows.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        assert!(window.try_find(row_mark.clone()).is_none());
    })
    .unwrap();

    run_palette(handle, "wait owner", cx);
    assert_eq!(waiting(cx), Some(WaitingOn::Owner));
    cx.read(|cx| assert_eq!(view.read(cx).message(), "LLM-101 waits on you"));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(row_mark.clone()).is_some());
        assert!(window.try_find(detail_mark.clone()).is_some());
    })
    .unwrap();

    run_palette(handle, "wait agent", cx);
    assert_eq!(waiting(cx), Some(WaitingOn::Agent));

    run_palette(handle, "wait bogus", cx);
    assert_eq!(waiting(cx), Some(WaitingOn::Agent));
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).message(),
            "Wait on agent, owner or none, not bogus"
        )
    });

    run_palette(handle, "wait none", cx);
    assert_eq!(waiting(cx), None);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(row_mark.clone()).is_none());
        assert!(window.try_find(detail_mark.clone()).is_none());
    })
    .unwrap();
}

/// LLM-20: the demo data marks two tasks for an agent and two for the
/// owner, like `scripts/seed-demo`.
#[gpui_kit::test]
fn demo_rows_show_waiting_marks(cx: &mut TestAppContext) {
    use crate::model::data::WaitingOn;
    let (_, view) = open(cx);
    cx.read(|cx| {
        let of = |key: &str| view.read(cx).issue_by_key(key, cx).waiting_on;
        assert_eq!(of("LLM-102"), Some(WaitingOn::Agent));
        assert_eq!(of("LLM-104"), Some(WaitingOn::Agent));
        assert_eq!(of("LLM-103"), Some(WaitingOn::Owner));
        assert_eq!(of("LLM-111"), Some(WaitingOn::Owner));
    });
}

/// LLM-17: a click on an issue link in the preview opens that issue, and
/// selects it in the list.
#[gpui_kit::test]
fn issue_links_open_the_issue(cx: &mut TestAppContext) {
    let (_, view) = open(cx);
    let target = cx.read(|cx| view.read(cx).issue_by_key("LLM-104", cx).id);
    view.update(cx, |this, cx| this.open_key("LLM-104", cx));
    cx.read(|cx| {
        let ws = view.read(cx);
        assert_eq!(ws.detail(cx).map(|d| d.issue), Some(target));
        assert_eq!(ws.selected_issue_key(cx).as_deref(), Some("LLM-104"));
        assert_eq!(ws.message(), "Opened LLM-104");
    });

    // A key the store does not hold says so and changes nothing.
    view.update(cx, |this, cx| this.open_key("ALX-9", cx));
    cx.read(|cx| {
        let ws = view.read(cx);
        assert_eq!(ws.detail(cx).map(|d| d.issue), Some(target));
        assert_eq!(ws.message(), "ALX-9 is not in this workspace");
    });
}

/// LLM-17: the preview learns the server's address when it connects, so
/// stored images load from it.
#[gpui_kit::test]
fn the_server_address_reaches_the_preview(cx: &mut TestAppContext) {
    let (_, view) = open(cx);
    view.update(cx, |this, cx| {
        this.apply_event(
            crate::model::backend::Event::Connected {
                url: "http://127.0.0.1:4567".into(),
            },
            cx,
        )
    });
    let server = cx.read(|cx| view.read(cx).server_url(cx));
    assert_eq!(server.as_deref(), Some("http://127.0.0.1:4567"));
    assert_eq!(
        crate::task::preview::prepare("![i](/v1/files/h)", server.as_deref(), |_| false),
        "![i](http://127.0.0.1:4567/v1/files/h)"
    );
}

/// A workspace on demo data in a window of the given size, with the task
/// view `detail_width` pixels wide beside the list.
fn open_wide(
    width: f32,
    height: f32,
    detail_width: f32,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<Workspace>) {
    let settings = crate::settings::UiSettings {
        split_side: Some(detail_width),
        ..Default::default()
    };
    cx.update(crate::init);
    cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(width), px(height)),
                ))),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                cx.new(|cx| {
                    let store = crate::settings::SettingsStore::memory();
                    let mut this = Workspace::with_settings(settings, store, window, cx);
                    this.load_demo(cx);
                    this
                })
            },
        )
        .expect("open test window")
    })
}

/// LLM-64: a wide task view shows every tab as a column. A narrow one keeps
/// the tab bar.
#[gpui_kit::test]
fn wide_task_view_shows_tabs_as_columns(cx: &mut TestAppContext) {
    // LLM-101 has three tabs: Description, Agent notes and Work log.
    let (handle, view) = open_wide(2800., 900., 1800., cx);
    let tabs = cx.read(|cx| view.read(cx).open_tab(cx).map(|(_, t)| t.len()));
    assert_eq!(tabs, None);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(view.read(cx).open_tab(cx).map(|(_, t)| t.len()), Some(3));
        assert!(window.try_find("preview-titles").is_some());
        let columns: Vec<_> = (0..3)
            .map(|ix: usize| window.find(("preview-column", ix)).bounds())
            .collect();
        // Side by side, each at least the minimum width.
        let min = window.rem_size() * crate::task::task_view::TaskView::COLUMN_MIN_REM;
        for pair in columns.windows(2) {
            assert!(pair[0].right() <= pair[1].left());
        }
        for c in &columns {
            assert!(c.size.width >= min, "{:?} < {min:?}", c.size.width);
        }
    })
    .unwrap();

    // The default width, beside the list, keeps the tabs.
    let (handle, _) = open_wide(1200., 760., 560., cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("preview-titles").is_none());
        assert!(window.try_find(("preview-column", 0usize)).is_none());
    })
    .unwrap();
}

/// LLM-64: the breakpoint needs room for every tab at the minimum width.
#[test]
fn columns_need_room_for_every_tab() {
    use crate::task::task_view::TaskView;
    let rem = px(16.);
    let column = rem * TaskView::COLUMN_MIN_REM;
    assert!(!TaskView::columns_fit(column * 10., 1, rem));
    assert!(TaskView::columns_fit(column * 2., 2, rem));
    assert!(!TaskView::columns_fit(column * 2. - px(1.), 2, rem));
    assert!(!TaskView::columns_fit(column * 2., 3, rem));
}

/// LLM-77: each column header cell has the left edge and width of the
/// cells below it, at a narrow, a medium and a wide window.
#[gpui_kit::test]
fn column_header_lines_up_with_rows(cx: &mut TestAppContext) {
    use crate::issues::columns::Column;
    use gpui_kit::ElementId;
    for width in [720., 1000., 1600.] {
        let (handle, _view) = open_sized(cx, width, 760.);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            let header = window.find("column-header").bounds();
            // Issue rows only: group headers have no column cells.
            let rows: Vec<ElementId> = (0..12usize)
                .map(|row| ElementId::from(("row", row)))
                .filter(|id| {
                    window
                        .within(id.clone())
                        .try_find(Column::Key.id())
                        .is_some()
                })
                .collect();
            assert!(rows.len() >= 3, "at least three issue rows to compare");
            for column in Column::ALL {
                let Some(head) = window.within("column-header").try_find(column.id()) else {
                    // A hidden column has no cell in any row either.
                    for id in &rows {
                        assert!(window.within(id.clone()).try_find(column.id()).is_none());
                    }
                    continue;
                };
                let head = head.bounds();
                assert!(head.left() >= header.left() && head.right() <= header.right());
                for id in &rows {
                    let cell = window.within(id.clone()).find(column.id()).bounds();
                    assert_eq!(
                        (cell.left(), cell.size.width),
                        (head.left(), head.size.width),
                        "{column:?} at window width {width}, {id:?}"
                    );
                }
            }
        })
        .unwrap();
    }
}

/// LLM-76: the list hides columns by its own width, so opening the task
/// view beside it hides some. The header shows the columns that fit. The
/// smart split keeps every column, so the task view here has a width the
/// owner dragged it to.
/// The columns the list header draws, left to right.
fn header_columns(window: &mut gpui_kit::Window) -> Vec<crate::issues::columns::Column> {
    use crate::issues::columns::Column;
    Column::ALL
        .into_iter()
        .filter(|c| window.within("column-header").try_find(c.id()).is_some())
        .collect()
}

/// Right-clicks a column heading and clicks the checklist entry of `column`.
fn click_column_entry(
    handle: AnyWindowHandle,
    heading: &'static str,
    column: crate::issues::columns::Column,
    cx: &mut TestAppContext,
) {
    let entry = crate::issues::columns::Column::ALL
        .iter()
        .position(|c| *c == column)
        .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.right_click(heading, cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.within("popup-menu").click(entry, cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_heading_menu_hides_a_column_and_saves_the_choice(cx: &mut TestAppContext) {
    use crate::issues::columns::Column;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ui.json");
    let (handle, view) = open_with_settings(&path, Default::default(), cx);
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(header_columns(window).contains(&Column::Key));
    })
    .unwrap();

    // A right-click on a sort button opens the checklist too.
    click_column_entry(handle, "sort-key", Column::Key, cx);
    cx.update_window(handle, |_, window, _| {
        assert!(!header_columns(window).contains(&Column::Key));
        assert!(
            window
                .within(("row", 0usize))
                .try_find(Column::Key.id())
                .is_none()
        );
    })
    .unwrap();
    cx.read(|cx| assert_eq!(view.read(cx).current_settings(cx).hidden_columns, ["key"]));
    cx.run_until_parked();
    let saved = crate::settings::SettingsStore::at(path.clone()).load();
    assert_eq!(saved.hidden_columns, ["key"]);

    // The next time it opens, the checks match the columns.
    cx.update_window(handle, |_, window, cx| {
        window.right_click("column-header", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let entry = |column| Column::ALL.iter().position(|c| *c == column).unwrap();
        let menu = window.within("popup-menu");
        assert_eq!(menu.find(entry(Column::Key)).checked(), Some(false));
        assert_eq!(menu.find(entry(Column::Status)).checked(), Some(true));
        window.press("escape", cx);
    })
    .unwrap();

    // The same entry brings it back, on the heading of another column.
    click_column_entry(handle, "column-header", Column::Key, cx);
    cx.update_window(handle, |_, window, _| {
        assert!(header_columns(window).contains(&Column::Key));
        assert!(
            window
                .within(("row", 0usize))
                .try_find(Column::Key.id())
                .is_some()
        );
    })
    .unwrap();
    cx.read(|cx| assert!(view.read(cx).current_settings(cx).hidden_columns.is_empty()));
}

#[gpui_kit::test]
fn the_title_column_entry_does_nothing(cx: &mut TestAppContext) {
    use crate::issues::columns::Column;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ui.json");
    let (handle, view) = open_with_settings(&path, Default::default(), cx);
    cx.run_until_parked();
    click_column_entry(handle, "column-header", Column::Title, cx);
    cx.update_window(handle, |_, window, _| {
        assert!(header_columns(window).contains(&Column::Title));
    })
    .unwrap();
    cx.read(|cx| assert!(view.read(cx).current_settings(cx).hidden_columns.is_empty()));
}

#[gpui_kit::test]
fn hidden_columns_come_back_after_a_restart(cx: &mut TestAppContext) {
    use crate::issues::columns::Column;
    let dir = tempfile::tempdir().unwrap();
    let settings = crate::settings::UiSettings {
        // An unknown name and `title` are ignored.
        hidden_columns: vec!["labels".into(), "title".into(), "gone".into()],
        ..Default::default()
    };
    let (handle, view) = open_with_settings(&dir.path().join("ui.json"), settings, cx);
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        let shown = header_columns(window);
        assert!(!shown.contains(&Column::Labels));
        assert!(shown.contains(&Column::Title));
    })
    .unwrap();
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).current_settings(cx).hidden_columns,
            ["labels"]
        );
    });
}

#[gpui_kit::test]
fn narrow_list_hides_columns_by_priority(cx: &mut TestAppContext) {
    use crate::issues::columns::{self, Column};
    let (handle, view) = open_wide(1200., 760., 600., cx);
    cx.update_window(handle, |_, window, cx| {
        let shown = |window: &mut gpui_kit::Window| -> Vec<Column> {
            Column::ALL
                .into_iter()
                .filter(|c| window.within("column-header").try_find(c.id()).is_some())
                .collect()
        };
        window.render_frame(cx);
        window.render_frame(cx);
        assert_eq!(shown(window), Column::ALL, "a wide list shows every column");

        window.press("enter", cx);
        window.render_frame(cx);
        window.render_frame(cx);
        let width = window.find("column-header").bounds().size.width;
        let open = shown(window);
        let demand = view.read(cx).list().read(cx).demand(cx);
        assert_eq!(
            open,
            columns::visible(Some(width), window.rem_size(), &[], &demand)
        );
        assert!(
            !open.contains(&Column::Added),
            "the task view leaves too little room for the date: {width:?}"
        );
        for column in [Column::Status, Column::Key, Column::Title] {
            assert!(open.contains(&column), "{column:?} always shows");
        }

        window.press("escape", cx);
        window.render_frame(cx);
        window.render_frame(cx);
        assert_eq!(
            shown(window),
            Column::ALL,
            "closing the task view brings them back"
        );
    })
    .unwrap();
}

/// LLM-81: the lock card lists who holds the claim and where they work.
#[test]
fn claim_card_lists_holder_model_branch_worktree_and_start() {
    let claim = crate::model::demo::claude_claim("LLM-102");
    let card = crate::shell::tooltip::Card::claim(&claim, jiff::Timestamp::now());
    assert_eq!(
        card.heading.as_deref(),
        Some("Locked by claude-code opus-5.5")
    );
    let row = |label: &str| {
        card.rows
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, v)| v.to_string())
    };
    assert_eq!(row("Model").as_deref(), Some("opus-5.5"));
    assert_eq!(row("Effort").as_deref(), Some("high"));
    assert_eq!(row("Branch").as_deref(), Some("main"));
    assert_eq!(row("Worktree").as_deref(), Some("/work/llmtodo-llm-102"));
    assert_eq!(row("Pane").as_deref(), Some("w1:p2"));
    assert!(row("Started").unwrap().ends_with(", 1h 05m ago"));
}

/// LLM-81: hovering the lock badge shows a card that wraps long content
/// at the card's maximum width and stays inside the window.
#[gpui_kit::test]
fn lock_badge_shows_a_wrapped_card(cx: &mut TestAppContext) {
    let (handle, view) = open_sized(cx, 700., 600.);
    let id = view.update(cx, |this, cx| {
        let mut issue = this.issue_by_key("LLM-102", cx).clone();
        let mut claim = crate::model::demo::claude_claim("LLM-102");
        claim.branch = Some(
            "feature/a very long branch name that keeps going well past one line of text".into(),
        );
        issue.claim = Some(claim);
        let id = issue.id;
        this.upsert_issue(issue, cx);
        id
    });
    // The badge grows in on a spring.
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.hover(crate::shell::widgets::issue_part("llm-locked", id), cx);
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let card = window.find("tooltip-card").bounds();
        let max = window.rem_size() * crate::shell::tooltip::MAX_WIDTH;
        assert!(
            card.size.width <= max + px(0.5),
            "{card:?} wider than {max:?}"
        );
        // A heading, a body line and seven rows, one of them wrapped.
        assert!(card.size.height > window.rem_size() * 8., "{card:?}");
        let viewport = window.viewport_size();
        assert!(card.right() <= viewport.width && card.bottom() <= viewport.height);
        assert!(card.left() >= px(0.) && card.top() >= px(0.));
    })
    .unwrap();
}

/// LLM-82: every tab column reaches the bottom of the task view, however
/// short its text. A long column scrolls on its own with j and k.
#[gpui_kit::test]
fn tab_columns_fill_the_task_view_height(cx: &mut TestAppContext) {
    let (handle, view) = open_wide(2800., 600., 1800., cx);
    view.update(cx, |this, cx| {
        let mut issue = this.issue_by_key("LLM-101", cx).clone();
        let long: String = (1..=80)
            .map(|n| format!("Line {n} of a long description.\n\n"))
            .collect();
        issue.description =
            format!("{long}+++ Agent notes\n\nShort.\n+++\n\n+++ Work log\n\nShort.\n+++\n").into();
        this.upsert_issue(issue, cx);
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let preview = window.find("preview").bounds();
        let columns: Vec<_> = (0..3)
            .map(|ix: usize| window.find(("preview-column", ix)).bounds())
            .collect();
        for c in &columns {
            assert_eq!(c.top(), preview.top(), "{c:?} in {preview:?}");
            assert_eq!(c.bottom(), preview.bottom(), "{c:?} in {preview:?}");
        }

        // The Description column is the open tab, so j scrolls it. The
        // short columns stay where they are.
        assert_eq!(view.read(cx).preview_offset(cx), gpui_kit::px(0.));
        window.press("j", cx);
        window.press("j", cx);
        window.render_frame(cx);
        assert!(view.read(cx).preview_offset(cx) < gpui_kit::px(0.));
        for (ix, before) in columns.iter().enumerate() {
            assert_eq!(window.find(("preview-column", ix)).bounds(), *before);
        }
    })
    .unwrap();
}

/// LLM-96: the list gets width first, up to the width at which it fits
/// most of its content. After that, the task view takes the rest.
#[gpui_kit::test]
fn the_list_grows_first_and_the_task_view_takes_the_rest(cx: &mut TestAppContext) {
    let (handle, view) = open_sized(cx, 1040., 760.);
    let frames = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            for _ in 0..3 {
                window.render_frame(cx);
            }
        })
        .unwrap();
    };
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    frames(cx);
    let layout = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            let this = view.read(cx);
            let (room, want) = this.split_room(window, cx);
            (
                room,
                want,
                this.detail_size(cx).expect("task view laid out"),
            )
        })
        .unwrap()
    };

    // Narrow: the task view keeps its minimum, 22 rem, and the list
    // has not yet reached its comfortable width.
    let (room, want, detail) = layout(cx);
    assert!(
        room - want < px(352.),
        "the test needs a narrow room: {room:?} {want:?} {detail:?}"
    );
    assert_eq!(detail, px(352.));

    // Wide: the list stops at its comfortable width.
    cx.simulate_window_resize(handle, size(px(2400.), px(760.)));
    frames(cx);
    let (room, want, detail) = layout(cx);
    assert!(room - want > px(352.));
    assert!(
        (f32::from(room - want - detail)).abs() < 1.,
        "{room:?} {want:?} {detail:?}"
    );

    // Narrow again: the task view gives the width back.
    cx.simulate_window_resize(handle, size(px(1040.), px(760.)));
    frames(cx);
    assert_eq!(layout(cx).2, px(352.));
}

/// LLM-96: `:split auto` drops a dragged width, and the smart split
/// sizes the task view again.
#[gpui_kit::test]
fn split_auto_drops_the_dragged_width(cx: &mut TestAppContext) {
    let (handle, view) = open_wide(2400., 760., 400., cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        for _ in 0..3 {
            window.render_frame(cx);
        }
        assert_eq!(view.read(cx).detail_size(cx), Some(px(400.)), "a drag pins");
        view.update(cx, |this, cx| this.run_command("split auto", window, cx));
        for _ in 0..3 {
            window.render_frame(cx);
        }
        let this = view.read(cx);
        let (room, want) = this.split_room(window, cx);
        let detail = this.detail_size(cx).expect("task view laid out");
        assert!(
            (f32::from(room - want - detail)).abs() < 1.,
            "{room:?} {want:?} {detail:?}"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_mouse_back_button_returns_to_the_previous_view(cx: &mut TestAppContext) {
    use gpui_kit::{MouseButton, MouseDownEvent, MouseUpEvent, NavigationDirection};
    let (handle, view) = open(cx);
    let back = MouseButton::Navigate(NavigationDirection::Back);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
        assert!(view.read(cx).detail(cx).is_some());
        let position = window.find(("row", 0usize)).bounds().center();
        let modifiers = Default::default();
        mouse(
            window,
            MouseDownEvent {
                button: back,
                position,
                modifiers,
                click_count: 1,
                first_mouse: false,
            },
            cx,
        );
        mouse(
            window,
            MouseUpEvent {
                button: back,
                position,
                modifiers,
                click_count: 1,
            },
            cx,
        );
        assert!(view.read(cx).detail(cx).is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_new_question_bounces_the_dock_only_in_the_background(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.run_until_parked();
    let ask = |cx: &mut TestAppContext, n: usize| {
        cx.update(|cx| {
            view.update(cx, |this, cx| {
                let mut issue = this.visible_issue(n, cx).clone();
                issue.description =
                    "+++ Owner questions\n### Q1. Ship it?\n\n**Answer:**\n+++".into();
                issue.open_questions = 1;
                this.upsert_issue(issue, cx);
            })
        });
        cx.run_until_parked();
    };
    let bounces = |cx: &mut TestAppContext| cx.update(|cx| view.read(cx).dock_bounces());
    assert_eq!(bounces(cx), 0, "the first load does not bounce");

    let mut window = gpui_kit::VisualTestContext::from_window(handle, cx);
    window.deactivate_window();
    ask(&mut window, 0);
    assert_eq!(bounces(&mut window), 1, "a new question bounces once");

    ask(&mut window, 0);
    assert_eq!(bounces(&mut window), 1, "the same count does not bounce");
}

/// Presses each key on its own, with a frame and the observers after it.
fn keys(handle: AnyWindowHandle, keys: &[&str], cx: &mut TestAppContext) {
    for key in keys {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
}

fn open_key(key: SharedString, view: &Entity<Workspace>, cx: &mut TestAppContext) {
    cx.update(|cx| view.update(cx, |this, cx| this.open_key(&key, cx)));
    cx.run_until_parked();
}

fn nth_key(n: usize, view: &Entity<Workspace>, cx: &mut TestAppContext) -> SharedString {
    cx.update(|cx| view.read(cx).visible_issue(n, cx).key.clone())
}

fn open_key_now(view: &Entity<Workspace>, cx: &mut TestAppContext) -> Option<SharedString> {
    cx.update(|cx| view.read(cx).open_task_key(cx))
}

#[gpui_kit::test]
fn ctrl_o_and_ctrl_i_walk_tasks_and_scopes(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let first = nth_key(0, &view, cx);
    let second = nth_key(1, &view, cx);
    open_key(first.clone(), &view, cx);
    open_key(second.clone(), &view, cx);
    keys(handle, &["g", "i"], cx);
    assert!(cx.update(|cx| view.read(cx).inbox_shown(cx)));

    keys(handle, &["ctrl-o"], cx);
    assert!(!cx.update(|cx| view.read(cx).inbox_shown(cx)));
    assert_eq!(open_key_now(&view, cx), Some(second.clone()));

    keys(handle, &["ctrl-o"], cx);
    assert_eq!(open_key_now(&view, cx), Some(first));

    keys(handle, &["ctrl-i"], cx);
    assert_eq!(open_key_now(&view, cx), Some(second));

    keys(handle, &["ctrl-i"], cx);
    assert!(cx.update(|cx| view.read(cx).inbox_shown(cx)));
}

#[gpui_kit::test]
fn the_mouse_forward_button_replays_a_view(cx: &mut TestAppContext) {
    use gpui_kit::{MouseButton, MouseDownEvent, NavigationDirection};
    let (handle, view) = open(cx);
    keys(handle, &["enter"], cx);
    let open_task = open_key_now(&view, cx);
    assert!(open_task.is_some());
    keys(handle, &["ctrl-o"], cx);
    assert_eq!(open_key_now(&view, cx), None);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let position = window.find(("row", 0usize)).bounds().center();
        mouse(
            window,
            MouseDownEvent {
                button: MouseButton::Navigate(NavigationDirection::Forward),
                position,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            },
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(open_key_now(&view, cx), open_task);
}

#[gpui_kit::test]
fn walking_the_list_is_one_view_in_the_history(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    keys(handle, &["enter", "j", "j"], cx);
    let walked_to = open_key_now(&view, cx);
    keys(handle, &["g", "i", "ctrl-o"], cx);
    assert_eq!(open_key_now(&view, cx), walked_to);
    keys(handle, &["ctrl-o"], cx);
    assert_eq!(open_key_now(&view, cx), None, "the view before the walk");
}

#[gpui_kit::test]
fn back_returns_to_the_tab_the_task_showed(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    keys(handle, &["enter"], cx);
    let tab = |cx: &mut TestAppContext| cx.update(|cx| view.read(cx).open_tab(cx).unwrap().0);
    assert_eq!(tab(cx), 0);
    keys(handle, &["g", "t"], cx);
    assert_eq!(tab(cx), 1);
    keys(handle, &["ctrl-o"], cx);
    assert_eq!(tab(cx), 0);
    keys(handle, &["ctrl-i"], cx);
    assert_eq!(tab(cx), 1);
}

#[gpui_kit::test]
fn back_skips_a_task_that_moved_to_the_trash(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let first = nth_key(0, &view, cx);
    let second = nth_key(1, &view, cx);
    let third = nth_key(2, &view, cx);
    open_key(first.clone(), &view, cx);
    open_key(second.clone(), &view, cx);
    open_key(third, &view, cx);
    cx.update(|cx| {
        view.update(cx, |this, cx| {
            let mut issue = this.issue_by_key(&second, cx).clone();
            issue.deleted = true;
            this.upsert_issue(issue, cx);
        })
    });
    cx.run_until_parked();
    keys(handle, &["ctrl-o"], cx);
    assert_eq!(open_key_now(&view, cx), Some(first));
}

#[gpui_kit::test]
fn the_history_keys_type_in_a_text_field(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    keys(handle, &["enter", "/"], cx);
    let open_task = open_key_now(&view, cx);
    keys(handle, &["ctrl-o"], cx);
    assert_eq!(open_key_now(&view, cx), open_task);
}

#[gpui_kit::test]
fn back_waits_for_the_editor(cx: &mut TestAppContext) {
    use gpui_kit::{MouseButton, MouseDownEvent, NavigationDirection};
    let (handle, view) = open(cx);
    keys(handle, &["enter", "e"], cx);
    let open_task = open_key_now(&view, cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let position = window.find(("row", 0usize)).bounds().center();
        mouse(
            window,
            MouseDownEvent {
                button: MouseButton::Navigate(NavigationDirection::Back),
                position,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            },
            cx,
        );
    })
    .unwrap();
    assert_eq!(open_key_now(&view, cx), open_task);
    cx.update(|cx| assert_eq!(view.read(cx).message(), "Save or leave the editor first"));
}

#[gpui_kit::test]
fn back_from_the_claims_list_closes_it(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    keys(handle, &["g", "c"], cx);
    assert!(cx.update(|cx| view.read(cx).claims().read(cx).is_open()));
    keys(handle, &["ctrl-o"], cx);
    assert!(!cx.update(|cx| view.read(cx).claims().read(cx).is_open()));
    keys(handle, &["ctrl-i"], cx);
    assert!(cx.update(|cx| view.read(cx).claims().read(cx).is_open()));
}

#[gpui_kit::test]
fn back_with_no_earlier_view_says_so(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    keys(handle, &["ctrl-o"], cx);
    cx.update(|cx| assert_eq!(view.read(cx).message(), "No earlier view"));
    keys(handle, &["ctrl-i"], cx);
    cx.update(|cx| assert_eq!(view.read(cx).message(), "No later view"));
}

#[gpui_kit::test]
fn escape_still_steps_out_without_using_the_history(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    keys(handle, &["enter", "escape"], cx);
    assert_eq!(open_key_now(&view, cx), None);
    keys(handle, &["ctrl-o"], cx);
    assert!(
        open_key_now(&view, cx).is_some(),
        "Esc is a step in history"
    );
}

/// The owner's rule: no element marks its state with a left border. The
/// thin `border_l_1` lines between side-by-side columns are dividers, so
/// this guard checks the thicker widths a marker would use.
#[test]
fn no_view_draws_a_left_border_marker() {
    let sources = [
        ("columns.rs", include_str!("../issues/columns.rs")),
        ("docs.rs", include_str!("../docs/mod.rs")),
        ("inbox.rs", include_str!("../inbox/mod.rs")),
        ("list.rs", include_str!("../issues/list.rs")),
        ("palette.rs", include_str!("palette.rs")),
        ("sidebar.rs", include_str!("sidebar.rs")),
        ("task_view.rs", include_str!("../task/task_view.rs")),
        ("claims.rs", include_str!("../issues/claims.rs")),
        ("create.rs", include_str!("../issues/create.rs")),
        ("widgets.rs", include_str!("widgets.rs")),
        ("workspace.rs", include_str!("workspace.rs")),
    ];
    let markers = [
        "border_l_2",
        "border_l_3",
        "border_l_4",
        "border_l_8",
        "border_l(",
    ];
    for (file, text) in sources {
        for marker in markers {
            assert!(!text.contains(marker), "{file} uses {marker}");
        }
    }
}

/// Views do not know the workspace. They emit a `ViewEvent`, so a view can
/// be built with no workspace behind it.
#[test]
fn no_view_names_the_workspace() {
    let sources = [
        ("claims.rs", include_str!("../issues/claims.rs")),
        ("columns.rs", include_str!("../issues/columns.rs")),
        ("create.rs", include_str!("../issues/create.rs")),
        ("docs.rs", include_str!("../docs/mod.rs")),
        ("inbox.rs", include_str!("../inbox/mod.rs")),
        ("issue_menu.rs", include_str!("issue_menu.rs")),
        ("list.rs", include_str!("../issues/list.rs")),
        ("palette.rs", include_str!("palette.rs")),
        ("settings/view.rs", include_str!("../settings/view.rs")),
        ("sidebar.rs", include_str!("sidebar.rs")),
        ("task_view.rs", include_str!("../task/task_view.rs")),
        ("widgets.rs", include_str!("widgets.rs")),
    ];
    for (name, source) in sources {
        assert!(
            !source
                .replace("(Workspace && mode == normal)", "")
                .contains("Workspace"),
            "{name} names Workspace"
        );
        assert!(
            !source.contains("shell::workspace"),
            "{name} imports the workspace module"
        );
    }
}

#[gpui_kit::test]
fn grouping_by_status_hides_the_status_column(cx: &mut TestAppContext) {
    use crate::issues::columns::Column;
    use crate::issues::list::GroupBy;
    let dir = tempfile::tempdir().unwrap();
    let (handle, view) = open_with_settings(&dir.path().join("ui.json"), Default::default(), cx);
    cx.run_until_parked();
    let group = |by: GroupBy, cx: &mut TestAppContext| {
        view.update(cx, |this, cx| {
            this.list().update(cx, |list, cx| list.group_by(by, cx));
        });
        cx.run_until_parked();
    };
    let shown = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            header_columns(window)
        })
        .unwrap()
    };
    assert!(shown(cx).contains(&Column::Status));
    group(GroupBy::Status, cx);
    assert!(
        !shown(cx).contains(&Column::Status),
        "the groups name the status"
    );
    cx.read(|cx| {
        assert!(
            view.read(cx).current_settings(cx).hidden_columns.is_empty(),
            "the owner's column choice stays as it was"
        );
    });
    group(GroupBy::None, cx);
    assert!(shown(cx).contains(&Column::Status));
}

/// LLM-183: the Locked badge shows the model, and the longest one fits.
#[gpui_kit::test]
fn the_locked_badge_shows_the_model_and_it_fits(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let id = view.update(cx, |this, cx| {
        let mut issue = this.issue_by_key("LLM-102", cx).clone();
        let mut claim = crate::model::demo::claude_claim("LLM-102");
        if let crate::model::data::Assignee::Agent(agent) = &mut claim.actor {
            agent.model = Some("claude-sonnet-5-5".into());
        }
        issue.claim = Some(claim);
        let id = issue.id;
        this.upsert_issue(issue, cx);
        id
    });
    let claim = cx.read(|cx| view.read(cx).claim_of("LLM-102", cx).cloned().unwrap());
    assert_eq!(claim.badge(jiff::Timestamp::now()), "Sonnet 5.5");
    // The badge grows in on a spring.
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let part = crate::shell::widgets::issue_part;
        let badge = window.find(part("llm-locked", id)).bounds();
        let text = window.find(part("llm-age", id)).bounds();
        let natural = window.text_system().layout_line(
            "Sonnet 5.5",
            window.rem_size() * 0.75,
            &[window.text_style().to_run("Sonnet 5.5".len())],
            None,
        );
        // The text box ends inside the badge's padding, and it is as wide
        // as the text needs, so nothing is cut off.
        assert!(text.right() <= badge.right(), "{text:?} in {badge:?}");
        assert!(
            text.size.width + gpui_kit::px(0.5) >= natural.width,
            "the text is cut off: {:?} < {:?}",
            text.size.width,
            natural.width
        );
    })
    .unwrap();
}
