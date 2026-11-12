//! The issue list's columns. The column header and every issue row build
//! their cells from this one table, with the same frame around them, so a
//! header cell always has the width and position of the cells below it.
//!
//! `plan` decides which columns show and how wide the labels get, from the
//! list width and what the rows hold. When the list gets narrow, it hides
//! columns one at a time, the least useful first, instead of squeezing the
//! title. When it is wide, the labels grow so they stop being cut. The list measures its own
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
/// The widest the labels column may get.
const LABELS_MAX: Rems = Rems(24.);
/// The largest share of the list width the labels column may take.
const LABELS_SHARE: f32 = 0.3;
/// The share of rows whose content must fit before a column counts as
/// wide enough.
const ROWS_THAT_FIT: f32 = 0.8;
/// When the title and the labels both still cut some rows, the spare room
/// goes to them in this ratio of what each still lacks: the title first.
const TITLE_WEIGHT: f32 = 2.;
const LABELS_WEIGHT: f32 = 1.;
/// The labels width moves in steps this big, so a resize does not draw
/// the list again on every pixel.
const LABELS_STEP: f32 = 0.25;
/// About how wide one title character is, in rem. Titles are `text_sm`,
/// 0.875 rem, and a character of a proportional font averages about half
/// its font size.
const TITLE_CHAR: f32 = 0.45;

/// What the rows on the list ask of the two columns that change width:
/// the title and the labels. The list measures it once per build.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Demand {
    /// The title length, in characters, that most titles fit in.
    pub title_chars: usize,
    /// The longest title, in characters.
    pub title_max_chars: usize,
    /// The labels width, in rem, that most rows with labels fit in.
    pub labels: f32,
    /// The widest row of labels, in rem.
    pub labels_max: f32,
}

impl Demand {
    /// Measures `rows`: the title length of each row, in characters, and
    /// the length of each of its label names.
    pub fn measure<'a>(rows: impl IntoIterator<Item = (usize, &'a [usize])>) -> Demand {
        let mut titles = Vec::new();
        let mut labels = Vec::new();
        for (title, names) in rows {
            titles.push(title);
            if !names.is_empty() {
                labels.push(labels_rem(names));
            }
        }
        let labels_max = labels.iter().copied().fold(0., f32::max);
        Demand {
            title_chars: most(&mut titles).unwrap_or(0),
            title_max_chars: titles.iter().copied().max().unwrap_or(0),
            labels: most_f32(&mut labels).unwrap_or(0.),
            labels_max,
        }
    }

    /// The demand of a list whose titles are `chars` long and that has no
    /// labels.
    pub fn titles(chars: usize) -> Demand {
        Demand {
            title_chars: chars,
            title_max_chars: chars,
            ..Demand::default()
        }
    }
}

/// The value that most of `values` stay at or under.
fn most(values: &mut [usize]) -> Option<usize> {
    let ix = most_ix(values.len())?;
    Some(*values.select_nth_unstable(ix).1)
}

fn most_f32(values: &mut [f32]) -> Option<f32> {
    let ix = most_ix(values.len())?;
    Some(*values.select_nth_unstable_by(ix, f32::total_cmp).1)
}

fn most_ix(n: usize) -> Option<usize> {
    (n > 0).then(|| ((n as f32 * ROWS_THAT_FIT).ceil() as usize).clamp(1, n) - 1)
}

/// The columns a list shows and how wide its labels column is.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// The columns that show, left to right.
    pub columns: Vec<Column>,
    /// The width of the labels column, when it shows.
    pub labels: Rems,
}

impl Layout {
    /// The width of `column`. The title has none: it takes the space left
    /// over.
    pub fn width(&self, column: Column) -> Option<Rems> {
        match column {
            Column::Labels => Some(self.labels),
            other => other.width(),
        }
    }
}

/// The width a row of `shown` columns needs, with a title `title` px wide
/// and labels `labels` px wide.
fn needed(shown: &[Column], title: f32, labels: f32, rem_size: Pixels) -> f32 {
    let px = |r: Rems| r.0 * f32::from(rem_size);
    let cells: f32 = shown
        .iter()
        .map(|c| match c {
            Column::Title => title,
            Column::Labels => labels,
            other => other.width().map_or(0., px),
        })
        .sum();
    cells + px(GAP) * (shown.len().saturating_sub(1)) as f32 + px(PADDING)
}

/// The list width at which every column shows, most titles fit and most
/// labels fit, so the list stops cutting most of its content. Above this
/// width, extra room is better spent on the task view.
pub fn comfortable_width_for(demand: &Demand, rem_size: Pixels, hidden: &[Column]) -> Pixels {
    let rem = f32::from(rem_size);
    let labels = labels_rem_clamped(demand.labels, LABELS_MAX.0) * rem;
    let title = title_px(demand.title_chars, rem_size);
    gpui_kit::px(needed(&chosen(hidden), title, labels, rem_size))
}

