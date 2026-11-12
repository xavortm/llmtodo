//! Small pieces the list and the task view both draw: issue keys, label
//! tags, the lock colors and the card that follows a drag.

use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    link::Link,
};
use gpui_kit::{
    AnyElement, App, Context, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, prelude::FluentBuilder as _,
};

use crate::model::data::{Issue, IssueId, LabelColor, WaitingOn};
use crate::shell::view_events::{IssueAction, ViewEvent, ViewEvents};

/// The fill and the text color of the `Locked` badge. The owner chose
/// color here, as an exception to the monochrome theme.
pub fn lock_colors(cx: &App) -> (gpui_kit::Hsla, gpui_kit::Hsla) {
    let theme = cx.theme();
    let fg = crate::shell::theme::ensure_contrast(theme.warning, &[theme.background], 5.0);
    (theme.warning.opacity(0.18), fg)
}

/// The theme color for a label. Labels are the one place the monochrome
/// theme allows color.
pub fn label_color(color: LabelColor, cx: &App) -> gpui_kit::Hsla {
    let theme = cx.theme();
    let color = match color {
        LabelColor::Red => theme.red,
        LabelColor::Green => theme.green,
        LabelColor::Blue => theme.blue,
        LabelColor::Yellow => theme.yellow,
        LabelColor::Magenta => theme.magenta,
        LabelColor::Cyan => theme.cyan,
        LabelColor::Gray => theme.muted_foreground,
    };
    // Tag text sits on a faint fill of its own color, so aim above 4.5:1.
    crate::shell::theme::ensure_contrast(color, &[theme.background, theme.muted], 5.0)
}

/// How much of the link color the underline of a link at rest shows. It
/// matches the underline GPUI Kit draws under Markdown links, so a link
/// looks the same in the git panel and in a description.
pub const LINK_UNDERLINE_REST: f32 = 0.4;

/// A link to a web page. Every link the app draws itself goes through
/// here, so all of them share one rest, hover and pressed style. GPUI Kit's
/// `Link` supplies the hover and pressed states.
pub fn link(id: impl Into<ElementId>, href: impl Into<SharedString>, cx: &App) -> Link {
    let theme = cx.theme();
    Link::new(id)
        .href(href)
        .text_color(theme.link)
        .text_decoration_1()
        .text_decoration_color(theme.link.opacity(LINK_UNDERLINE_REST))
}

/// The label tags of an issue, in label order.
pub fn tags(issue: &Issue, cx: &App) -> impl Iterator<Item = gpui_kit::Div> + use<> {
    let tags: Vec<_> = issue.labels.iter().map(|label| tag(label, cx)).collect();
    tags.into_iter()
}

/// One label's tag.
pub fn tag(label: &crate::model::data::Label, cx: &App) -> gpui_kit::Div {
    let color = label_color(label.color, cx);
    div()
        .px_1p5()
        .rounded(cx.theme().radius)
        .text_xs()
        .text_color(color)
        .bg(color.opacity(0.12))
        .child(label.name.clone())
}

/// The ID of one part of one issue's row, such as `issue_part("waiting",
/// id)`. It costs no allocation, unlike a formatted name, and tests find
/// the element with the same call. It takes the low 64 bits of the ID,
/// which are random in the server's IDs.
pub fn issue_part(name: &'static str, id: crate::model::data::IssueId) -> ElementId {
    ElementId::NamedInteger(SharedString::new_static(name), id.as_uuid().as_u64_pair().1)
}

/// A task ID. A click copies it, so it gets its own hover and pressed
/// states.
pub fn issue_key(
    id: ElementId,
    issue: &Issue,
    events: ViewEvents,
    text: gpui_kit::Hsla,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let hover = theme.foreground.opacity(0.08);
    let pressed = theme.foreground.opacity(0.16);
    let issue_id = issue.id;
    div()
        .id(id)
        .test_support()
        .px_1()
        .rounded(theme.radius)
        .cursor_pointer()
        .font_family(theme.mono_font_family.clone())
        .text_xs()
        .text_color(text)
        .hover(move |s| s.bg(hover))
        .active(move |s| s.bg(pressed))
        .tooltip(crate::shell::tooltip::Card::text("Copy ID").builder())
        .child(issue.key.clone())
        .on_click(move |_, window, cx| {
            events.emit(
                ViewEvent::OnIssue(issue_id, IssueAction::CopyKey),
                window,
                cx,
            );
        })
}

