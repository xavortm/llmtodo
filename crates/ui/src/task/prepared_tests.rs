//! The task view, the document view and the status bar build their text
//! once per change, not once per frame (LLM-174).

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions,
    px, size,
};

use crate::model::data::IssueId;
use crate::model::store::IssueStore;
use crate::shell::workspace::Workspace;

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1200.), px(760.)),
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
fn the_task_view_prepares_its_tabs_once_per_change(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
        let task = view.read(cx).task().clone();
        let store = view.read(cx).store().clone();
        let id = task.read(cx).detail().expect("a task is open").issue;
        let builds = |cx: &gpui_kit::App| task.read(cx).prepared_builds.get();
        let first = builds(cx);
        assert!(first >= 1, "the open task built its tabs");

        // Scrolling renders every frame, and builds nothing.
        for _ in 0..5 {
            task.update(cx, |t, cx| t.scroll_lines(true, window, cx));
            window.render_frame(cx);
        }
        assert_eq!(builds(cx), first);

        // New text builds the tabs again. LLM-999 is not a task yet, so it
        // stays text.
        store.update(cx, |s, _| {
            s.modify(id, |i| i.description = "See LLM-999 first.".into());
        });
        task.update(cx, |_, cx| cx.notify());
        window.render_frame(cx);
        assert_eq!(builds(cx), first + 1);
        assert_eq!(
            task.read(cx).prepared_text(cx)[0].as_ref(),
            "See LLM-999 first."
        );

        // A task that brings the key makes it a link.
        let mut new = store.read(cx).get(id).unwrap().clone();
        new.id = IssueId::from_u128(999_999);
        new.key = "LLM-999".into();
        store.update(cx, |s, _| {
            s.upsert(new);
        });
        task.update(cx, |_, cx| cx.notify());
        window.render_frame(cx);
        assert_eq!(builds(cx), first + 2);
        assert!(
            task.read(cx).prepared_text(cx)[0].contains("(llmtodo://issue/LLM-999)"),
            "{:?}",
            task.read(cx).prepared_text(cx)
        );

        // A change to another field of the task keeps the text.
        store.update(cx, |s, _| {
            s.modify(id, |i| i.title = "A new title".into());
        });
        task.update(cx, |_, cx| cx.notify());
        window.render_frame(cx);
        assert_eq!(builds(cx), first + 2);

        // New history adds its tabs.
        task.update(cx, |t, cx| {
            t.set_history(
                id,
                "- created".into(),
                String::new(),
                Default::default(),
                cx,
            )
        });
        window.render_frame(cx);
        assert_eq!(builds(cx), first + 3);
        let (_, tabs) = task.read(cx).open_tab(cx).unwrap();
        assert!(
            tabs.iter()
                .any(|t| t.title == crate::task::history::ACTIVITY_TAB)
        );
    })
    .unwrap();
}

/// A store holding the demo issues.
fn demo_store() -> IssueStore {
    let catalog = crate::model::demo::catalog();
    let mut store = IssueStore::new();
    store.replace_all(crate::model::demo::issues(&catalog));
    store
}

#[test]
fn by_key_follows_every_kind_of_write() {
    let mut store = demo_store();
    let issue = store.iter().next().unwrap().clone();
    let (id, key) = (issue.id, issue.key.clone());
    assert_eq!(store.by_key(&key).map(|i| i.id), Some(id));
    let revision = store.keys_revision();

    // The same snapshot again keeps the revision.
    let all: Vec<_> = store.iter().cloned().collect();
    store.replace_all(all.clone());
    assert_eq!(store.keys_revision(), revision);

    // A team move frees the old key.
    let mut moved = issue.clone();
    moved.key = "ABC-1".into();
    store.upsert(moved);
    assert!(store.by_key(&key).is_none());
    assert_eq!(store.by_key("ABC-1").map(|i| i.id), Some(id));
    assert!(store.keys_revision() > revision);

    // So does a change in place.
    let revision = store.keys_revision();
    store.modify(id, |i| i.key = key.clone());
    assert!(store.by_key("ABC-1").is_none());
    assert_eq!(store.by_key(&key).map(|i| i.id), Some(id));
    assert!(store.keys_revision() > revision);

    // A snapshot without the issue drops its key.
    let revision = store.keys_revision();
    store.replace_all(all.into_iter().filter(|i| i.id != id).collect());
    assert!(store.by_key(&key).is_none());
    assert!(store.keys_revision() > revision);
    for i in store.iter() {
        assert_eq!(store.by_key(&i.key).map(|f| f.id), Some(i.id));
    }
}

#[test]
fn the_claim_count_follows_every_kind_of_write() {
    let mut store = demo_store();
    let real = |store: &IssueStore| {
        store
            .iter()
            .filter(|i| !i.deleted && i.claim.is_some())
            .count()
    };
    let claimed = store.iter().find(|i| i.claim.is_some()).unwrap().clone();
    let free = store
        .iter()
        .find(|i| i.claim.is_none() && !i.deleted)
        .unwrap()
        .id;
    assert!(real(&store) > 0, "the demo set holds a claim");
    assert_eq!(store.claim_count(), real(&store));

    store.modify(claimed.id, |i| i.claim = None);
    assert_eq!(store.claim_count(), real(&store));
    store.modify(free, |i| i.claim = claimed.claim.clone());
    assert_eq!(store.claim_count(), real(&store));
    store.modify(free, |i| i.deleted = true);
    assert_eq!(store.claim_count(), real(&store));

    store.upsert(claimed.clone());
    assert_eq!(store.claim_count(), real(&store));
    let mut copy = claimed.clone();
    copy.claim = None;
    store.upsert(copy);
    assert_eq!(store.claim_count(), real(&store));

    store.replace_all(vec![claimed.clone(), claimed]);
    assert_eq!(store.claim_count(), 1);
}
