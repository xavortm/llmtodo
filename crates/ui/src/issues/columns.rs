//! The issue list's columns. The column header and every issue row build
//! their cells from this one table, with the same frame around them, so a
//! header cell always has the width and position of the cells below it.
//!
//! When the list gets narrow, it hides columns one at a time, the least
//! useful first, instead of squeezing the title. The list measures its own
//! width, so this also works with the task view open beside it.

use gpui_kit::Action;
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::h_flex;
use gpui_kit::{
    AnyElement, Div, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Rems,
    Stateful, Styled as _, div, rems,
};

/// The narrowest the title may get before the list hides a column.
const TITLE_MIN: Rems = Rems(10.);
/// The gap between cells, as `gap_2` in `row_frame`.
const GAP: Rems = Rems(0.5);
/// The side padding of `row_frame`, both sides together.
const PADDING: Rems = Rems(1.);

/// Shows or hides a column of the issue list. The column heading menu
/// sends it, and the workspace saves the choice.
#[derive(Action, Clone, PartialEq)]
#[action(namespace = list, no_json)]
pub struct ToggleColumn(pub Column);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// The box that marks the task for a bulk action.
    Check,
    /// What an agent does on the task: Locked, Analyzing or nothing.
    Llm,
    Status,
    Key,
    Title,
    Labels,
    /// The open blockers of the task: an icon and the depth of the chain.
    Deps,
    Waiting,
    Assignee,
    /// What the task has cost in tokens so far, in dollars.
    Cost,
    /// How long agents have worked on the task so far.
    Time,
    Added,
}

impl Column {
    /// Every column, left to right.
    pub const ALL: [Column; 12] = [
        Column::Check,
        Column::Llm,
        Column::Status,
        Column::Key,
        Column::Title,
        Column::Labels,
        Column::Deps,
        Column::Waiting,
        Column::Assignee,
        Column::Cost,
        Column::Time,
        Column::Added,
    ];

    /// The element ID of the cell. Header and rows use the same IDs, inside
    /// their own row.
    pub fn id(self) -> &'static str {
        match self {
            Column::Check => "col-check",
            Column::Llm => "col-llm",
            Column::Status => "col-status",
            Column::Key => "col-key",
            Column::Title => "col-title",
            Column::Labels => "col-labels",
            Column::Deps => "col-deps",
            Column::Waiting => "col-waiting",
            Column::Assignee => "col-assignee",
            Column::Cost => "col-cost",
            Column::Time => "col-time",
            Column::Added => "col-added",
        }
    }

    /// The entry for this column in the column heading menu.
    pub fn label(self) -> &'static str {
        match self {
            Column::Check => "Checkbox",
            Column::Llm => "LLM status",
            Column::Status => "Status",
            Column::Key => "Key",
            Column::Title => "Title",
            Column::Labels => "Labels",
            Column::Deps => "Dependencies",
            Column::Waiting => "Waiting on",
            Column::Assignee => "Assignee",
            Column::Cost => "Cost",
            Column::Time => "Time",
            Column::Added => "Added",
        }
    }

    /// The name of this column in the UI settings.
    pub fn setting(self) -> &'static str {
        self.id().trim_start_matches("col-")
    }

    /// The column saved as `name`, if the owner may hide it. The title
    /// always shows, so a saved `title` means nothing.
    pub fn hideable_from_setting(name: &str) -> Option<Column> {
        Column::ALL
            .into_iter()
            .find(|c| *c != Column::Title && c.setting() == name)
    }

    /// The fixed width. The title has none: it takes the space left over.
    pub fn width(self) -> Option<Rems> {
        match self {
            Column::Check => Some(rems(1.)),
            Column::Llm => Some(rems(5.)),
            Column::Status => Some(rems(5.)),
            Column::Key => Some(rems(4.)),
            Column::Title => None,
            // As wide as the Added column: the title gets the room.
            Column::Labels => Some(rems(4.)),
            // An icon and two digits.
            Column::Deps => Some(rems(2.25)),
            Column::Waiting => Some(rems(1.25)),
            Column::Assignee => Some(rems(2.)),
            // Seven monospace characters, such as $123.45.
            Column::Cost => Some(rems(3.5)),
            // Six monospace characters, such as 12h 05m.
            Column::Time => Some(rems(3.5)),
            Column::Added => Some(rems(4.)),
        }
    }

    /// When the list is narrow, the column with the lowest rank hides
    /// first. The check box, key, status and title have none: they always show.
    pub fn hide_rank(self) -> Option<u8> {
        match self {
            Column::Labels => Some(0),
            Column::Time => Some(1),
            Column::Cost => Some(2),
            Column::Added => Some(3),
            Column::Assignee => Some(4),
            Column::Waiting => Some(5),
            Column::Deps => Some(6),
            Column::Llm => Some(7),
            Column::Check | Column::Status | Column::Key | Column::Title => None,
        }
    }
}

