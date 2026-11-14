//! The two menus in the filter bar (LLM-247).
//!
//! - The filter button opens every way to filter. Each item turns one
//!   filter word on or off in the `/` filter text, and reads its check
//!   from that text. Typed words and clicks always agree, because the text
//!   is the only place the filter lives.
//! - The saved filter button lists the server's saved views. A saved
//!   filter is a saved view: picking one sets `Scope::View`, as `:view`
//!   and the sidebar do.

use std::collections::BTreeMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Icon,
    menu::{PopupMenu, PopupMenuItem},
};
use gpui_kit::{App, Context, Entity, SharedString, Styled as _, Window};

use crate::issues::filter::{self, Filter, has_word, toggle_word};
use crate::model::data::{Assignee, Catalog, Issue, LabelColor, OWNER_NAME};
use crate::model::scope::Scope;
use crate::model::store::IssueStore;
use crate::shell::icons::HasIcon as _;
use crate::shell::view_events::{ViewEvent, ViewEvents};
use crate::shell::widgets::label_color;

/// One item of the filter menu: its check, and the filter text a click
/// sets.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub label: SharedString,
    pub icon: IconName,
    /// A label's color, for its icon. Every other icon stays monochrome.
    pub color: Option<LabelColor>,
    pub checked: bool,
    pub text: String,
}

/// What the filter menu shows for one filter text.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterPanel {
    pub statuses: Vec<Entry>,
    pub labels: Vec<Entry>,
    pub assignees: Vec<Entry>,
    pub llm: Vec<Entry>,
    /// The text with only its free text, when it has filter words.
    pub clear: Option<String>,
}

impl FilterPanel {
    pub fn new<'a>(text: &str, catalog: &Catalog, issues: impl Iterator<Item = &'a Issue>) -> Self {
        let entry = |label: SharedString,
                     icon: IconName,
                     key: &str,
                     value: &dyn Fn(&str) -> bool,
                     word: String,
                     single: bool| Entry {
            label,
            icon,
            color: None,
            checked: has_word(text, key, value),
            text: toggle_word(text, key, value, &word, single),
        };

        let mut statuses = vec![entry(
            "Open".into(),
            IconName::CircleDashed,
            "status",
            &|v| v == "open",
            "status:open".into(),
            false,
        )];
        let mut seen: Vec<String> = Vec::new();
        for status in catalog.all_statuses() {
            let word = filter::status_word(&status);
            if seen.contains(&word) {
                continue;
            }
            seen.push(word.clone());
            let kept = |v: &str| v != "open" && filter::status_kept(&status, v);
            statuses.push(entry(
                status.name.clone(),
                status.icon(),
                "status",
                &kept,
                word,
                false,
            ));
        }

        // A filter word holds no spaces, so a label with one cannot be a
        // word. The panel leaves it out.
        let labels = catalog
            .labels
            .iter()
            .filter(|l| !l.name.contains(char::is_whitespace))
            .map(|label| {
                let ours = |v: &str| catalog.find_label(v).is_some_and(|l| l.name == label.name);
                Entry {
                    color: Some(label.color),
                    ..entry(
                        label.name.clone(),
                        IconName::Tag,
                        "label",
                        &ours,
                        format!("label:{}", label.name.to_lowercase()),
                        false,
                    )
                }
            })
            .collect();

        let mut assignees = vec![
            entry(
                "Me".into(),
                IconName::User,
                "assignee",
                &|v| v == OWNER_NAME,
                format!("assignee:{OWNER_NAME}"),
                true,
            ),
            entry(
                "No one".into(),
                IconName::Minus,
                "assignee",
                &|v| v == llmtodo_api_types::ASSIGNEE_NONE,
                format!("assignee:{}", llmtodo_api_types::ASSIGNEE_NONE),
                true,
            ),
        ];
        // Each other actor once, by name.
        let mut actors: BTreeMap<String, (SharedString, IconName)> = BTreeMap::new();
        for issue in issues {
            let Some(assignee) = &issue.assignee else {
                continue;
            };
            let name = match assignee {
                Assignee::Person(name) => name.clone(),
                Assignee::Agent(agent) => agent.name.clone(),
            };
            if name.as_ref() == OWNER_NAME || name.contains(char::is_whitespace) {
                continue;
            }
            actors
                .entry(name.to_lowercase())
                .or_insert((name, assignee.icon()));
        }
        for (word, (name, icon)) in actors {
            let ours = |v: &str| v == word;
            assignees.push(entry(
                name,
                icon,
                "assignee",
                &ours,
                format!("assignee:{word}"),
                true,
            ));
        }

        let llm = [
            ("Any agent on it", IconName::Bot, "any"),
            ("Locked", IconName::Lock, "locked"),
            ("Analyzing", IconName::Eye, "analyzing"),
        ]
        .into_iter()
        .map(|(label, icon, value)| {
            entry(
                label.into(),
                icon,
                "llm",
                &|v| v == value,
                format!("llm:{value}"),
                true,
            )
        })
        .collect();

        Self {
            statuses,
            labels,
            assignees,
            llm,
            clear: (Filter::parse(text).word_count() > 0).then(|| filter::clear_words(text)),
        }
    }
}