/// The width, in px, that a title of `chars` characters wants, between
/// the title's minimum and maximum.
fn title_px(chars: usize, rem_size: Pixels) -> f32 {
    let rem = f32::from(rem_size);
    (chars as f32 * TITLE_CHAR * rem).clamp(TITLE_MIN.0 * rem, TITLE_MAX.0 * rem)
}

/// The narrowest the labels column gets.
fn labels_min() -> f32 {
    Column::Labels.width().map_or(0., |w| w.0)
}

/// `want` rem of labels, between the column's minimum and `max`.
fn labels_rem_clamped(want: f32, max: f32) -> f32 {
    want.clamp(labels_min(), max.max(labels_min()))
}

/// The columns the owner did not hide, left to right. The title always
/// stays.
fn chosen(hidden: &[Column]) -> Vec<Column> {
    Column::ALL
        .into_iter()
        .filter(|c| *c == Column::Title || !hidden.contains(c))
        .collect()
}

/// Plans the columns of a list `width` wide, out of those the owner did not
/// hide. Before the first layout the width is unknown: every chosen column
/// shows, and the labels get what most rows need.
///
/// The room goes out in order of priority:
///
/// 1. The title gets enough for most titles. The labels column shows only
///    when it then still fits at its minimum. The other columns hide, least
///    useful first, only when the title would get narrower than its own
///    minimum.
/// 2. The labels column grows until most rows' labels fit.
/// 3. The title and the labels share the rest until the longest of each
///    fits. Each gets a share of what it still lacks, the title twice as
///    much. The labels never take more than `LABELS_SHARE` of the list.
/// 4. Whatever is left goes to the title.
pub fn plan(width: Option<Pixels>, rem_size: Pixels, hidden: &[Column], demand: &Demand) -> Layout {
    let rem = f32::from(rem_size);
    let mut shown = chosen(hidden);
    let Some(width) = width else {
        let labels = labels_rem_clamped(demand.labels, LABELS_MAX.0);
        return Layout {
            columns: shown,
            labels: rems(step(labels)),
        };
    };
    let width = f32::from(width);
    let min = labels_min() * rem;
    let title_want = title_px(demand.title_chars, rem_size);
    if needed(&shown, title_want, min, rem_size) > width {
        shown.retain(|c| *c != Column::Labels);
    }
    let title_min = TITLE_MIN.0 * rem;
    while needed(&shown, title_min, min, rem_size) > width {
        let next = shown
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.hide_rank().map(|rank| (rank, i)))
            .min();
        let Some((_, i)) = next else { break };
        shown.remove(i);
    }
    if !shown.contains(&Column::Labels) {
        return Layout {
            columns: shown,
            labels: rems(labels_min()),
        };
    }
    let cap = (LABELS_MAX.0 * rem).min(width * LABELS_SHARE).max(min);
    let labels_want = (demand.labels * rem).clamp(min, cap);
    let labels_full = (demand.labels_max * rem).clamp(labels_want, cap);
    let title_full = title_px(demand.title_max_chars, rem_size).max(title_want);
    // Step 2: the labels grow until most rows fit.
    let mut spare = (width - needed(&shown, title_want, min, rem_size)).max(0.);
    let mut labels = min + spare.min(labels_want - min);
    spare -= labels - min;
    // Step 3: the title and the labels share the rest.
    let title_lacks = (title_full - title_want) * TITLE_WEIGHT;
    let labels_lacks = (labels_full - labels) * LABELS_WEIGHT;
    if labels_lacks > 0. {
        let share = spare * labels_lacks / (title_lacks + labels_lacks);
        // When the title needs less than its share, the labels may use it.
        let title_rest = (spare - (title_full - title_want)).max(0.);
        labels += share.max(title_rest).min(labels_full - labels);
    }
    Layout {
        columns: shown,
        labels: rems(step(labels / rem)),
    }
}

/// Rounds `rem` down to a whole `LABELS_STEP`, but not below the minimum.
fn step(rem: f32) -> f32 {
    ((rem / LABELS_STEP).floor() * LABELS_STEP).max(labels_min())
}

/// The columns that fit a list `width` wide, left to right. See `plan`.
pub fn visible(
    width: Option<Pixels>,
    rem_size: Pixels,
    hidden: &[Column],
    demand: &Demand,
) -> Vec<Column> {
    plan(width, rem_size, hidden, demand).columns
}