/// The widest a title may want: wider titles get cut off anyway.
const TITLE_MAX: Rems = Rems(36.);
/// The share of titles that must fit before the list counts as wide enough.
const TITLES_THAT_FIT: f32 = 0.8;
/// About how wide one title character is, in rem. Titles are `text_sm`,
/// 0.875 rem, and a character of a proportional font averages about half
/// its font size.
const TITLE_CHAR: f32 = 0.45;

/// The width a row of `shown` columns needs, with a title `title` px wide.
fn needed(shown: &[Column], title: f32, rem_size: Pixels) -> f32 {
    let px = |r: Rems| r.0 * f32::from(rem_size);
    let cells: f32 = shown.iter().map(|c| c.width().map_or(title, px)).sum();
    cells + px(GAP) * (shown.len() - 1) as f32 + px(PADDING)
}

/// The title length, in characters, that most titles on the list fit in.
pub fn chars_that_fit(title_chars: impl IntoIterator<Item = usize>) -> usize {
    let mut chars: Vec<usize> = title_chars.into_iter().collect();
    match chars.len() {
        0 => 0,
        n => {
            let ix = ((n as f32 * TITLES_THAT_FIT).ceil() as usize).clamp(1, n) - 1;
            *chars.select_nth_unstable(ix).1
        }
    }
}

/// The list width at which every column shows and most titles fit, so
/// the list stops cutting most of its content. `chars` comes from
/// `chars_that_fit`. Above this width, extra room is better spent on the
/// task view.
pub fn comfortable_width_for(chars: usize, rem_size: Pixels, hidden: &[Column]) -> Pixels {
    gpui_kit::px(needed(&chosen(hidden), title_px(chars, rem_size), rem_size))
}

/// The width, in px, that a title of `chars` characters wants, between
/// the title's minimum and maximum.
fn title_px(chars: usize, rem_size: Pixels) -> f32 {
    let rem = f32::from(rem_size);
    (chars as f32 * TITLE_CHAR * rem).clamp(TITLE_MIN.0 * rem, TITLE_MAX.0 * rem)
}

/// The columns the owner did not hide, left to right. The title always
/// stays.
fn chosen(hidden: &[Column]) -> Vec<Column> {
    Column::ALL
        .into_iter()
        .filter(|c| *c == Column::Title || !hidden.contains(c))
        .collect()
}

/// The columns that fit a list `width` wide, left to right, out of those the
/// owner did not hide. Before the first layout the width is unknown, and
/// every chosen column shows.
///
/// The labels show only when the titles also get their full width. `chars`
/// comes from `chars_that_fit`. The other columns hide only when the title
/// would get narrower than its minimum.
pub fn visible(
    width: Option<Pixels>,
    rem_size: Pixels,
    hidden: &[Column],
    chars: usize,
) -> Vec<Column> {
    let mut shown = chosen(hidden);
    let Some(width) = width else {
        return shown;
    };
    if needed(&shown, title_px(chars, rem_size), rem_size) > f32::from(width) {
        shown.retain(|c| *c != Column::Labels);
    }
    let title = TITLE_MIN.0 * f32::from(rem_size);
    while needed(&shown, title, rem_size) > f32::from(width) {
        let next = shown
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.hide_rank().map(|rank| (rank, i)))
            .min();
        let Some((_, i)) = next else { break };
        shown.remove(i);
    }
    shown
}