/// A task being dragged between status groups. It renders as the small
/// card that follows the pointer.
#[derive(Clone)]
pub struct DraggedIssue {
    pub issue: IssueId,
    pub key: SharedString,
    pub title: SharedString,
}

impl Render for DraggedIssue {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .gap_2()
            .px_2()
            .py_1()
            .text_sm()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius)
            .shadow_md()
            .child(
                div()
                    .font_family(theme.mono_font_family.clone())
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.key.clone()),
            )
            .child(self.title.clone())
    }
}

/// Height of the first bar in each panel: the filter bar over the list and
/// the header of the task view. In rem, so it follows the font size.
pub fn top_bar_height(window: &Window) -> gpui_kit::Pixels {
    window.rem_size() * 2.5
}

/// How far the task view's content sits inside its panel: the padding that
/// leaves room for the focus ring, plus the one-pixel outline border.
pub fn focus_inset(window: &Window) -> gpui_kit::Pixels {
    window.rem_size() * 0.25 + gpui_kit::px(1.)
}

/// The dependency icon of a task that waits for open work, with the depth
/// of its chain. It is not clickable, so it has no hover state. Without open
/// blockers, there is nothing to show.
pub fn dependency_badge(issue: &Issue, text: gpui_kit::Hsla, cx: &App) -> Option<AnyElement> {
    if issue.depth == 0 {
        return None;
    }
    let card = crate::shell::tooltip::Card::blockers(&issue.open_blockers, issue.depth);
    Some(
        h_flex()
            .id(SharedString::from(format!("deps-{}", issue.id)))
            .test_support()
            .gap_0p5()
            .text_xs()
            .font_family(cx.theme().mono_font_family.clone())
            .text_color(text)
            .child(Icon::new(IconName::Workflow).xsmall())
            .child(issue.depth.to_string())
            .tooltip(card.builder())
            .into_any_element(),
    )
}

/// The waiting-on mark: a robot with a green check when an agent can pick
/// the task up, a person with a question mark when the owner must answer.
/// Nothing when the task waits on no one.
pub fn waiting_on(
    id: ElementId,
    waiting: Option<WaitingOn>,
    cx: &App,
) -> Option<impl IntoElement + use<>> {
    let waiting = waiting?;
    let theme = cx.theme();
    let (icon, tooltip) = match waiting {
        WaitingOn::Agent => (IconName::Bot, "Waiting on an agent"),
        WaitingOn::Owner => (IconName::User, "Waiting on you"),
    };
    // The small mark sits on the icon's lower right corner, on a disc of
    // the background color so it stays legible over the icon.
    let mark = div()
        .absolute()
        .right(gpui_kit::px(-3.))
        .bottom(gpui_kit::px(-3.))
        .size(gpui_kit::px(10.))
        .rounded_full()
        .bg(theme.background)
        .flex()
        .items_center()
        .justify_center()
        .map(|this| match waiting {
            WaitingOn::Agent => this.child(
                Icon::new(IconName::Check)
                    .with_size(gpui_kit::px(9.))
                    .text_color(theme.success),
            ),
            WaitingOn::Owner => this
                .text_color(theme.foreground)
                .text_size(gpui_kit::px(9.))
                .font_weight(gpui_kit::FontWeight::BOLD)
                .line_height(gpui_kit::px(9.))
                .child("?"),
        });
    Some(
        div()
            .id(id)
            .test_support()
            .relative()
            .flex_none()
            .child(Icon::new(icon).small().text_color(theme.muted_foreground))
            .child(mark)
            .tooltip(crate::shell::tooltip::Card::text(tooltip).builder()),
    )
}

/// The app's standard action button: a filled button at the small size.
/// A selected one turns primary, as the Edit / Preview toggle does, so the
/// choice reads at a glance. Hover and pressed states come from GPUI Kit.
pub fn button(id: impl Into<ElementId>, selected: bool) -> Button {
    Button::new(id)
        .small()
        .selected(selected)
        .when(selected, |b| b.primary())
}