fn entry_item(entry: &Entry, events: &ViewEvents, cx: &App) -> PopupMenuItem {
    let icon = Icon::new(entry.icon);
    let icon = match entry.color {
        Some(color) => icon.text_color(label_color(color, cx)),
        None => icon,
    };
    let text = entry.text.clone();
    let events = events.clone();
    PopupMenuItem::new(entry.label.clone())
        .icon(icon)
        .checked(entry.checked)
        .on_click(move |_, window, cx| {
            events.emit(ViewEvent::SetFilter(text.clone()), window, cx);
        })
}

fn section(
    menu: PopupMenu,
    title: &'static str,
    icon: IconName,
    entries: Vec<Entry>,
    events: ViewEvents,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let on = entries.iter().filter(|e| e.checked).count();
    let label = match on {
        0 => title.to_string(),
        n => format!("{title} ({n})"),
    };
    let scroll = entries.len() > 12;
    menu.submenu_with_icon(
        Some(Icon::new(icon)),
        label,
        window,
        cx,
        move |menu, _, cx| {
            entries.iter().fold(menu.scrollable(scroll), |menu, e| {
                menu.item(entry_item(e, &events, cx))
            })
        },
    )
}

/// Whether `:view save` can save from `scope`.
fn saves_from(scope: &Scope) -> bool {
    matches!(scope, Scope::All | Scope::Team(_) | Scope::Project(_))
}