/// About how wide one label character is, in rem. Tags are `text_xs`,
/// 0.75 rem.
const TAG_CHAR: f32 = 0.4;
/// The side padding of a tag, both sides together, as `px_1p5`.
const TAG_PADDING: f32 = 0.75;
/// The gap between tags, as `gap_1`.
const TAG_GAP: f32 = 0.25;

/// How many of the labels, with names `name_chars` characters long, fit in
/// the labels column. When some do not fit, the cell ends with "+N" for the
/// rest, and this leaves room for it. One label always shows, cut off if
/// it must be.
pub fn labels_that_fit(name_chars: &[usize]) -> usize {
    let room = Column::Labels.width().map_or(0., |w| w.0);
    let tag = |chars: usize| chars as f32 * TAG_CHAR + TAG_PADDING;
    // The width of the first `k` tags and the gaps after each of them.
    let tags = |k: usize| -> f32 { name_chars[..k].iter().map(|c| tag(*c) + TAG_GAP).sum() };
    let n = name_chars.len();
    if tags(n) - TAG_GAP <= room {
        return n;
    }
    let more = |rest: usize| format!("+{rest}").len() as f32 * TAG_CHAR;
    (1..n)
        .rev()
        .find(|k| tags(*k) + more(n - k) <= room)
        .unwrap_or(1)
}

/// The frame of a header or issue row: its height, side padding and the
/// gap between cells. Rows carry no left border: the owner's rule.
pub fn row_frame() -> Div {
    h_flex().w_full().h_7().px_2().gap_2()
}

/// An empty cell of `column`, sized for it. The caller fills it.
pub fn cell(column: Column) -> Stateful<Div> {
    let cell = div().id(column.id()).h_full().flex().items_center();
    match column.width() {
        Some(width) => cell.w(width).flex_none().overflow_hidden(),
        None => cell.flex_1().min_w_0(),
    }
}

