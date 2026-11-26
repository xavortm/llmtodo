//! How views ask the workspace to do something. A view holds a
//! `ViewEvents` handle and emits a `ViewEvent`. The workspace runs each event
//! in its one update pass. Views never name `Workspace`, so
//! they can be built without one.

use std::sync::Arc;

use gpui_kit::{App, SharedString, Window};

use crate::issues::list::{GroupBy, RowClick, SortKey};
use crate::model::data::{IssueId, Label, Priority, Status, WaitingOn};
use crate::model::scope::Scope;

/// What a click on an issue asks for. The issue may be gone by the time the
/// workspace runs it. The workspace then says so in the status bar.
#[derive(Clone)]
pub enum IssueAction {
    CopyKey,
    SetStatus(Status),
    SetPriority(Priority),
    ToggleLabel(Label),
    SetWaitingOn(Option<WaitingOn>),
    /// The parent's key, or none to make it a top-level task.
    SetParent(Option<SharedString>),
    EditDescription,
    Restore,
    Delete,
}

type Handler = dyn Fn(ViewEvent, &mut Window, &mut App) + Send + Sync;

#[derive(Clone)]
pub enum ViewEvent {
    ShowMessage(String),
    OpenKey(String),
    OnIssue(IssueId, IssueAction),
    // The issue list.
    MoveDown,
    MoveUp,
    ClickRow {
        id: IssueId,
        click: RowClick,
    },
    ClickHeader {
        row: usize,
        title: String,
        collapsed: bool,
    },
    SortBy(SortKey),
    GroupBy(GroupBy),
    /// The filter panel: replace the `/` filter text.
    SetFilter(String),
    /// The filter button: open or close the filter panel.
    ToggleFilterPanel,
    CloseFilterPanel,
    /// The Add ticket button: opens the create form, as `c` does.
    NewIssue,
    CloseAllGroups,
    OpenAllGroups,
    // The sidebar.
    PickScope(Scope),
    SwitchTracker(usize),
    /// The chevron on a team row: close or open the team's group.
    ToggleSidebarGroup(SharedString),
    /// Set or clear a project's color, as `#rrggbb`.
    SetProjectColor {
        slug: String,
        color: Option<String>,
    },
    /// The owner dropped project `slug` on project `onto` in the sidebar.
    /// It takes that project's place.
    MoveProject {
        slug: SharedString,
        onto: SharedString,
    },
    // The task view.
    EditDescription,
    ShowPreview,
    SetTab(usize),
    /// The owner dragged the edge of a task view panel.
    PanelResized,
    RunCommand(String),
    OpenPaletteWith(String),
    ChangeDependency {
        id: IssueId,
        verb: String,
        other: String,
    },
    // Owner questions.
    AnswerQuestion {
        issue: IssueId,
        number: u32,
        answer: String,
    },
    UndoAnswer,
    /// Approve or Request changes on a ticket in User review. The feedback
    /// comes from the task view's feedback field.
    AnswerReview {
        issue: crate::model::data::IssueId,
        verdict: llmtodo_api_types::ReviewVerdict,
    },
    // Documents.
    EditDocument,
    LeaveDocEditor,
    // Settings.
    SetHideClosed(bool),
    SetHideClosedDays(u32),
    SetVimEditor(bool),
    SetAppearance(crate::shell::theme::Appearance),
    SetColorTheme(crate::shell::theme::ColorTheme),
    SetOpenLastView(bool),
    SetMascot(bool),
    SetAnimations(bool),
    SetStrikeDone(bool),
    SetZoom(f32),
    /// LLM-567: one key of a background table; `None` takes it out.
    SetBackground {
        slot: crate::shell::backdrop::Slot,
        key: &'static str,
        value: Option<crate::settings::BackgroundValue>,
    },
    /// LLM-580: closes Settings and opens the floating Backgrounds panel.
    EditBackgroundsLive,
    /// LLM-567: asks for an image file for a slot.
    ChooseBackground(crate::shell::backdrop::Slot),
}

/// A view's handle for sending events to whatever owns the window.
///
/// Delivery is synchronous. `cx.emit` would queue the event until the
/// current update ends, and the workspace must apply a click before the
/// code after it runs, as it did when views called it directly.
#[derive(Clone)]
pub struct ViewEvents(Arc<Handler>);

impl ViewEvents {
    pub fn new(handler: impl Fn(ViewEvent, &mut Window, &mut App) + Send + Sync + 'static) -> Self {
        Self(Arc::new(handler))
    }

    pub fn emit(&self, event: ViewEvent, window: &mut Window, cx: &mut App) {
        (self.0)(event, window, cx);
    }
}
