//! The filter panel (LLM-247). A click on the filter button opens it at
//! the right of the list. The list stays in view and takes clicks, and
//! each click in the panel filters it at once.
//!
//! - Each row turns one filter word on or off in the `/` filter text, and
//!   reads its check from that text. Typed words and clicks always agree,
//!   because the text is the only place the filter lives.
//! - Save filter… makes a saved view, as `:view save` does.
//! - `j` / `k` and `Ctrl-N` / `Ctrl-P` move, Enter or Space toggles the
//!   row, and Esc closes the panel.
//!
//! GPUI Kit's Sheet covers the whole window with a layer that takes every
//! click and keeps focus inside the sheet. So the panel is a plain view in
//! the workspace layout instead.

use std::collections::BTreeMap;

use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, Theme, button::Button,
    button::ButtonVariants as _, h_flex, v_flex,
};
use gpui_kit::{
    App, Context, Div, ElementId, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    KeyBinding, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, actions, div, prelude::FluentBuilder as _, rems,
};

use crate::issues::filter::{self, Filter, has_word, toggle_word};
use crate::issues::list::IssueList;
use crate::model::data::{Assignee, Catalog, Issue, LabelColor, OWNER_NAME, priority_name};
use crate::model::scope::Scope;
use crate::model::store::IssueStore;
use crate::shell::icons::HasIcon as _;
use crate::shell::tooltip::{Card, with_card};
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
    /// The filter word the entry stands for, such as `label:bug`.
    pub word: String,
    pub text: String,
}

/// What the filter menu shows for one filter text.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterPanel {
    pub statuses: Vec<Entry>,
    pub priorities: Vec<Entry>,
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
            word,
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

        let priorities = crate::shell::icons::PRIORITIES
            .into_iter()
            .map(|priority| {
                let ours = |v: &str| filter::priority_word(v) == Some(priority);
                entry(
                    priority_name(priority).into(),
                    priority.icon(),
                    "priority",
                    &ours,
                    format!("priority:{}", priority.as_str()),
                    false,
                )
            })
            .collect();

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
            priorities,
            labels,
            assignees,
            llm,
            clear: (Filter::parse(text).word_count() > 0).then(|| filter::clear_words(text)),
        }
    }
}

/// Whether `:view save` can save from `scope`.
pub(crate) fn saves_from(scope: &Scope) -> bool {
    matches!(scope, Scope::All | Scope::Team(_) | Scope::Project(_))
}

actions!(
    filter_panel,
    [
        FilterPanelDown,
        FilterPanelUp,
        FilterPanelToggle,
        FilterPanelClose
    ]
);

pub const CONTEXT: &str = "FilterPanel";

/// The panel's width, in rem.
pub const PANEL_REM: f32 = 15.;

pub fn bind_keys(cx: &mut App) {
    let panel = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("j", FilterPanelDown, panel),
        KeyBinding::new("down", FilterPanelDown, panel),
        KeyBinding::new("ctrl-n", FilterPanelDown, panel),
        KeyBinding::new("k", FilterPanelUp, panel),
        KeyBinding::new("up", FilterPanelUp, panel),
        KeyBinding::new("ctrl-p", FilterPanelUp, panel),
        KeyBinding::new("enter", FilterPanelToggle, panel),
        KeyBinding::new("space", FilterPanelToggle, panel),
        KeyBinding::new("escape", FilterPanelClose, panel),
        KeyBinding::new("ctrl-c", FilterPanelClose, panel),
    ]);
}

/// The sections of the panel, top to bottom.
const SECTIONS: [&str; 5] = ["Status", "Priority", "Label", "Assignee", "LLM"];

impl FilterPanel {
    /// The entries of one section, by its place in `SECTIONS`.
    fn section(&self, ix: usize) -> &[Entry] {
        match ix {
            0 => &self.statuses,
            1 => &self.priorities,
            2 => &self.labels,
            3 => &self.assignees,
            _ => &self.llm,
        }
    }

