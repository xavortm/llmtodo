//! The right-click menu for a task.

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, h_flex,
    menu::{PopupMenu, PopupMenuItem},
};
use gpui_kit::{Context, Entity, ParentElement as _, Styled as _, Window};

use crate::model::data::{IssueId, WaitingOn};
use crate::model::store::IssueStore;
use crate::shell::icons::HasIcon as _;
use crate::shell::view_events::{IssueAction, ViewEvent, ViewEvents};

/// Every item also has a palette command,
/// so the menu is never the only way.
///
/// The menu holds the task's ID. A snapshot can reorder the list while the
/// menu is open, so each item finds the task again when it is clicked.
pub fn issue_menu(
    store: Entity<IssueStore>,
    events: ViewEvents,
    id: IssueId,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, window, cx| {
        let Some(issue) = store.read(cx).get(id).cloned() else {
            return menu;
        };
        let status_events = events.clone();
        let label_events = events.clone();
        let copy_events = events.clone();
        let edit_events = events.clone();
        let delete_events = events.clone();
        let deleted = issue.deleted;
        let current_status = issue.status.clone();
        let current_labels = issue.labels.clone();
        let wait_events = events.clone();
        let current_waiting = issue.waiting_on;
        let statuses = store.read(cx).catalog.team_statuses(&issue.team).to_vec();
        let labels = store.read(cx).catalog.labels.clone();
        // A ticket in a review status gets Approve at the top, for quick
        // access. An approved ticket that waits on an agent to merge its PR
        // has its answer already.
        let approvable =
            issue.status.review && !deleted && issue.waiting_on != Some(WaitingOn::Agent);
        let approve_events = events.clone();
        let menu = if approvable {
            menu.item(
                PopupMenuItem::new("Approve")
                    .icon(Icon::new(IconName::Check))
                    .on_click(move |_, window, cx| {
                        approve_events.emit(
                            ViewEvent::AnswerReview {
                                issue: id,
                                verdict: llmtodo_api_types::ReviewVerdict::Approved,
                            },
                            window,
                            cx,
                        );
                    }),
            )
            .separator()
        } else {
            menu
        };
        let menu = menu
            .submenu("Status", window, cx, move |menu, _, _| {
                statuses.iter().cloned().fold(menu, |menu, status| {
                    let events = status_events.clone();
                    menu.item(
                        PopupMenuItem::new(status.name.clone())
                            .icon(Icon::new(status.icon()))
                            .checked(status == current_status)
                            .on_click(move |_, window, cx| {
                                let status = status.clone();
                                events.emit(
                                    ViewEvent::OnIssue(id, IssueAction::SetStatus(status)),
                                    window,
                                    cx,
                                );
                            }),
                    )
                })
            })
            .submenu("Labels", window, cx, move |menu, _, _| {
                labels
                    .iter()
                    .cloned()
                    .fold(menu.scrollable(true), |menu, label| {
                        let events = label_events.clone();
                        menu.item(
                            PopupMenuItem::new(label.name.clone())
                                .checked(current_labels.contains(&label))
                                .on_click(move |_, window, cx| {
                                    events.emit(
                                        ViewEvent::OnIssue(
                                            id,
                                            IssueAction::ToggleLabel(label.clone()),
                                        ),
                                        window,
                                        cx,
                                    );
                                }),
                        )
                    })
            })
            .submenu("Waiting on", window, cx, move |menu, _, _| {
                [
                    (Some(WaitingOn::Agent), "An agent", IconName::Bot),
                    (Some(WaitingOn::Owner), "You", IconName::User),
                    (None, "No one", IconName::Minus),
                ]
                .into_iter()
                .fold(menu, |menu, (waiting, name, icon)| {
                    let events = wait_events.clone();
                    menu.item(
                        PopupMenuItem::new(name)
                            .icon(Icon::new(icon))
                            .checked(waiting == current_waiting)
                            .on_click(move |_, window, cx| {
                                events.emit(
                                    ViewEvent::OnIssue(id, IssueAction::SetWaitingOn(waiting)),
                                    window,
                                    cx,
                                );
                            }),
                    )
                })
            })
            .separator()
            .item(
                PopupMenuItem::new("Copy ID")
                    .icon(Icon::new(IconName::Copy))
                    .on_click(move |_, window, cx| {
                        copy_events.emit(ViewEvent::OnIssue(id, IssueAction::CopyKey), window, cx);
                    }),
            )
            .item(
                PopupMenuItem::new("Edit description")
                    .icon(Icon::new(IconName::SquarePen))
                    .on_click(move |_, window, cx| {
                        edit_events.emit(
                            ViewEvent::OnIssue(id, IssueAction::EditDescription),
                            window,
                            cx,
                        );
                    }),
            )
            .separator();
        if deleted {
            return menu.item(
                PopupMenuItem::new("Restore")
                    .icon(Icon::new(IconName::Undo2))
                    .on_click(move |_, window, cx| {
                        delete_events.emit(
                            ViewEvent::OnIssue(id, IssueAction::Restore),
                            window,
                            cx,
                        );
                    }),
            );
        }
        menu.item(
            // The one destructive item, so it is red.
            PopupMenuItem::element(|_, cx| {
                let danger = cx.theme().danger;
                h_flex()
                    .gap_2()
                    .text_color(danger)
                    .child(Icon::new(IconName::Trash).small().text_color(danger))
                    .child("Move to Trash")
            })
            .on_click(move |_, window, cx| {
                delete_events.emit(ViewEvent::OnIssue(id, IssueAction::Delete), window, cx);
            }),
        )
    }
}
