//! UI integration tests for the title bar's project name, repo and folder
//! (LLM-213).

use std::path::Path;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};

use crate::model::data::Catalog;
use crate::model::scope::Scope;
use crate::shell::workspace::{TitlePlace, Workspace, title_place};

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut this = Workspace::new(window, cx);
                this.set_project_dir(None);
                this.load_demo(cx);
                this
            })
        })
        .unwrap()
    })
}

fn place(scope: Scope, catalog: &Catalog) -> (String, TitlePlace) {
    let (name, place) = title_place(&scope, catalog, "Demo", None, Some("/Users/me"));
    (name.to_string(), place)
}

#[test]
fn a_project_shows_its_repo_else_its_folder() {
    let mut catalog = crate::model::demo::catalog();
    // App has a repo and a folder: the repo wins.
    catalog
        .project_folders
        .push(("app".into(), "/Users/me/dev/app".into()));
    assert_eq!(
        place(Scope::Project("app".into()), &catalog),
        (
            "App".into(),
            TitlePlace::Repo {
                label: "xavortm/llmtodo".into(),
                url: "https://github.com/xavortm/llmtodo".into(),
            }
        )
    );
    // Server has only a folder.
    catalog.project_folders = vec![("server".into(), "/Users/me/dev/server".into())];
    assert_eq!(
        place(Scope::Project("server".into()), &catalog),
        ("Server".into(), TitlePlace::Folder("~/dev/server".into()))
    );
    // A folder outside home stays whole, and `/Users/mel` is not home.
    catalog.project_folders = vec![("server".into(), "/Users/mel/server".into())];
    assert_eq!(
        place(Scope::Project("server".into()), &catalog).1,
        TitlePlace::Folder("/Users/mel/server".into())
    );
    // No repo and no folder: the name alone.
    catalog.project_folders.clear();
    assert_eq!(
        place(Scope::Project("server".into()), &catalog),
        ("Server".into(), TitlePlace::None)
    );
}

#[test]
fn a_scope_that_is_not_one_project_shows_the_tracker() {
    let catalog = crate::model::demo::catalog();
    for scope in [
        Scope::All,
        Scope::Team("LLM".into()),
        Scope::View("Open bugs".into()),
        Scope::Inbox,
        Scope::Trash,
        Scope::Settings,
        Scope::Project("gone".into()),
    ] {
        assert_eq!(
            place(scope.clone(), &catalog),
            ("Demo".into(), TitlePlace::None),
            "{scope:?}"
        );
    }
    // A project document counts as its project.
    let doc = catalog.documents[0].id.to_string();
    assert_eq!(place(Scope::Document(doc.into()), &catalog).0, "App");
}

#[test]
fn the_dev_folder_wins() {
    let catalog = crate::model::demo::catalog();
    let (name, place) = title_place(
        &Scope::Project("app".into()),
        &catalog,
        "Demo",
        Some(Path::new("/Users/me/dev/llmtodo")),
        Some("/Users/me"),
    );
    assert_eq!(name, "llmtodo");
    assert_eq!(place, TitlePlace::Folder("~/dev/llmtodo".into()));
}

/// The bar follows the scope: a link for the repo, a copy button for the
/// folder, and neither for all issues. A click on the link opens the repo.
#[gpui_kit::test]
fn the_title_bar_follows_the_scope(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let shown = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            (
                window.try_find("project-repo").is_some(),
                window.try_find("project-path").is_some(),
            )
        })
        .unwrap()
    };
    assert_eq!(shown(cx), (false, false));
    cx.read(|cx| assert_eq!(view.read(cx).title_place(cx).0, "llmtodo"));

    view.update(cx, |this, cx| {
        this.set_scope(Scope::Project("app".into()), cx)
    });
    assert_eq!(shown(cx), (true, false));
    cx.read(|cx| assert_eq!(view.read(cx).title_place(cx).0, "App"));
    cx.update_window(handle, |_, window, cx| window.click("project-repo", cx))
        .unwrap();
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://github.com/xavortm/llmtodo")
    );

    view.update(cx, |this, cx| {
        this.set_scope(Scope::Project("server".into()), cx)
    });
    assert_eq!(shown(cx), (false, true));
    cx.update_window(handle, |_, window, cx| window.click("project-path", cx))
        .unwrap();
    assert_eq!(
        cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("/tmp/llmtodo-demo/server")
    );

    view.update(cx, |this, cx| this.set_scope(Scope::All, cx));
    assert_eq!(shown(cx), (false, false));
}

/// LLM-196: the title bar draws the project's icon before its name. Other
/// scopes draw none.
#[gpui_kit::test]
fn the_title_bar_shows_the_project_icon(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let has_icon = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("title-project-icon").is_some()
        })
        .unwrap()
    };
    assert!(!has_icon(cx));
    view.update(cx, |this, cx| {
        this.set_scope(Scope::Project("app".into()), cx)
    });
    assert!(has_icon(cx));
}