    /// Every entry, in the order the panel shows them.
    pub fn rows(&self) -> impl Iterator<Item = &Entry> {
        (0..SECTIONS.len()).flat_map(|ix| self.section(ix))
    }
}

/// A built panel, with the filter text and store revision it came from.
type PanelMemo = ((String, u64), std::rc::Rc<FilterPanel>);

/// The panel view. It holds only view state: whether it is open and which
/// row the keys point at. The filter is the list's `/` filter text.
pub struct FilterPanelView {
    store: Entity<IssueStore>,
    list: Entity<IssueList>,
    events: ViewEvents,
    pub focus: FocusHandle,
    open: bool,
    /// The row the keys point at, as an index into `FilterPanel::rows`.
    highlight: usize,
    /// The rows' hover fills, drawn under this view (LLM-345).
    pub fill_layer: Entity<crate::shell::motion::FillLayer>,
    /// The last panel, by the filter text and the store revision it was
    /// built from, so a render or an arrow key does not walk every issue
    /// (LLM-345).
    memo: std::cell::RefCell<Option<PanelMemo>>,
    /// Whether the list's scope could save a view, as last drawn.
    saves: bool,
    /// How often the panel walked the issues, for tests.
    #[cfg(test)]
    pub builds: std::cell::Cell<usize>,
    _subscriptions: Vec<Subscription>,
}

impl FilterPanelView {
    pub fn new(
        store: Entity<IssueStore>,
        list: Entity<IssueList>,
        events: ViewEvents,
        cx: &mut Context<Self>,
    ) -> Self {
        // A typed word, a new scope or a new label changes the panel.
        let filter = list.read(cx).filter.clone();
        // Each draws the panel only when what it shows changed, so a cursor
        // blink in the filter or a selection move in the list does not.
        let subscriptions = vec![
            cx.observe(&filter, |this: &mut Self, _, cx| this.notify_if_changed(cx)),
            cx.observe(&list, |this: &mut Self, list, cx| {
                let saves = saves_from(&list.read(cx).scope);
                if saves != this.saves {
                    this.saves = saves;
                    cx.notify();
                }
            }),
            cx.observe(&store, |this: &mut Self, _, cx| this.notify_if_changed(cx)),
        ];
        let saves = saves_from(&list.read(cx).scope);
        Self {
            store,
            list,
            events,
            focus: cx.focus_handle(),
            open: false,
            highlight: 0,
            fill_layer: gpui_kit::AppContext::new(cx, |_| Default::default()),
            memo: Default::default(),
            saves,
            #[cfg(test)]
            builds: Default::default(),
            _subscriptions: subscriptions,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.open != open {
            self.open = open;
            self.highlight = 0;
            cx.notify();
        }
    }

    /// What the panel shows for the filter text now. Built again only
    /// when the text or the store changed.
    pub fn panel(&self, cx: &App) -> std::rc::Rc<FilterPanel> {
        let store = self.store.read(cx);
        let filter = self.list.read(cx).filter.read(cx);
        let text = filter.value();
        let revision = store.revision();
        if let Some(((t, r), panel)) = &*self.memo.borrow()
            && *r == revision
            && t.as_str() == text.as_ref()
        {
            return panel.clone();
        }
        #[cfg(test)]
        self.builds.set(self.builds.get() + 1);
        let panel = std::rc::Rc::new(FilterPanel::new(&text, &store.catalog, store.iter()));
        *self.memo.borrow_mut() = Some(((text.to_string(), revision), panel.clone()));
        panel
    }

    /// Draws the panel again when the filter text or the store changed
    /// what it shows. A closed panel draws nothing, so it waits.
    fn notify_if_changed(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        let before = self.memo.borrow().as_ref().map(|(_, p)| p.clone());
        let now = self.panel(cx);
        if before.is_none_or(|before| !std::rc::Rc::ptr_eq(&before, &now) && *before != *now) {
            cx.notify();
        }
    }

    /// The row the keys point at, for tests.
    pub fn highlighted(&self, cx: &App) -> Option<SharedString> {
        self.panel(cx)
            .rows()
            .nth(self.highlight)
            .map(|e| e.label.clone())
    }

    fn step(&mut self, down: bool, cx: &mut Context<Self>) {
        let count = self.panel(cx).rows().count();
        if count == 0 {
            return;
        }
        self.highlight = if down {
            (self.highlight + 1) % count
        } else {
            (self.highlight + count - 1) % count
        };
        cx.notify();
    }

    fn down(&mut self, _: &FilterPanelDown, _: &mut Window, cx: &mut Context<Self>) {
        self.step(true, cx);
    }

    fn up(&mut self, _: &FilterPanelUp, _: &mut Window, cx: &mut Context<Self>) {
        self.step(false, cx);
    }

    /// Emits after this update, because the workspace updates this view
    /// when it runs some events.
    fn emit_later(&self, event: ViewEvent, window: &mut Window, cx: &mut Context<Self>) {
        let events = self.events.clone();
        window.defer(cx, move |window, cx| events.emit(event, window, cx));
    }

    fn toggle(&mut self, _: &FilterPanelToggle, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(entry) = self.panel(cx).rows().nth(self.highlight) {
            self.emit_later(ViewEvent::SetFilter(entry.text.clone()), window, cx);
        }
    }

    fn close(&mut self, _: &FilterPanelClose, window: &mut Window, cx: &mut Context<Self>) {
        self.emit_later(ViewEvent::CloseFilterPanel, window, cx);
    }
}

/// The square that shows a row's check. Monochrome, as the owner asks.
fn check_box(checked: bool, theme: &Theme) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(rems(0.875))
        .rounded(theme.radius.min(gpui_kit::px(3.)))
        .border_1()
        .border_color(if checked {
            theme.foreground
        } else {
            theme.muted_foreground.opacity(0.6)
        })
        .when(checked, |this| {
            this.bg(theme.foreground).child(
                Icon::new(IconName::Check)
                    .xsmall()
                    .text_color(theme.background),
            )
        })
}