/// About how wide one label character is, in rem. Tags are `text_xs`,
/// 0.75 rem.
const TAG_CHAR: f32 = 0.4;
/// The side padding of a tag, both sides together, as `px_1p5`.
const TAG_PADDING: f32 = 0.75;
/// The gap between tags, as `gap_1`.
const TAG_GAP: f32 = 0.25;

fn tag_rem(chars: usize) -> f32 {
    chars as f32 * TAG_CHAR + TAG_PADDING
}

/// The width, in rem, that every label of a row needs side by side.
fn labels_rem(name_chars: &[usize]) -> f32 {
    let tags: f32 = name_chars.iter().map(|c| tag_rem(*c)).sum();
    tags + TAG_GAP * name_chars.len().saturating_sub(1) as f32
}

/// How many of the labels, with names `name_chars` characters long, fit in
/// a labels column `room` wide. When some do not fit, the cell ends with
/// "+N" for the rest, and this leaves room for it. One label always shows,
/// cut off if it must be.
pub fn labels_that_fit(name_chars: &[usize], room: Rems) -> usize {
    let room = room.0;
    // The width of the first `k` tags and the gaps after each of them.
    let tags = |k: usize| -> f32 { name_chars[..k].iter().map(|c| tag_rem(*c) + TAG_GAP).sum() };
    let n = name_chars.len();
    if labels_rem(name_chars) <= room {
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

/// An empty cell of `column`, sized for it by `layout`. The caller fills it.
pub fn cell(column: Column, layout: &Layout) -> Stateful<Div> {
    let cell = div().id(column.id()).h_full().flex().items_center();
    match layout.width(column) {
        Some(width) => cell.w(width).flex_none().overflow_hidden(),
        None => cell.flex_1().min_w_0(),
    }
}

/// Puts `child` in a cell of `column`, sized by `layout`. Tests find the
/// cell by its ID and read its bounds.
pub fn cell_with(column: Column, layout: &Layout, child: impl IntoElement) -> AnyElement {
    cell(column, layout)
        .child(child)
        .test_support()
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::px;

    const REM: f32 = 16.;

    fn plan_at(width: f32, demand: &Demand) -> Layout {
        plan(Some(px(width)), px(REM), &[], demand)
    }

    fn at(width: f32) -> Vec<Column> {
        plan_at(width, &Demand::default()).columns
    }

    /// Rows with `title` characters and labels of the given name lengths.
    fn rows(rows: &[(usize, &[usize])]) -> Demand {
        Demand::measure(rows.iter().copied())
    }

    #[test]
    fn the_labels_column_starts_as_wide_as_the_added_column() {
        assert_eq!(Column::Labels.width(), Column::Added.width());
        assert_eq!(plan_at(2000., &Demand::default()).labels, rems(4.));
    }

    #[test]
    fn columns_hide_least_useful_first() {
        use Column::*;
        assert_eq!(visible(None, px(REM), &[], &Demand::default()), Column::ALL);
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
        let at = |width: f32| plan_at(width, &Demand::titles(50)).columns;
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
    fn spare_room_goes_to_the_labels() {
        // Short titles and three labels of 8 characters on every row:
        // 3 * (3.2 + 0.75) + 2 * 0.25 = 12.35 rem of labels.
        let demand = rows(&[(20, &[8, 8, 8]), (20, &[8, 8, 8])]);
        assert!((demand.labels - 12.35).abs() < 1e-4);
        // At the narrowest width with labels, they keep their minimum.
        assert_eq!(plan_at(832., &demand).labels, rems(4.));
        // A wide list gives them every label, rounded down to a step.
        let wide = plan_at(2000., &demand);
        assert_eq!(wide.labels, rems(12.25));
        assert_eq!(labels_that_fit(&[8, 8, 8], rems(12.35)), 3);
        // In between, they grow with the list.
        let some = plan_at(900., &demand).labels.0;
        assert!(some > 4. && some < 12.25, "{some}");
    }

    #[test]
    fn the_title_comes_first_then_the_labels_for_most_rows() {
        // Most titles want 50 characters, 360 px; most labels want 12.35
        // rem, 197.6 px.
        let demand = rows(&[(50, &[8, 8, 8][..]); 5]);
        // The labels show from 1032 px, at their minimum: the title has
        // what most titles need.
        assert_eq!(plan_at(1032., &demand).labels, rems(4.));
        // Every extra pixel then goes to the labels, up to what most rows
        // need: 1032 - 64 + 197.6.
        assert_eq!(plan_at(1100., &demand).labels, rems(8.25));
        assert_eq!(plan_at(1166., &demand).labels, rems(12.25));
    }

    #[test]
    fn long_titles_and_long_labels_share_the_rest() {
        // Most rows are short, but one has a 100-character title and five
        // long labels.
        let mut list: Vec<(usize, &[usize])> = vec![(20, &[4][..]); 9];
        list.push((100, &[10, 10, 10, 10, 10][..]));
        let demand = rows(&list);
        // Both the title and the labels grow, the title faster, until the
        // labels hit their cap.
        let a = plan_at(1100., &demand).labels.0;
        let b = plan_at(1400., &demand).labels.0;
        assert!(a < b, "{a} < {b}");
        let huge = plan_at(4000., &demand).labels;
        assert_eq!(huge, rems(24.), "the labels stop at their maximum");
    }

    #[test]
    fn the_labels_never_take_more_than_their_share() {
        // A single row with a wall of labels.
        let demand = rows(&[(10, &[20; 10])]);
        let layout = plan_at(1200., &demand);
        assert!(layout.labels.0 * REM <= 1200. * LABELS_SHARE, "{layout:?}");
    }

    #[test]
    fn the_owners_choice_applies_before_the_width_rule() {
        use Column::*;
        let hidden = [Labels, Key, Check, Time];
        let without = [Llm, Status, Title, Deps, Waiting, Assignee, Cost, Added];
        let none = Demand::default();
        assert_eq!(visible(None, px(REM), &hidden, &none), without);
        assert_eq!(visible(Some(px(2000.)), px(REM), &hidden, &none), without);
        // The narrower row has more room, so the cost column stays longer.
        // 832 - 64 for labels, - 64 for the key, - 16 for the box,
        // - 56 for the time, - 4 gaps.
        assert_eq!(visible(Some(px(600.)), px(REM), &hidden, &none), without);
        assert_eq!(
            visible(Some(px(599.)), px(REM), &hidden, &none),
            [Llm, Status, Title, Deps, Waiting, Assignee, Added],
            "the width rule hides what remains, least useful first"
        );
    }

    #[test]
    fn the_title_cannot_be_hidden() {
        let hidden = Column::ALL.to_vec();
        let none = Demand::default();
        assert_eq!(visible(None, px(REM), &hidden, &none), [Column::Title]);
        assert_eq!(
            visible(Some(px(10.)), px(REM), &hidden, &Demand::titles(80)),
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
        let room = rems(4.);
        // No labels, one short label, or one that fills the cell.
        assert_eq!(labels_that_fit(&[], room), 0);
        assert_eq!(labels_that_fit(&[3], room), 1);
        assert_eq!(labels_that_fit(&[8], room), 1);
        // Two tiny labels fit side by side.
        assert_eq!(labels_that_fit(&[2, 2], room), 2);
        // A third does not: one shows, and "+2" takes the rest of the room.
        assert_eq!(labels_that_fit(&[2, 2, 2], room), 1);
        // A long first label still shows, cut off, before "+1".
        assert_eq!(labels_that_fit(&[20], room), 1);
        assert_eq!(labels_that_fit(&[20, 3], room), 1);
        // A wider column fits them all.
        assert_eq!(labels_that_fit(&[2, 2, 2], rems(8.)), 3);
    }

    #[test]
    fn the_comfortable_width_leaves_out_hidden_columns() {
        let none = Demand::default();
        let all = comfortable_width_for(&none, px(REM), &[]);
        let less = comfortable_width_for(&none, px(REM), &[Column::Labels]);
        assert_eq!(f32::from(all) - f32::from(less), 64. + 8.);
    }

    fn comfortable(chars: &[usize]) -> f32 {
        let list: Vec<(usize, &[usize])> = chars.iter().map(|c| (*c, &[][..])).collect();
        f32::from(comfortable_width_for(&rows(&list), px(REM), &[]))
    }

    #[test]
    fn the_comfortable_width_fits_most_titles_and_labels() {
        // The frame and the fixed columns take 832 - 160 = 672 px.
        // No titles, or short ones: the title keeps its minimum, 160.
        assert_eq!(comfortable(&[]), 832.);
        assert_eq!(comfortable(&[5, 10]), 832.);
        // Four in five titles fit: 50 characters at 7.2 px each.
        assert_eq!(comfortable(&[10, 20, 30, 50, 200]), 672. + 360.);
        // A list of very long titles stops at 36 rem, 576 px.
        assert_eq!(comfortable(&[300, 300]), 672. + 576.);
        // Labels that need 12.35 rem add 8.35 rem to the 4 rem minimum.
        let labelled = rows(&[(5, &[8, 8, 8])]);
        let width = f32::from(comfortable_width_for(&labelled, px(REM), &[]));
        assert!((width - (832. + 8.35 * REM)).abs() < 1e-3, "{width}");
    }
}
