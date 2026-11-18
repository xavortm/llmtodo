//! The saved filter button's menu (LLM-247). It lists the server's saved
//! views. A saved filter is a saved view: picking one sets `Scope::View`,
//! as `:view` and the sidebar do. The filter button opens the filter
//! panel instead, in `filter_panel.rs`.

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Icon,
    menu::{PopupMenu, PopupMenuItem},
};
use gpui_kit::{Context, Entity, SharedString, Window};

use crate::model::scope::Scope;
use crate::model::store::IssueStore;
use crate::shell::view_events::{ViewEvent, ViewEvents};

/// The name of the saved view the list shows, if it shows one.
pub fn open_view(scope: &Scope) -> Option<SharedString> {
    match scope {
        Scope::View(name) => Some(name.clone()),
        _ => None,
    }
}

/// The saved filter button's menu: every saved view, the open one
/// checked, and a way out of it.
pub fn saved_filter_menu(
    store: Entity<IssueStore>,
    scope: Scope,
    events: ViewEvents,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, _, cx| {
        let open = open_view(&scope);
        let views = store.read(cx).catalog.views.clone();
        let menu = views.into_iter().fold(menu.scrollable(true), |menu, view| {
            let events = events.clone();
            let checked = open.as_ref() == Some(&view.key);
            menu.item(
                PopupMenuItem::new(view.name.clone())
                    .checked(checked)
                    .on_click(move |_, window, cx| {
                        events.emit(
                            ViewEvent::PickScope(Scope::View(view.key.clone())),
                            window,
                            cx,
                        );
                    }),
            )
        });
        let leave_events = events.clone();
        menu.separator().item(
            PopupMenuItem::new("Leave the view")
                .icon(Icon::new(IconName::X))
                .disabled(open.is_none())
                .on_click(move |_, window, cx| {
                    leave_events.emit(ViewEvent::PickScope(Scope::All), window, cx);
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_saved_view_scope_has_an_open_view() {
        assert_eq!(
            open_view(&Scope::View("Bugs".into())).as_deref(),
            Some("Bugs")
        );
        assert_eq!(open_view(&Scope::All), None);
    }
}