impl FilterPanelView {
    fn render_row(
        &self,
        section: &str,
        entry: &Entry,
        ix: usize,
        highlighted: bool,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let name = format!("{section}-{}", entry.label);
        let frame = crate::shell::motion::fill_under(
            h_flex().id(ElementId::Name(format!("filter-row-{name}").into())),
            ElementId::Name(format!("filter-row-fill-{name}").into()),
            crate::shell::motion::Fill {
                rest: highlighted.then(|| crate::shell::theme::selection_bg(theme, true)),
                hover: if highlighted {
                    crate::shell::theme::selection_hover(theme)
                } else {
                    theme.list_hover
                },
                pressed: crate::shell::theme::row_pressed(theme),
            },
            theme.radius,
            Some(&self.fill_layer),
            window,
            cx,
        );
        let icon = Icon::new(entry.icon).small();
        let icon = match entry.color {
            Some(color) => icon.text_color(label_color(color, cx)),
            None => icon.text_color(theme.muted_foreground),
        };
        let text = entry.text.clone();
        let events = self.events.clone();
        let me = cx.entity().downgrade();
        let row = frame
            .test_support()
            .w_full()
            .h_7()
            .flex_none()
            .px_2()
            .gap_2()
            .text_sm()
            .rounded(theme.radius)
            .cursor_pointer()
            .child(check_box(entry.checked, theme))
            .child(icon)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(entry.label.clone()),
            )
            .on_click(move |_, window, cx| {
                me.update(cx, |this, cx| {
                    this.highlight = ix;
                    cx.notify();
                })
                .ok();
                events.emit(ViewEvent::SetFilter(text.clone()), window, cx);
            });
        with_card(
            ElementId::Name(format!("filter-row-tip-{name}").into()),
            row,
            Card::text(entry.word.clone()),
        )
    }
}