/// A single-line text field at the app's one field height. Every text field
/// goes through here, so a change of height reaches all of them. GPUI Kit's
/// default size is 2rem tall, the height the create window uses.
pub fn text_input(state: &Entity<InputState>) -> Input {
    Input::new(state)
}

/// Links an inbox button to the letter square inside it, which changes color
/// on hover.
pub const INBOX_BUTTON_GROUP: &str = "inbox-button";

/// The colors an inbox button takes when the pointer is on it.
pub struct InboxButtonColors {
    pub hover_bg: gpui_kit::Hsla,
    pub hover_fg: gpui_kit::Hsla,
    pub pressed_bg: gpui_kit::Hsla,
}

/// Hover inverts the page colors, so it shows in both themes.
pub fn inbox_button_colors(cx: &App) -> InboxButtonColors {
    let theme = cx.theme();
    InboxButtonColors {
        hover_bg: theme.foreground,
        hover_fg: theme.background,
        pressed_bg: theme.foreground.opacity(0.7),
    }
}

/// The fill and the letter color of an option's letter square.
#[derive(Clone, Copy)]
pub struct LetterColors {
    pub fill: gpui_kit::Hsla,
    pub letter: gpui_kit::Hsla,
}

/// The square is the opposite of the button it sits on. At rest it is black
/// with a white letter in both themes. On a hovered button it takes the
/// button's colors reversed, so in the light theme, where the hovered button
/// is black, it turns white with a black letter.
pub fn letter_colors(picked: bool, hovered: bool, cx: &App) -> LetterColors {
    let theme = cx.theme();
    if picked {
        return LetterColors {
            fill: theme.primary_foreground,
            letter: theme.primary,
        };
    }
    if hovered {
        let colors = inbox_button_colors(cx);
        return LetterColors {
            fill: colors.hover_fg,
            letter: colors.hover_bg,
        };
    }
    let (fill, letter) = if theme.is_dark() {
        (theme.background, theme.foreground)
    } else {
        (theme.foreground, theme.background)
    };
    LetterColors { fill, letter }
}

/// The opacity of a disabled filled button's fill. The text keeps its full
/// color, so it stays readable.
pub const FILLED_DISABLED_OPACITY: f32 = 0.6;

/// A filled button in the primary colors: white with dark text in the dark
/// theme, and the inverse in the light theme. Hover and press lighten the
/// fill. It has the same border width as an inbox button, so the two line up.
///
/// GPUI Kit draws a custom variant at rest with 20% of its color, so the
/// button's own style holds the rest fill and text color. GPUI Kit's disabled
/// style replays the button's own style, so a disabled button keeps a dimmer
/// fill and the same text color.
pub fn filled_button(id: impl Into<ElementId>, disabled: bool, cx: &App) -> Button {
    let theme = cx.theme();
    Button::new(id)
        .border_1()
        .custom(
            ButtonCustomVariant::new(cx)
                .color(theme.primary)
                .foreground(theme.primary_foreground)
                .hover(theme.primary_hover)
                .active(theme.primary_active),
        )
        .border_color(theme.primary)
        .bg(theme.primary)
        .text_color(theme.primary_foreground)
        .disabled(disabled)
        .when(disabled, |b| {
            let dim = theme.primary.opacity(FILLED_DISABLED_OPACITY);
            b.bg(dim).border_color(dim)
        })
}

/// A button for the inbox. Hover fills it with the page's inverse. A picked
/// button shows the primary colors and still reacts to the pointer, which a
/// GPUI Kit `selected` button does not.
///
/// GPUI Kit's hover keeps the variant's text color, so the variant holds the
/// hover text color and the button's own style holds the rest color. The
/// text inherits from the button, the one element whose hover state exists
/// during layout, which is when GPUI picks the color of text.
pub fn inbox_button(id: impl Into<ElementId>, picked: bool, cx: &App) -> Button {
    if picked {
        return filled_button(id, false, cx);
    }
    let theme = cx.theme();
    let button = Button::new(id).border_1();
    let colors = inbox_button_colors(cx);
    button
        .group(INBOX_BUTTON_GROUP)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(colors.hover_bg)
                .foreground(colors.hover_fg)
                .hover(colors.hover_bg)
                .active(colors.pressed_bg),
        )
        .border_color(theme.input)
        .bg(theme.button)
        .text_color(theme.button_foreground)
}
