//! UI integration tests for project documents.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::model::backend::{Backend, Event, Request};
use crate::model::scope::Scope;
use crate::shell::workspace::Workspace;

const DOC: u128 = 901;

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

fn run(handle: AnyWindowHandle, view: &Entity<Workspace>, command: &str, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.run_command(command, window, cx))
    })
    .unwrap();
}

fn doc(body: &str, version: i64) -> Box<llmtodo_client::types::Document> {
    Box::new(
        serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::from_u128(DOC), "project": "app", "slug": "release-checklist",
            "title": "Release checklist", "body": body, "version": version,
            "created_at": "2026-11-10T10:00:00Z", "updated_at": "2026-11-10T10:00:00Z",
            "deleted_at": null
        }))
        .unwrap(),
    )
}

fn saves(requests: &mut UnboundedReceiver<Request>) -> Vec<(String, i64, bool)> {
    let mut out = Vec::new();
    while let Ok(r) = requests.try_recv() {
        if let Request::SaveDocument {
            body, base, force, ..
        } = r
        {
            out.push((body, base, force));
        }
    }
    out
}

fn asked_for_doc(requests: &mut UnboundedReceiver<Request>) -> bool {
    let mut asked = false;
    while let Ok(r) = requests.try_recv() {
        asked |=
            matches!(r, Request::Document { id } if id == llmtodo_core::DocumentId::from_u128(DOC));
    }
    asked
}

#[gpui_kit::test]
fn a_document_opens_edits_and_saves_on_its_version(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    run(handle, &view, "doc release", cx);
    assert!(asked_for_doc(&mut requests));
    cx.read(|cx| {
        assert!(view.read(cx).doc_shown(cx));
        assert_eq!(view.read(cx).message(), "Showing Release checklist");
    });
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::Document(doc("## Before the beta\n\n- Seed\n", 2)),
            cx,
        )
    });

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("e", cx);
        window.render_frame(cx);
        let docs = view.read(cx).doc_view().read(cx);
        assert!(docs.editing);
        assert_eq!(
            docs.editor.read(cx).value().as_ref(),
            "## Before the beta\n\n- Seed\n"
        );
        assert_eq!(view.read(cx).vim_mode(window, cx), "insert");
        let editor = docs.editor.clone();
        editor.update(cx, |state, cx| state.set_value("New body\n", window, cx));
        window.press("escape", cx);
    })
    .unwrap();
    cx.read(|cx| assert!(!view.read(cx).doc_view().read(cx).editing));
    assert_eq!(saves(&mut requests), [("New body\n".to_string(), 2, false)]);

    view.update(cx, |this, cx| {
        this.apply_event(Event::DocumentSaved(doc("New body\n", 3)), cx);
        assert_eq!(this.message(), "Saved Release checklist");
    });
}

#[gpui_kit::test]
fn a_stale_document_save_keeps_the_text_and_w_bang_overwrites(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    run(handle, &view, "doc release", cx);
    view.update(cx, |this, cx| {
        this.apply_event(Event::Document(doc("Old\n", 2)), cx)
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |this, cx| this.edit_document(window, cx));
        let editor = view.read(cx).doc_view().read(cx).editor.clone();
        editor.update(cx, |state, cx| state.set_value("Mine\n", window, cx));
    })
    .unwrap();
    run(handle, &view, "w", cx);
    assert_eq!(saves(&mut requests), [("Mine\n".to_string(), 2, false)]);

    // Someone saved first. The server's text comes back for the preview.
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::DocumentFailed {
                id: llmtodo_core::DocumentId::from_u128(DOC),
                message: "stale".into(),
                stale: true,
            },
            cx,
        );
        assert!(this.message().contains(":w! to overwrite"));
    });
    assert!(asked_for_doc(&mut requests));

    // Leaving and coming back: e brings the owner's text back. Leaving does
    // not send the refused text again.
    run(handle, &view, "q", cx);
    view.update(cx, |this, cx| {
        this.apply_event(Event::Document(doc("Theirs\n", 3)), cx)
    });
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.edit_document(window, cx));
        let text = view.read(cx).doc_view().read(cx).editor.read(cx).value();
        assert_eq!(text.as_ref(), "Mine\n");
    })
    .unwrap();
    // A plain save would overwrite the other version, so it waits for :w!.
    run(handle, &view, "w", cx);
    assert!(saves(&mut requests).is_empty());
    cx.read(|cx| assert!(view.read(cx).message().contains(":w! does")));
    run(handle, &view, "w!", cx);
    assert_eq!(saves(&mut requests), [("Mine\n".to_string(), 3, true)]);
}

#[gpui_kit::test]
fn an_issue_link_in_a_document_goes_back_to_the_list(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    run(handle, &view, "sidebar", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click(format!("sidebar-doc:{}", uuid::Uuid::from_u128(DOC)), cx);
    })
    .unwrap();
    cx.read(|cx| assert!(view.read(cx).doc_shown(cx)));
    view.update(cx, |this, cx| {
        this.open_key("LLM-104", cx);
        assert_eq!(this.scope(cx), Scope::All);
        assert_eq!(this.selected_issue_key(cx).as_deref(), Some("LLM-104"));
    });
}