impl Render for FilterPanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::render_count::bump::<Self>();
        self.fill_layer.update(cx, |layer, _| layer.start_frame());
        let panel = self.panel(cx);
        let theme = cx.theme().clone();
        let focused = self.focus.contains_focused(window, cx);

        let mut body = v_flex()
            .id("filter-side-panel-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_1()
            .gap_px();
        let mut ix = 0;
        for (section_ix, title) in SECTIONS.iter().enumerate() {
            let entries = panel.section(section_ix);
            if entries.is_empty() {
                continue;
            }
            let on = entries.iter().filter(|e| e.checked).count();
            body = body.child(
                h_flex()
                    .px_2()
                    .pt_2()
                    .pb_1()
                    .gap_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(*title)
                    .when(on > 0, |this| this.child(format!("· {on}"))),
            );
            for entry in entries {
                let highlighted = focused && ix == self.highlight;
                body =
                    body.child(self.render_row(title, entry, ix, highlighted, &theme, window, cx));
                ix += 1;
            }
        }

        let close_events = self.events.clone();
        let header = h_flex()
            .flex_none()
            .h(crate::shell::widgets::top_bar_height(window))
            .pl_3()
            .pr_2()
            .justify_between()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("Filters"),
            )
            .child(with_card(
                "filter-side-panel-close-tip",
                Button::new("filter-side-panel-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::X)
                    .on_click(move |_, window, cx| {
                        close_events.emit(ViewEvent::CloseFilterPanel, window, cx);
                    }),
                Card::text("Close (Esc)"),
            ));

        let clear = panel.clear.clone();
        let clear_events = self.events.clone();
        let save_events = self.events.clone();
        let footer = v_flex()
            .flex_none()
            .p_1()
            .gap_px()
            .border_t_1()
            .border_color(theme.border)
            .child(with_card(
                "clear-filters-tip",
                Button::new("clear-filters")
                    .ghost()
                    .small()
                    .w_full()
                    .justify_start()
                    .icon(IconName::X)
                    .label("Clear filters")
                    .disabled(clear.is_none())
                    .on_click(move |_, window, cx| {
                        if let Some(text) = clear.clone() {
                            clear_events.emit(ViewEvent::SetFilter(text), window, cx);
                        }
                    }),
                Card::text("Remove the filter words, keep the typed text"),
            ))
            .child(with_card(
                "save-filter-tip",
                Button::new("save-filter")
                    .ghost()
                    .small()
                    .w_full()
                    .justify_start()
                    .icon(IconName::Bookmark)
                    .label("Save filter…")
                    .disabled(!saves_from(&self.list.read(cx).scope))
                    .on_click(move |_, window, cx| {
                        // The palette asks for the name, as `:view save` does.
                        save_events.emit(
                            ViewEvent::OpenPaletteWith("view save ".into()),
                            window,
                            cx,
                        );
                    }),
                Card::text("Save as a view (:view save <name>)"),
            ));

        v_flex()
            .id("filter-side-panel")
            .test_support()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::toggle))
            .on_action(cx.listener(Self::close))
            .w(rems(PANEL_REM))
            .h_full()
            .flex_none()
            // The workspace draws the background, under the rows' hover
            // fills (LLM-345).
            .border_l_1()
            .border_color(theme.border)
            .child(header)
            .child(body)
            .child(footer)
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
    fn save_works_from_a_place_not_a_view() {
        assert!(saves_from(&Scope::Project("app".into())));
        assert!(saves_from(&Scope::All));
        assert!(!saves_from(&Scope::View("Bugs".into())));
    }

    #[test]
    fn rows_run_through_every_section_in_order() {
        let p = panel("");
        let rows: Vec<&str> = p.rows().map(|e| e.label.as_ref()).collect();
        assert_eq!(rows[0], "Open");
        assert_eq!(
            rows.len(),
            p.statuses.len()
                + p.priorities.len()
                + p.labels.len()
                + p.assignees.len()
                + p.llm.len()
        );
        assert_eq!(*rows.last().unwrap(), "Analyzing");
        assert_eq!(find(&p.labels, "bug").word, "label:bug");
    }
}