/// The filter button's menu. It reads the filter text when it opens.
pub fn filter_menu(
    store: Entity<IssueStore>,
    input: Entity<gpui_kit::component::input::InputState>,
    scope: Scope,
    events: ViewEvents,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, window, cx| {
        let text = input.read(cx).value().to_string();
        let store = store.read(cx);
        let panel = FilterPanel::new(&text, &store.catalog, store.iter());
        let menu = section(
            menu,
            "Status",
            IconName::CircleDot,
            panel.statuses,
            events.clone(),
            window,
            cx,
        );
        let menu = section(
            menu,
            "Label",
            IconName::Tag,
            panel.labels,
            events.clone(),
            window,
            cx,
        );
        let menu = section(
            menu,
            "Assignee",
            IconName::User,
            panel.assignees,
            events.clone(),
            window,
            cx,
        );
        let menu = section(
            menu,
            "LLM",
            IconName::Bot,
            panel.llm,
            events.clone(),
            window,
            cx,
        );
        let clear_events = events.clone();
        let save_events = events.clone();
        menu.separator()
            .item(
                PopupMenuItem::new("Clear filters")
                    .icon(Icon::new(IconName::X))
                    .disabled(panel.clear.is_none())
                    .on_click(move |_, window, cx| {
                        if let Some(text) = panel.clear.clone() {
                            clear_events.emit(ViewEvent::SetFilter(text), window, cx);
                        }
                    }),
            )
            .item(
                // The palette asks for the name, as `:view save` does.
                PopupMenuItem::new("Save filter…")
                    .icon(Icon::new(IconName::Bookmark))
                    .disabled(!saves_from(&scope))
                    .on_click(move |_, window, cx| {
                        save_events.emit(
                            ViewEvent::OpenPaletteWith("view save ".into()),
                            window,
                            cx,
                        );
                    }),
            )
    }
}

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

    fn panel(text: &str) -> FilterPanel {
        let catalog = crate::model::demo::catalog();
        let issues = crate::model::demo::issues(&catalog);
        FilterPanel::new(text, &catalog, issues.iter())
    }

    fn find<'a>(entries: &'a [Entry], label: &str) -> &'a Entry {
        entries
            .iter()
            .find(|e| e.label.as_ref() == label)
            .unwrap_or_else(|| panic!("no entry {label}"))
    }

    #[test]
    fn checks_come_from_the_typed_words() {
        let p = panel("crash Label:Bug status:in-progress assignee:me llm:locked");
        assert!(find(&p.labels, "bug").checked);
        assert!(!find(&p.labels, "infra").checked);
        assert!(find(&p.statuses, "In Progress").checked);
        assert!(!find(&p.statuses, "Todo").checked);
        assert!(!find(&p.statuses, "Open").checked);
        assert!(find(&p.assignees, "Me").checked);
        assert!(!find(&p.assignees, "No one").checked);
        assert!(find(&p.llm, "Locked").checked);
        assert!(!find(&p.llm, "Any agent on it").checked);
        assert_eq!(p.clear.as_deref(), Some("crash"));
        assert_eq!(panel("crash").clear, None);
    }

    #[test]
    fn a_click_sets_the_text_with_the_word_toggled() {
        let p = panel("crash label:bug");
        assert_eq!(find(&p.labels, "bug").text, "crash");
        assert_eq!(find(&p.labels, "infra").text, "crash label:bug label:infra");
        assert_eq!(
            find(&p.statuses, "Open").text,
            "crash label:bug status:open"
        );
        assert_eq!(
            find(&p.statuses, "In Review").text,
            "crash label:bug status:in-review"
        );
        // One assignee and one LLM word at a time.
        let p = panel("assignee:me llm:any");
        assert_eq!(find(&p.assignees, "No one").text, "llm:any assignee:none");
        assert_eq!(find(&p.llm, "Locked").text, "assignee:me llm:locked");
        assert_eq!(find(&p.llm, "Any agent on it").text, "assignee:me");
    }

    #[test]
    fn the_text_a_click_sets_filters_like_typing_it() {
        // Every entry's text parses back to a filter with that entry
        // checked, unless the click turned it off.
        let p = panel("");
        for e in p
            .statuses
            .iter()
            .chain(&p.labels)
            .chain(&p.assignees)
            .chain(&p.llm)
        {
            assert!(!e.checked, "{}", e.label);
            let after = panel(&e.text);
            let again = after
                .statuses
                .iter()
                .chain(&after.labels)
                .chain(&after.assignees)
                .chain(&after.llm)
                .find(|x| x.label == e.label)
                .unwrap();
            assert!(again.checked, "{} -> {}", e.label, e.text);
        }
    }

    #[test]
    fn assignees_list_each_actor_once() {
        let p = panel("");
        let names: Vec<&str> = p.assignees.iter().map(|e| e.label.as_ref()).collect();
        assert_eq!(&names[..2], ["Me", "No one"]);
        let mut rest = names[2..].to_vec();
        rest.dedup();
        assert_eq!(rest.len(), names.len() - 2);
        assert!(!names[2..].contains(&OWNER_NAME));
        // A status type word checks every status of that type.
        let p = panel("status:started");
        assert!(find(&p.statuses, "In Progress").checked);
        assert!(find(&p.statuses, "In Review").checked);
    }

    #[test]
    fn only_a_saved_view_scope_has_an_open_view() {
        assert_eq!(
            open_view(&Scope::View("Bugs".into())).as_deref(),
            Some("Bugs")
        );
        assert_eq!(open_view(&Scope::All), None);
        assert!(saves_from(&Scope::Project("app".into())));
        assert!(!saves_from(&Scope::View("Bugs".into())));
    }
}