/// Puts `child` in a cell of `column`. Tests find the cell by its ID and
/// read its bounds.
pub fn cell_with(column: Column, child: impl IntoElement) -> AnyElement {
    cell(column).child(child).test_support().into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::px;

    fn at(width: f32) -> Vec<Column> {
        visible(Some(px(width)), px(16.), &[], 0)
    }

    #[test]
    fn the_labels_column_is_as_wide_as_the_added_column() {
        assert_eq!(Column::Labels.width(), Column::Added.width());
    }

    #[test]
    fn columns_hide_least_useful_first() {
        use Column::*;
        assert_eq!(visible(None, px(16.), &[], 0), Column::ALL);
        assert_eq!(at(2000.), Column::ALL);
        // 16 for the padding, and
        // 16 + 80 + 80 + 64 + 160 + 64 + 36 + 20 + 32 + 56 + 56 + 64, and
        // 11 gaps of 8.
        assert_eq!(at(832.), Column::ALL);
        // The labels hide first, then the time and the cost.
        assert_eq!(
            at(831.),
            [
                Check, Llm, Status, Key, Title, Deps, Waiting, Assignee, Cost, Time, Added
            ]
        );
        assert_eq!(
            at(759.),
            [
                Check, Llm, Status, Key, Title, Deps, Waiting, Assignee, Cost, Added
            ]
        );
        assert_eq!(
            at(695.),
            [
                Check, Llm, Status, Key, Title, Deps, Waiting, Assignee, Added
            ]
        );
        assert_eq!(
            at(631.),
            [Check, Llm, Status, Key, Title, Deps, Waiting, Assignee]
        );
        assert_eq!(at(559.), [Check, Llm, Status, Key, Title, Deps, Waiting]);
        assert_eq!(at(519.), [Check, Llm, Status, Key, Title, Deps]);
        assert_eq!(at(491.), [Check, Llm, Status, Key, Title]);
        assert_eq!(at(447.), [Check, Status, Key, Title]);
        // The box, key, status and title stay, even when they do not fit.
        assert_eq!(at(100.), [Check, Status, Key, Title]);
    }

    #[test]
    fn the_labels_hide_before_a_title_is_cut() {
        use Column::*;
        // Most titles have 50 characters: 360 px instead of the 160 minimum.
        let at = |width: f32| visible(Some(px(width)), px(16.), &[], 50);
        assert_eq!(at(1032.), Column::ALL);
        let no_labels = [
            Check, Llm, Status, Key, Title, Deps, Waiting, Assignee, Cost, Time, Added,
        ];
        assert_eq!(at(1031.), no_labels);
        // The other columns still wait for the title's minimum.
        assert_eq!(at(760.), no_labels);
        assert_eq!(
            at(759.),
            [
                Check, Llm, Status, Key, Title, Deps, Waiting, Assignee, Cost, Added
            ]
        );
    }

    #[test]
    fn the_owners_choice_applies_before_the_width_rule() {
        use Column::*;
        let hidden = [Labels, Key, Check, Time];
        let without = [Llm, Status, Title, Deps, Waiting, Assignee, Cost, Added];
        assert_eq!(visible(None, px(16.), &hidden, 0), without);
        assert_eq!(visible(Some(px(2000.)), px(16.), &hidden, 0), without);
        // The narrower row has more room, so the cost column stays longer.
        // 832 - 64 for labels, - 64 for the key, - 16 for the box,
        // - 56 for the time, - 4 gaps.
        assert_eq!(visible(Some(px(600.)), px(16.), &hidden, 0), without);
        assert_eq!(
            visible(Some(px(599.)), px(16.), &hidden, 0),
            [Llm, Status, Title, Deps, Waiting, Assignee, Added],
            "the width rule hides what remains, least useful first"
        );
    }

    #[test]
    fn the_title_cannot_be_hidden() {
        let hidden = Column::ALL.to_vec();
        assert_eq!(visible(None, px(16.), &hidden, 0), [Column::Title]);
        assert_eq!(
            visible(Some(px(10.)), px(16.), &hidden, 80),
            [Column::Title]
        );
        assert_eq!(Column::hideable_from_setting("title"), None);
        assert_eq!(
            Column::hideable_from_setting("labels"),
            Some(Column::Labels)
        );
        assert_eq!(Column::hideable_from_setting("gone"), None);
    }

    #[test]
    fn as_many_labels_show_as_fit() {
        // No labels, one short label, or one that fills the cell.
        assert_eq!(labels_that_fit(&[]), 0);
        assert_eq!(labels_that_fit(&[3]), 1);
        assert_eq!(labels_that_fit(&[8]), 1);
        // Two tiny labels fit side by side.
        assert_eq!(labels_that_fit(&[2, 2]), 2);
        // A third does not: one shows, and "+2" takes the rest of the room.
        assert_eq!(labels_that_fit(&[2, 2, 2]), 1);
        // A long first label still shows, cut off, before "+1".
        assert_eq!(labels_that_fit(&[20]), 1);
        assert_eq!(labels_that_fit(&[20, 3]), 1);
    }

    #[test]
    fn the_comfortable_width_leaves_out_hidden_columns() {
        let all = comfortable_width_for(0, px(16.), &[]);
        let less = comfortable_width_for(0, px(16.), &[Column::Labels]);
        assert_eq!(f32::from(all) - f32::from(less), 64. + 8.);
    }

    fn comfortable(chars: &[usize]) -> f32 {
        f32::from(comfortable_width_for(
            chars_that_fit(chars.iter().copied()),
            px(16.),
            &[],
        ))
    }

    #[test]
    fn the_comfortable_width_fits_most_titles() {
        // The frame and the fixed columns take 832 - 160 = 672 px.
        // No titles, or short ones: the title keeps its minimum, 160.
        assert_eq!(comfortable(&[]), 832.);
        assert_eq!(comfortable(&[5, 10]), 832.);
        // Four in five titles fit: 50 characters at 7.2 px each.
        assert_eq!(comfortable(&[10, 20, 30, 50, 200]), 672. + 360.);
        // A list of very long titles stops at 36 rem, 576 px.
        assert_eq!(comfortable(&[300, 300]), 672. + 576.);
    }
}
