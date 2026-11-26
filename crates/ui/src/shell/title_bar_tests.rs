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

/// LLM-592: a preview build fills the title bar with yellow,
/// names the ticket, and never offers Restart to update.
#[gpui_kit::test]
fn a_preview_build_has_a_yellow_bar_with_its_ticket(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("llmtodo.app");
    std::fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
    std::fs::write(bundle.join(llmtodo_core::bundle::COMMIT_FILE), "newer").unwrap();
    let watch = crate::model::update::UpdateWatch {
        bundle,
        own: "own".into(),
    };
    view.update(cx, |this, cx| {
        this.watch_for_updates(watch, dir.path().to_path_buf(), None, cx);
    });
    cx.run_until_parked();
    assert!(cx.read(|cx| view.read(cx).update_button_shown()));

    // Without a preview, the bar is not yellow.
    let warning = cx
        .read(|cx| crate::shell::theme::preview_bar(gpui_kit::component::ActiveTheme::theme(cx)).0);
    // The bar is the full-width quad at the top of the window.
    let has_warning_fill = |window: &mut gpui_kit::Window| {
        let width = f32::from(window.viewport_size().width) * window.scale_factor();
        window.painted_quads().iter().any(|q| {
            q.background.as_solid() == Some(warning)
                && q.bounds.origin.y.0 <= 1.
                && q.bounds.size.width.0 >= width - 1.
        })
    };
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("title-preview").is_none());
        assert!(!has_warning_fill(window));
    })
    .unwrap();

    view.update(cx, |this, cx| {
        this.set_preview_ticket(Some("LLM-123".into()), cx)
    });
    cx.run_until_parked();
    assert!(!cx.read(|cx| view.read(cx).update_button_shown()));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let label = window.find("title-preview");
        assert!(label.bounds().size.width > gpui_kit::px(0.));
        assert!(window.try_find("restart-update").is_none());
        assert!(has_warning_fill(window));
    })
    .unwrap();
}

/// LLM-592, agy round 1: on a preview bar the repo of a project reads as
/// bar text. The link color, which can vanish on yellow, draws nowhere in
/// the bar.
#[gpui_kit::test]
fn a_preview_bar_draws_the_repo_in_its_text_color(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    view.update(cx, |this, cx| {
        this.set_scope(crate::model::scope::Scope::Project("app".into()), cx)
    });
    cx.run_until_parked();
    let link = cx.read(|cx| gpui_kit::component::ActiveTheme::theme(cx).link);
    let bar_link_underlines = |window: &mut gpui_kit::Window| {
        let bar = window
            .find("project-repo")
            .bounds()
            .scale(window.scale_factor());
        window
            .painted_underlines()
            .iter()
            .filter(|u| u.bounds.intersects(&bar) && u.color.h == link.h && u.color.s == link.s)
            .count()
    };
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            bar_link_underlines(window) > 0,
            "the real app links the repo"
        );
    })
    .unwrap();

    view.update(cx, |this, cx| {
        this.set_preview_ticket(Some("LLM-123".into()), cx)
    });
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("project-repo-link").is_some());
        assert_eq!(bar_link_underlines(window), 0, "no link color on yellow");
    })
    .unwrap();
}
