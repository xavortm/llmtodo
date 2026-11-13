//! The app's single window: title bar, issue list, task view, palette and
//! status bar, with vim keys.
//!
//! The workspace owns the issues, in an `IssueStore` keyed by ID, and every
//! write to them. Four child entities draw the window: `Sidebar`,
//! `IssueList`, `TaskView` and `Palette`. They hold view state only. Clicks inside them
//! emit a `ViewEvent`, and the workspace updates the store and the children
//! in one pass.
//!
//! Vim modes live in the key context. The root element sets
//! `Workspace mode=normal` when no text field has focus, and `mode=insert`
//! when one does. Single-letter keys bind only under `mode == normal`, so
//! they never fire while the owner types. GPUI matches a binding against
//! every ancestor context of the focused element. That is why the mode must
//! sit on the Workspace context itself, and not only on the inputs.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _, ThemeMode, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{EditorState, InputEvent},
    resizable::{ResizablePanelEvent, ResizableState, h_resizable, resizable_panel},
    status_bar::StatusBar,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyBinding, KeyContext, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, actions, div,
    prelude::FluentBuilder as _,
};

use crate::docs::DocView;
use crate::inbox::{AnswerInputs, InboxView};
use crate::issues::claims::{ClaimsEvent, ClaimsList};
use crate::issues::columns::{Column, ToggleColumn};
use crate::issues::create::{self, CreateEvent, CreateForm, Draft};
use crate::issues::filter::Filter;
use crate::issues::list::{GroupBy, IssueList, RowClick, SortKey};
use crate::model::backend::{self, Backend, Write, WriteKind};
use crate::model::data::{
    Assignee, Catalog, Claim, Issue, IssueId, Label, OWNER_NAME, Status, WaitingOn,
};
use crate::model::drafts::DraftStore;
use crate::model::external_editor::ExternalEdits;
use crate::model::pipeline::WritePipeline;
use crate::model::scope::{Scope, ViewMembers};
use crate::model::store::{Connection, IssueStore};
use crate::model::trackers::{self, Tracker};
use crate::settings::view::SettingsView;
use crate::settings::{MAX_HIDE_DAYS, SettingsStore, UiSettings};
use crate::shell::app_menus;
use crate::shell::layout::Layout;
use crate::shell::mascot::Mascot;
use crate::shell::palette::{Palette, PaletteEvent};
use crate::shell::sidebar::{self, Sidebar};
use crate::shell::view_events::ViewEvents;
use crate::shell::view_history::{View, ViewHistory};
use crate::task::sections;
use crate::task::task_view::{DetailMode, TaskView};
use crate::task::vim_editor::{self, VimMode};

mod commands;
mod events;
mod keys;
mod render;
mod review_batch;
mod update;
mod view_events;

pub use keys::*;
pub use update::UpdateButton;

pub use crate::issues::list::Row;
pub use crate::task::task_view::Detail;

pub const CONTEXT: &str = "Workspace";

/// The newest answer, while the status bar offers to take it back.
struct AnswerUndo {
    issue: IssueId,
    number: u32,
    message: SharedString,
    /// Ends the offer. A newer answer drops this timer and starts its own.
    _expire: gpui_kit::Task<()>,
}

/// LLM-199: a short card above the pointer, such as "Copied", after a
/// click whose result the window does not show by itself.
pub(crate) struct Flash {
    pub(crate) text: SharedString,
    /// The pointer position at the click, in window coordinates.
    pub(crate) at: gpui_kit::Point<gpui_kit::Pixels>,
    /// The card, built once, with the same look as a tooltip.
    pub(crate) view: gpui_kit::AnyView,
    /// Hides the card. A newer card drops this timer and starts its own.
    _expire: gpui_kit::Task<()>,
}

/// How long a flash card stays.
pub(crate) const FLASH_FOR: std::time::Duration = std::time::Duration::from_millis(1200);
/// How long issues must stay unchanged before a saved view is read again.
const VIEW_REFRESH_AFTER: std::time::Duration = std::time::Duration::from_millis(250);

/// What the status bar says while the conflict view is open.
fn conflict_help(key: &str) -> String {
    format!(
        "{key} changed on the server while you edited it. Your text is on the left, the server's on the right. \
         Edit yours to merge and press Esc to save, or run :mine or :theirs"
    )
}

/// The command word for a status name: `In Review` becomes `in-review`.
/// Moves an issue whose last open question got an answer back to the
/// agents, and words the status bar message. `open` counts the questions
/// still open.
fn answered(issue: &mut Issue, id: &str, given: &str, open: usize) -> String {
    let mut message = format!("Answered {} {id}: {given}", issue.key);
    if open == 0 && issue.waiting_on == Some(WaitingOn::Owner) {
        issue.waiting_on = Some(WaitingOn::Agent);
        message.push_str(". It goes back to the agents");
    } else if open > 0 {
        message.push_str(&format!(". {open} more open on {}", issue.key));
    }
    message
}

fn status_word(name: &str) -> String {
    name.to_lowercase().replace(' ', "-")
}

/// What the title bar shows right of the project name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TitlePlace {
    /// The project's linked repo: `label` such as `xavortm/llmtodo`, and
    /// the page a click opens.
    Repo {
        label: SharedString,
        url: SharedString,
    },
    /// A local folder, with the home folder shortened to `~`. A click
    /// copies it.
    Folder(SharedString),
    /// Nothing: the scope is not one project.
    None,
}

/// The title bar's name and place. `LLMTODO_PROJECT_DIR`, passed as
/// `dev_dir`, wins for development runs. Then the project of the scope
/// gives its name and its repo, else its local folder. Any other scope
/// shows the tracker's name alone. The app's working folder never shows:
/// an app started from the Dock has `/` there (LLM-213).
pub(crate) fn title_place(
    scope: &Scope,
    catalog: &Catalog,
    tracker: &str,
    dev_dir: Option<&std::path::Path>,
    home: Option<&str>,
) -> (SharedString, TitlePlace) {
    let short = |path: &str| -> SharedString {
        match home.and_then(|h| path.strip_prefix(h)) {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}").into(),
            _ => path.to_string().into(),
        }
    };
    if let Some(dir) = dev_dir {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| tracker.to_string());
        return (
            name.into(),
            TitlePlace::Folder(short(&dir.to_string_lossy())),
        );
    }
    let slug = match scope {
        Scope::Project(slug) => Some(slug.clone()),
        Scope::Document(id) => catalog
            .documents
            .iter()
            .find(|d| d.id.to_string() == id.as_ref())
            .map(|d| d.project.clone()),
        _ => None,
    };
    let project = slug.and_then(|slug| catalog.projects.iter().find(|p| p.key == slug));
    let Some(project) = project else {
        return (tracker.to_string().into(), TitlePlace::None);
    };
    let place = if let Some(url) = catalog.project_repo(&project.key) {
        let label = url
            .strip_prefix("https://github.com/")
            .or_else(|| url.strip_prefix("https://"))
            .unwrap_or(url);
        TitlePlace::Repo {
            label: label.to_string().into(),
            url: url.to_string().into(),
        }
    } else if let Some(folder) = catalog.project_folder(&project.key) {
        TitlePlace::Folder(short(folder))
    } else {
        TitlePlace::None
    };
    (project.name.clone(), place)
}

/// Starts the API thread for a tracker. Tests swap it for one that records
/// which tracker it was asked for.
pub type Starter = Arc<
    dyn Fn(
        &Tracker,
    ) -> (
        Backend,
        futures::channel::mpsc::UnboundedReceiver<backend::Event>,
    ),
>;

/// The window's position and size, as settings store them.
fn frame_of(window: &Window) -> [f32; 4] {
    let b = window.bounds();
    [
        f32::from(b.origin.x),
        f32::from(b.origin.y),
        f32::from(b.size.width),
        f32::from(b.size.height),
    ]
}

/// How long the window must rest before its frame is saved.
const UNDO_ANSWER_FOR: std::time::Duration = std::time::Duration::from_secs(8);
const SAVE_FRAME_AFTER: std::time::Duration = std::time::Duration::from_millis(500);

/// Below this window width the sidebar hides on its own. It is in rems, so
/// it scales with the zoom level: 48 rems is 768 pixels at 100% zoom.
const SIDEBAR_AUTO_HIDE: gpui_kit::Rems = gpui_kit::Rems(48.);

/// Whether the window is too narrow to show the sidebar.
fn is_narrow(window: &Window) -> bool {
    window.viewport_size().width < SIDEBAR_AUTO_HIDE.to_pixels(window.rem_size())
}

/// How often the workspace checks the files open in `$EDITOR`.
const EXTERNAL_POLL: std::time::Duration = std::time::Duration::from_millis(400);

pub struct Workspace {
    /// The list's focus. The root element tracks it.
    focus: FocusHandle,
    store: Entity<IssueStore>,
    sidebar: Entity<Sidebar>,
    /// The cat under the sidebar. It shows and ticks only with the sidebar,
    /// and only while `mascot_on`.
    mascot: Entity<Mascot>,
    /// The Settings switch for the cat.
    mascot_on: bool,
    list: Entity<IssueList>,
    task: Entity<TaskView>,
    palette: Entity<Palette>,
    create: Entity<CreateForm>,
    doc_view: Entity<DocView>,
    inbox: Entity<InboxView>,
    settings_view: Entity<SettingsView>,
    /// The settings the Settings view shows, as last saved.
    settings_values: Entity<UiSettings>,
    /// Where `Esc` returns from the Settings view: the scope and the task
    /// the list had selected.
    settings_return: (Scope, Option<IssueId>),
    hide_closed: bool,
    hide_closed_days: u32,
    open_last_view: bool,
    /// The saved task to reopen once the first snapshot arrives.
    restore_task: Option<String>,
    /// The task open at the last save, so a save happens only on a change.
    last_open: Option<IssueId>,
    /// The answer fields of owner questions, shared by the inbox
    /// and the task view.
    answers: Entity<AnswerInputs>,
    /// The Approve answers that one menu click sent for the marked tasks.
    review_batch: Option<review_batch::ReviewBatch>,
    /// LLM-168: the single Approve answers sent while a task panel was
    /// open. Only these open the next review ticket when the server takes
    /// them.
    approved_with_panel: std::collections::HashSet<IssueId>,
    /// The owner's text from a document save the server refused. The
    /// editor opens with it again, and `:w!` overwrites.
    doc_draft: Option<(llmtodo_core::DocumentId, String)>,
    /// The server refused the last document save as stale. Only `:w!`
    /// saves again, so a plain save never overwrites the other version.
    doc_stale: bool,
    claims: Entity<ClaimsList>,
    /// The API thread. None in tests on demo data.
    backend: Option<Backend>,
    /// Which writes are on their way, and the owner's unsaved text.
    pipeline: WritePipeline,
    /// A claim closed the editor. The next frame moves its focus to the
    /// preview, because a server event has no window to do it with.
    refocus_after_lock: bool,
    /// Descriptions open in `$EDITOR`.
    external: ExternalEdits,
    _external_poll: Option<gpui_kit::Task<()>>,
    /// Restart to update: set only for the owner's real app.
    update: Option<update::UpdateState>,
    _events: Option<gpui_kit::Task<()>>,
    /// Redraws the claim timers once a minute.
    _clock: gpui_kit::Task<()>,
    /// `LLMTODO_PROJECT_DIR`: a development run names its folder, and
    /// the title bar shows it in place of the scope's project.
    project_dir: Option<std::path::PathBuf>,
    /// The panes: the sidebar and the split between list and task view.
    layout: Layout,
    window_frame: Option<[f32; 4]>,
    settings: SettingsStore,
    /// The settings as last saved, to skip writes that change nothing.
    saved: UiSettings,
    /// A save that waits until the window stops moving.
    save_soon: Option<gpui_kit::Task<()>>,
    /// The views the owner visited, for back and forward.
    views: ViewHistory<View>,
    /// The open owner questions at the last store change. `None` until the
    /// store holds a tracker's issues, so a first load cannot bounce the Dock.
    questions_seen: Option<usize>,
    /// How many times the Dock icon bounced.
    #[cfg(test)]
    dock_bounces: usize,
    /// The task whose history was asked for last, so each open asks once.
    history_for: Option<IssueId>,
    /// Set while this machine is not the primary copy: the host that holds
    /// the sync lock, if the server knows it.
    not_primary: Option<Option<SharedString>>,
    /// Asks the server for the saved view's issues again, shortly after
    /// issues change. Many changes in a row make one request.
    view_refresh: Option<gpui_kit::Task<()>>,
    /// Issues moved to the Trash in this run, newest last, for `:restore`.
    /// Each keeps its key, because a snapshot leaves trashed issues out of
    /// the store.
    last_deleted: Vec<(IssueId, SharedString)>,
    help_open: bool,
    message: SharedString,
    /// LLM-199: the card a copy click shows above the pointer.
    flash: Option<Flash>,
    /// The answer the Undo button takes back, while the button shows.
    answer_undo: Option<AnswerUndo>,
    /// The question an undo on its way to the server reopens.
    undoing: Option<(IssueId, u32)>,
    /// The task whose text the app asked for last, so a list change does
    /// not ask again before the answer comes.
    text_asked: Option<IssueId>,
    /// Starts the API thread for another tracker. None until the app
    /// knows its trackers.
    starter: Option<Starter>,
    /// A key to open once the tracker the app switched to has loaded.
    open_after_load: Option<String>,
    /// LLM-149: the list moved to a saved view whose issues have not come
    /// yet. When they come, the task panel closes unless the view holds
    /// its task.
    check_view_task: bool,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    /// A workspace on demo data with default settings that are never
    /// saved, and no server. Tests use it.
    #[cfg(test)]
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this =
            Self::with_settings(UiSettings::default(), SettingsStore::memory(), window, cx);
        this.load_demo(cx);
        this
    }

    /// Sends writes to `backend` without listening for its events. Tests
    /// play the server's answers with `apply_event`.
    #[cfg(test)]
    pub fn store(&self) -> &Entity<IssueStore> {
        &self.store
    }

    #[cfg(test)]
    pub fn attach_backend(&mut self, backend: Backend) {
        self.backend = Some(backend);
    }

    /// Shows the demo issues, with no server behind them.
    #[cfg(test)]
    /// Stands in for `LLMTODO_PROJECT_DIR`, so a test does not depend on
    /// the shell that runs it.
    #[cfg(test)]
    pub(crate) fn set_project_dir(&mut self, dir: Option<std::path::PathBuf>) {
        self.project_dir = dir;
    }

    pub fn load_demo(&mut self, cx: &mut Context<Self>) {
        let catalog = crate::model::demo::catalog();
        let issues = crate::model::demo::issues(&catalog);
        self.set_connection(Connection::Local, cx);
        self.load(catalog, issues, cx);
    }

    /// How often the claim timers redraw. They show whole minutes, so a
    /// tick every 15 s keeps them at most that far behind.
    const CLOCK_TICK: std::time::Duration = std::time::Duration::from_secs(15);

    /// Redraws the list and the task view while any task is claimed, so
    /// the claim timers count up.
    fn start_clock(cx: &mut Context<Self>) -> gpui_kit::Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Self::CLOCK_TICK).await;
                let alive = this.update(cx, |this, cx| {
                    if this.store.read(cx).iter().any(|i| i.claim.is_some()) {
                        this.list.update(cx, |_, cx| cx.notify());
                        this.task.update(cx, |_, cx| cx.notify());
                    }
                    // An Analyzing mark expires without a server event.
                    if this.list.read(cx).reads_llm_status() {
                        this.list_do(cx, |list, cx| list.refilter(cx));
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
    }

    /// A workspace that starts from `settings` and saves changes to `store`.
    pub fn with_settings(
        settings: UiSettings,
        store: SettingsStore,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let events = {
            let me = cx.weak_entity();
            ViewEvents::new(move |event, window, cx| {
                me.update(cx, |this, cx| this.on_view_event(event, window, cx))
                    .ok();
            })
        };
        let issues = cx.new(|_| IssueStore::new());
        let list =
            cx.new(|cx| IssueList::new(issues.clone(), events.clone(), focus.clone(), window, cx));
        let answers = cx.new(|cx| AnswerInputs::new(events.clone(), &issues, window, cx));
        let task =
            cx.new(|cx| TaskView::new(issues.clone(), events.clone(), answers.clone(), window, cx));
        let sidebar = cx.new(|cx| Sidebar::new(issues.clone(), events.clone(), cx));
        let mascot = cx.new(|cx| Mascot::new(issues.clone(), cx));
        let palette = cx.new(|cx| Palette::new(focus.clone(), window, cx));
        let create = cx.new(|cx| CreateForm::new(focus.clone(), window, cx));
        let doc_view = cx.new(|cx| DocView::new(issues.clone(), events.clone(), window, cx));
        let inbox =
            cx.new(|cx| InboxView::new(issues.clone(), events.clone(), answers.clone(), cx));
        let settings_values = cx.new(|_| settings.clone());
        let settings_view = cx.new(|cx| {
            SettingsView::new(
                issues.clone(),
                events.clone(),
                settings_values.clone(),
                window,
                cx,
            )
        });
        let claims = cx.new(|cx| ClaimsList::new(issues.clone(), focus.clone(), cx));

        let filter = list.read(cx).filter.clone();
        let mut subscriptions = vec![
            cx.subscribe_in(
                &filter,
                window,
                |this, state, event, window, cx| match event {
                    InputEvent::Change => {
                        let query = state.read(cx).value().to_lowercase();
                        this.apply_filter(&query, cx);
                    }
                    // Enter opens the selected match, which is the first one
                    // unless the owner moved.
                    InputEvent::PressEnter { .. } => {
                        this.focus_list(window, cx);
                        this.open(&Open, window, cx);
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(&claims, window, |this, _, event, window, cx| {
                let ClaimsEvent::Open(id) = event;
                this.open_task(*id, window, cx);
            }),
            // Claims change live, so the open list redraws with the store.
            cx.observe(&issues, {
                let claims = claims.clone();
                move |_, _, cx| claims.update(cx, |_, cx| cx.notify())
            }),
            cx.subscribe_in(&create, window, |this, form, event, window, cx| {
                let CreateEvent::Submit { text, description } = event;
                let sent = this.create_issue(text, description, window, cx);
                let message = this.message.clone();
                form.update(cx, |form, cx| {
                    if sent {
                        form.close(window, cx);
                    } else {
                        form.show_notice(message, cx);
                    }
                });
            }),
            cx.subscribe_in(&palette, window, |this, _, event, window, cx| match event {
                PaletteEvent::Run(command) => this.run_command(command, window, cx),
                PaletteEvent::Dismissed => cx.notify(),
            }),
            // Any key means keyboard navigation again, so the focus outline
            // comes back. An observer sees every key, bound ones included.
            cx.observe_keystrokes(|this: &mut Self, _, _, cx| {
                this.task.update(cx, |task, cx| {
                    if task.focus_by_mouse {
                        task.focus_by_mouse = false;
                        cx.notify();
                    }
                });
            }),
            // The children draw from the store.
            cx.observe(&issues, |_, _, cx| cx.notify()),
            // The task to reopen at the next start.
            cx.observe(&task, |this, _, cx| this.remember_open_task(cx)),
            // The Dock shows the inbox's open owner questions.
            cx.observe_in(&issues, window, |this, _, window, cx| {
                this.update_dock(window.is_window_active(), cx);
            }),
            // The view history follows whatever changed the view.
            cx.observe(&task, |this: &mut Self, _, cx| this.record_view(false, cx)),
            cx.observe(&list, |this: &mut Self, _, cx| this.record_view(false, cx)),
            cx.observe(&claims, |this: &mut Self, _, cx| {
                this.record_view(false, cx)
            }),
            // A newly opened task asks the server for its history.
            cx.observe(&task, |this: &mut Self, task, cx| {
                let open = task.read(cx).detail().map(|d| d.issue);
                if open != this.history_for {
                    this.history_for = open;
                    this.request(backend::Request::Open { id: open });
                    this.request_history(cx);
                    this.show_sessions(cx);
                }
            }),
        ];

        // Pane sizes: remember what the owner dragged them to.
        let split_side = cx.new(|_| ResizableState::default());
        let split_stacked = cx.new(|_| ResizableState::default());
        for (state, stacked) in [(&split_side, false), (&split_stacked, true)] {
            subscriptions.push(cx.subscribe(
                state,
                move |this, state, _: &ResizablePanelEvent, cx| {
                    this.split_resized(&state, stacked, cx);
                },
            ));
        }
        // No frame shows while the window is hidden, so the cat rests.
        subscriptions.push(
            cx.observe_window_visibility(window, |this, visibility, _, cx| {
                let hidden = visibility == gpui_kit::WindowVisibility::Hidden;
                this.mascot
                    .update(cx, |mascot, cx| mascot.set_paused(hidden, cx));
            }),
        );
        // The window frame, saved when it moves or resizes.
        subscriptions.push(cx.observe_window_bounds(window, |this, window, cx| {
            this.window_moved(frame_of(window), cx);
            this.update_narrow(window, cx);
        }));

        // A save still waiting for the window to stop goes out at quit.
        subscriptions.push(cx.on_app_quit(|this, cx| {
            this.persist(cx);
            // Kept description text is written on a background thread.
            crate::model::drafts::flush();
            async {}
        }));
        // And when the red button closes the window, which quits the app.
        window.on_window_should_close(cx, {
            let me = cx.weak_entity();
            move |_, cx| {
                me.update(cx, |this, cx| this.persist(cx)).ok();
                crate::model::drafts::flush();
                true
            }
        });

        window.focus(&focus, cx);
        let narrow = is_narrow(window);
        let mut this = Self {
            focus,
            store: issues,
            sidebar,
            mascot,
            mascot_on: true,
            list,
            task,
            palette,
            create,
            doc_view,
            inbox,
            settings_view,
            settings_values,
            settings_return: (Scope::All, None),
            hide_closed: false,
            hide_closed_days: 14,
            open_last_view: true,
            restore_task: None,
            last_open: None,
            answers,
            review_batch: None,
            approved_with_panel: Default::default(),
            doc_draft: None,
            doc_stale: false,
            claims,
            backend: None,
            pipeline: WritePipeline::default(),
            refocus_after_lock: false,
            external: ExternalEdits::from_env(),
            _external_poll: None,
            update: None,
            _events: None,
            _clock: Self::start_clock(cx),
            project_dir: std::env::var_os("LLMTODO_PROJECT_DIR")
                .filter(|d| !d.is_empty())
                .map(std::path::PathBuf::from),
            layout: Layout::new(split_side, split_stacked, narrow),
            window_frame: None,
            settings: store,
            saved: settings.clone(),
            save_soon: None,
            view_refresh: None,
            not_primary: None,
            history_for: None,
            views: ViewHistory::default(),
            questions_seen: None,
            #[cfg(test)]
            dock_bounces: 0,
            last_deleted: Vec::new(),
            help_open: false,
            message: "".into(),
            flash: None,
            answer_undo: None,
            undoing: None,
            text_asked: None,
            starter: None,
            open_after_load: None,
            check_view_task: false,
            _subscriptions: subscriptions,
        };
        this.apply_settings(&settings, cx);
        this.rebuild_commands(cx);
        let values = this.current_settings(cx);
        this.settings_values.update(cx, |shown, _| *shown = values);
        this
    }

    /// A pane divider moved. Only a drag by the owner pins a size.
    fn split_resized(
        &mut self,
        state: &Entity<ResizableState>,
        stacked: bool,
        cx: &mut Context<Self>,
    ) {
        if self.task.read(cx).detail().is_none() {
            return;
        }
        let Some(size) = state.read(cx).sizes().get(1).copied() else {
            return;
        };
        // The smart split resizes the panel too. Only a drag
        // pins a size.
        let auto = self.layout.auto_split.filter(|_| !stacked);
        if auto.is_some_and(|(_, w)| (f32::from(w) - f32::from(size)).abs() < 0.5) {
            return;
        }
        if size > gpui_kit::px(0.) {
            let size = Some(f32::from(size));
            if stacked {
                self.layout.split_sizes.1 = size;
            } else {
                self.layout.split_sizes.0 = size;
            }
            self.persist(cx);
        }
    }

    /// Talks to the server through `backend` from now on. `events` carries
    /// what the API thread hears: snapshots, fresh issues and write results.
    pub fn connect(
        &mut self,
        backend: Backend,
        mut events: futures::channel::mpsc::UnboundedReceiver<backend::Event>,
        cx: &mut Context<Self>,
    ) {
        use futures::StreamExt as _;
        self.backend = Some(backend);
        self.set_connection(Connection::Connecting, cx);
        self.message = "Connecting to the llmtodo server".into();
        // The thread starts with no open task.
        self.request(backend::Request::Open {
            id: self.history_for,
        });
        self._events = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                // Apply every event that already waits in one update, so the
                // store's observers run once per burst, not once per event.
                let mut batch = vec![event];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                let applied = this.update(cx, |this, cx| {
                    for event in batch {
                        this.apply_event(event, cx);
                    }
                });
                if applied.is_err() {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// The trackers the sidebar lists. The first is the one `connect`
    /// talks to. `starter` starts the API thread for another one.
    /// Keeps unsaved description text in this store from now on.
    pub fn set_drafts(&mut self, drafts: DraftStore) {
        self.pipeline.set_conflicts(drafts);
    }

    #[cfg(test)]
    pub fn drafts(&self) -> &DraftStore {
        self.pipeline.conflicts()
    }

    pub fn set_trackers(&mut self, list: Vec<Tracker>, starter: Starter, cx: &mut Context<Self>) {
        self.starter = Some(starter);
        self.store.update(cx, |store, cx| {
            store.trackers = list;
            store.tracker = 0;
            cx.notify();
        });
        self.rebuild_commands(cx);
    }

    /// The trackers and which one is open.
    #[cfg(test)]
    pub fn trackers(&self, cx: &App) -> (Vec<Tracker>, usize) {
        let store = self.store.read(cx);
        (store.trackers.clone(), store.tracker)
    }

    /// The key of the task the task view shows.
    #[cfg(test)]
    pub fn open_task_key(&self, cx: &App) -> Option<SharedString> {
        let id = self.task.read(cx).detail()?.issue;
        self.store.read(cx).get(id).map(|i| i.key.clone())
    }

    /// Why the app cannot leave the open tracker now, if it cannot.
    fn switch_blocker(&self, cx: &App) -> Option<&'static str> {
        if self.editing(cx) || self.doc_view.read(cx).editing {
            Some("Save or leave the editor first")
        } else if !self.external.is_empty() {
            Some("Close the descriptions open in $EDITOR first")
        } else if self.pipeline.is_busy() {
            Some("Wait for the saves on their way to finish")
        } else {
            None
        }
    }

    /// Why the open tracker takes no changes, if it does not.
    pub(super) fn read_only_reason(&self, cx: &App) -> Option<String> {
        let store = self.store.read(cx);
        store
            .trackers
            .get(store.tracker)
            .filter(|t| t.read_only)
            .map(|t| format!("{} is read-only in a preview, so nothing changed", t.name))
    }

    /// True, with a message, when the open tracker takes no changes. Each
    /// edit checks this before it changes the app's own copy. The client
    /// refuses a write that gets past it.
    fn refuse_read_only_tracker(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(reason) = self.read_only_reason(cx) else {
            return false;
        };
        self.message = reason.into();
        cx.notify();
        true
    }

    /// Shows the tracker at `ix` instead of the open one. It gets its own
    /// API thread, which starts its server when none answers. The old
    /// thread ends when its handle drops.
    pub fn switch_tracker(&mut self, ix: usize, cx: &mut Context<Self>) -> bool {
        let (current, tracker) = {
            let store = self.store.read(cx);
            (store.tracker, store.trackers.get(ix).cloned())
        };
        let Some(tracker) = tracker else {
            return false;
        };
        if ix == current {
            self.message = format!("{} is already open", tracker.name).into();
            cx.notify();
            return false;
        }
        if let Some(reason) = self.switch_blocker(cx) {
            self.message = reason.into();
            cx.notify();
            return false;
        }
        let Some(starter) = self.starter.clone() else {
            return false;
        };
        // Nothing from the old tracker may reach the new one.
        self.backend = None;
        self._events = None;
        self.pipeline
            .switch_tracker(&tracker.data_dir, tracker.read_only);
        self.last_deleted.clear();
        self.doc_draft = None;
        self.doc_stale = false;
        self.not_primary = None;
        self.history_for = None;
        self.views = ViewHistory::default();
        self.view_refresh = None;
        self.task.update(cx, |task, cx| task.close(cx));
        self.store.update(cx, |store, cx| {
            cx.notify();
            store.tracker = ix;
            store.view = None;
            store.server_url = None;
        });
        self.load(Catalog::default(), Vec::new(), cx);
        let (backend, events) = starter(&tracker);
        self.connect(backend, events, cx);
        self.message = if tracker.read_only {
            format!("Opening {}. It is read-only in a preview", tracker.name)
        } else if tracker.real {
            format!(
                "Opening {}. Changes here change your real data",
                tracker.name
            )
        } else {
            format!("Opening {}", tracker.name)
        }
        .into();
        self.rebuild_commands(cx);
        cx.notify();
        true
    }

    /// Opens `key` in the other tracker that holds its team, when the open
    /// tracker does not have that team. Returns the message to show, or
    /// `None` when no other tracker could hold the key.
    fn open_elsewhere(&mut self, key: &str, cx: &mut Context<Self>) -> Option<String> {
        let team = trackers::team_of(key)?;
        let store = self.store.read(cx);
        if store
            .catalog
            .team_names
            .iter()
            .any(|t| t.key.eq_ignore_ascii_case(team))
        {
            return None;
        }
        let ix = trackers::owner_of(&store.trackers, store.tracker, key)?;
        let name = store.trackers[ix].name.clone();
        if !self.switch_tracker(ix, cx) {
            return Some(self.message.to_string());
        }
        self.open_after_load = Some(key.to_uppercase());
        Some(format!("{key} is in {name}. {}", self.message))
    }

    /// After a snapshot: note the open tracker's teams, and open the key
    /// that sent the app here.
    fn loaded_tracker(&mut self, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| {
            cx.notify();
            let teams = store
                .catalog
                .team_names
                .iter()
                .map(|t| t.key.clone())
                .collect();
            let open = store.tracker;
            if let Some(tracker) = store.trackers.get_mut(open) {
                tracker.teams = Some(teams);
            }
        });
        if let Some(key) = self.open_after_load.take() {
            self.open_key(&key, cx);
        }
        self.restore_last_task(cx);
    }

    /// Reopens the task from the last run, once the store holds it. The
    /// inbox and a document have no task panel, so nothing opens there.
    pub fn restore_last_task(&mut self, cx: &mut Context<Self>) {
        if self.store.read(cx).is_empty() {
            return;
        }
        let Some(key) = self.restore_task.take() else {
            return;
        };
        if matches!(self.list.read(cx).scope, Scope::Inbox | Scope::Document(_)) {
            return;
        }
        let store = self.store.read(cx);
        let Some(id) = store.by_key(&key).filter(|i| !i.deleted).map(|i| i.id) else {
            return;
        };
        if let Some(row) = self.list.read(cx).row_of(id) {
            self.list_do(cx, |list, cx| list.select(row, cx));
        }
        self.task.update(cx, |task, cx| task.show(id, cx));
        self.fetch_open_details(cx);
    }

    /// Shows the open owner questions on the Dock icon, and bounces it when
    /// a new one arrives while the app is in the background.
    fn update_dock(&mut self, app_active: bool, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let count = crate::inbox::open_count(store);
        let loaded = !store.is_empty();
        crate::shell::app_icon::set_dock_badge(count);
        if crate::shell::app_icon::should_bounce(self.questions_seen, count, app_active) {
            crate::shell::app_icon::bounce_dock();
            #[cfg(test)]
            {
                self.dock_bounces += 1;
            }
        }
        self.questions_seen = loaded.then_some(count);
    }

    /// How many times the Dock icon bounced.
    #[cfg(test)]
    pub fn dock_bounces(&self) -> usize {
        self.dock_bounces
    }

    /// Saves when another task opened, so a crash still reopens it.
    fn remember_open_task(&mut self, cx: &mut Context<Self>) {
        let open = self.task.read(cx).detail().map(|d| d.issue);
        if open != self.last_open {
            self.last_open = open;
            self.persist(cx);
        }
    }

    /// Asks each other tracker's server for its teams, so a key from
    /// another tracker opens there.
    fn probe_trackers(&self, cx: &App) {
        let Some(backend) = &self.backend else {
            return;
        };
        let store = self.store.read(cx);
        for (ix, tracker) in store.trackers.iter().enumerate() {
            if ix != store.tracker {
                backend.request(backend::Request::ProbeTeams {
                    data_dir: tracker.data_dir.clone(),
                });
            }
        }
    }

    fn set_connection(&mut self, connection: Connection, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| {
            store.connection = connection;
            cx.notify();
        });
    }

    /// Lists and snapshots carry no git panel, so the open task fetches its
    /// own the first time it shows. An empty panel marks the request, so
    /// moving back and forth fetches once; live events refresh it after.
    /// Fetches the open task when the app lacks its git panel or its text.
    /// The snapshot carries neither.
    fn fetch_open_details(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.task.read(cx).detail().map(|d| d.issue) else {
            return;
        };
        let asked = self.text_asked;
        let (key, wants_text) = self.store.update(cx, |store, _| {
            let wants_text = store
                .get(id)
                .is_some_and(|i| !i.description_loaded && asked != Some(id));
            let stale = store.take_stale_detail(id);
            if store.git(id).is_some() && !wants_text && !stale {
                return (None, false);
            }
            if store.git(id).is_none() {
                store.set_git(id, Default::default());
            }
            (store.get(id).map(|i| i.key.clone()), wants_text)
        });
        if wants_text {
            self.text_asked = Some(id);
        }
        if let Some(key) = key {
            self.fetch(&key);
        }
    }

    /// Whether the app holds the text of `id`. Without it, the app asks for
    /// it and says so in the status bar.
    fn has_text(&mut self, id: IssueId, cx: &mut Context<Self>) -> bool {
        let Some(issue) = self.store.read(cx).get(id) else {
            return false;
        };
        if issue.description_loaded {
            return true;
        }
        let key = issue.key.clone();
        self.message = format!("Loading {key}. Try again in a moment").into();
        self.fetch(&key);
        cx.notify();
        false
    }

    fn fetch(&self, key: &str) {
        if let Some(backend) = &self.backend {
            backend.fetch(key);
        }
    }

    /// Runs `f` on the list, then lets an open preview follow the selection.
    /// Every change to the list goes through here. The list is cached, so
    /// this notifies it: `f` may set its fields directly.
    fn list_do<R>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut IssueList, &mut Context<IssueList>) -> R,
    ) -> R {
        let out = self.list.update(cx, |list, cx| {
            let out = f(list, cx);
            cx.notify();
            out
        });
        let selected = self.list.read(cx).selected_issue();
        self.task.update(cx, |task, cx| task.follow(selected, cx));
        self.fetch_open_details(cx);
        cx.notify();
        out
    }

    /// Replaces every issue and the catalog, and keeps the selection and the
    /// open task by issue ID.
    pub fn load(&mut self, catalog: Catalog, issues: Vec<Issue>, cx: &mut Context<Self>) {
        let selected = self.list.read(cx).selected_issue();
        let detail = self.task.read(cx).detail();
        // The task in the editor, kept if the snapshot leaves it out.
        let editing = detail
            .filter(|d| d.mode == DetailMode::Editing)
            .and_then(|d| self.store.read(cx).get(d.issue).cloned());
        let open_before = editing.clone();
        // Issues with writes on their way keep the app's copy. The
        // snapshot's copy waits for the last answer, like a fetched one.
        let mut issues = issues;
        for issue in &mut issues {
            if self.pipeline.has_writes(issue.id)
                && let Some(local) = self.store.read(cx).get(issue.id)
            {
                let snapshot = std::mem::replace(issue, local.clone());
                self.pipeline.hold(snapshot);
            }
        }
        self.store.update(cx, |store, cx| {
            store.catalog = catalog;
            store.replace_all(issues);
            // A snapshot holds the Trash too, but an issue can go to the
            // Trash while it loads. The owner may still be editing it. Keep
            // the old copy so the editor and its text stay. The load dropped
            // its cached detail, so the open task's fetch below asks for the
            // issue, which the server still returns by key.
            if let Some(open) = editing.filter(|open| store.get(open.id).is_none()) {
                store.upsert(open);
            }
            cx.notify();
        });
        // A reconnect can bring a claim that started while offline.
        if let Some(open) = open_before {
            let kept = self.close_editor_on_lock(open.id, Some(&open.description), cx);
            self.announce_claim(open.id, open.claim.as_ref(), kept, cx);
        }
        self.text_asked = None;
        self.after_load(selected, detail, cx);
    }

    /// Puts in a new catalog and maps the issues the app holds to it. Server
    /// copies that wait for writes are mapped too.
    pub fn apply_catalog(&mut self, catalog: Catalog, cx: &mut Context<Self>) {
        let selected = self.list.read(cx).selected_issue();
        let detail = self.task.read(cx).detail();
        let renames = self.store.update(cx, |store, cx| {
            cx.notify();
            store.remap(catalog)
        });
        let catalog = self.store.read(cx).catalog.clone();
        self.pipeline.remap_held(&catalog, &renames);
        self.after_load(selected, detail, cx);
    }

    /// What follows a new set of issues or a new catalog: commands, tabs,
    /// the scope, the open task and the list.
    fn after_load(
        &mut self,
        selected: Option<IssueId>,
        detail: Option<crate::task::task_view::Detail>,
        cx: &mut Context<Self>,
    ) {
        self.rebuild_commands(cx);
        self.prune_tabs(cx);
        // A team or project that went away leaves the list on every issue.
        let scope = self.list.read(cx).scope.clone();
        let catalog = &self.store.read(cx).catalog;
        if !catalog.teams.is_empty() && !scope.exists(catalog) {
            self.message = format!("{} is gone. Showing all issues", scope.name(catalog)).into();
            self.list.update(cx, |list, cx| {
                list.scope = Scope::All;
                cx.notify();
            });
            self.sidebar.update(cx, |sidebar, cx| {
                sidebar.active = Scope::All;
                cx.notify();
            });
            self.persist(cx);
        }
        self.refresh_view(cx);
        self.sync_doc_view(cx);
        let gone = detail.is_some_and(|d| self.store.read(cx).get(d.issue).is_none());
        if gone {
            self.task.update(cx, |task, cx| task.close(cx));
            if !self.store.read(cx).is_empty() {
                self.message = "The open task is gone".into();
            }
        }
        self.list_do(cx, |list, cx| list.reload(selected, cx));
        // A snapshot is not the owner going somewhere.
        self.record_view(true, cx);
    }

    /// A fresh copy of one issue from the server: a write's answer or a
    /// live fetch. While the app still has writes on their way for it, the
    /// copy waits, so the app's own newer changes stay on screen.
    pub fn receive_issue(&mut self, issue: Issue, cx: &mut Context<Self>) {
        if let Some(issue) = self.pipeline.receive(issue) {
            self.upsert_issue(issue, cx);
        }
    }

    /// One write for `key` got its answer, or failed. After the last one,
    /// the newest server copy goes in.
    fn write_returned(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(issue) = self.pipeline.write_returned(key, self.store.read(cx)) {
            self.upsert_issue(issue, cx);
        }
    }

    /// Drops saved tabs for tasks the data no longer holds, so the saved
    /// settings do not grow with every task ever opened.
    fn prune_tabs(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        if store.is_empty() {
            return;
        }
        let keys: std::collections::HashSet<SharedString> =
            store.iter().map(|i| i.key.clone()).collect();
        let pruned = self.task.update(cx, |task, cx| {
            let before = task.open_tabs.len();
            task.open_tabs.retain(|key, _| keys.contains(key));
            let pruned = task.open_tabs.len() != before;
            if pruned {
                cx.notify();
            }
            pruned
        });
        if pruned {
            self.persist(cx);
        }
    }

    /// Puts a fresh copy of one issue in place by ID, or adds a new one. A
    /// team move changes the key, and the row keeps its place under the new
    /// key. The selection stays on the same task.
    pub fn upsert_issue(&mut self, issue: Issue, cx: &mut Context<Self>) {
        let id = issue.id;
        let list = self.list.read(cx);
        let refilter = match self.store.read(cx).get(id) {
            Some(old) => list.needs_refilter(old, &issue),
            None => true,
        };
        let old = self.store.update(cx, |store, cx| {
            cx.notify();
            store.upsert(issue)
        });
        let before = old.as_ref().map(|o| o.description.clone());
        let kept = self.close_editor_on_lock(id, before.as_deref(), cx);
        self.announce_claim(id, old.and_then(|o| o.claim).as_ref(), kept, cx);
        self.refresh_view(cx);
        // A task the owner is not editing closes when it goes to the Trash.
        let deleted = self.store.read(cx).get(id).is_some_and(|i| i.deleted);
        let task = self.task.read(cx);
        if deleted && task.detail().is_some_and(|d| d.issue == id) && !task.editing() {
            self.task.update(cx, |task, cx| task.close(cx));
        }
        self.list_do(cx, |list, cx| {
            if refilter {
                list.refilter(cx);
            } else {
                cx.notify();
            }
        });
    }

    /// A claimed task is read-only, so a claim closes the editor open on it
    /// and shows the preview. Nothing goes to the server, because a save
    /// would write under the agent. Unsaved text goes where a refused save
    /// goes, and `e` brings it back beside the server's version after the
    /// claim ends. Returns whether it kept text.
    ///
    /// `before` is the description before this update. Text that matches it
    /// is not an edit, even when the agent changed the description.
    fn close_editor_on_lock(
        &mut self,
        id: IssueId,
        before: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        let task = self.task.read(cx);
        if !task.editing() || task.detail().map(|d| d.issue) != Some(id) {
            return false;
        }
        let in_conflict = task.conflict;
        let text = task.editor.read(cx).value();
        let Some(issue) = self.store.read(cx).get(id).filter(|i| i.claim.is_some()) else {
            return false;
        };
        let key = issue.key.clone();
        let changed = text != issue.description && before != Some(&*text);
        // A queued save would write to the locked task. The store already
        // shows its text, so only the queue says that text is unsaved.
        let queued = self.pipeline.drop_queued(&key);
        let kept = queued || in_conflict || changed;
        if kept {
            self.pipeline.keep_after_lock(key, text.to_string());
        }
        self.pipeline.end_editing();
        self.refocus_after_lock = true;
        self.task
            .update(cx, |task, cx| task.set_mode(DetailMode::Preview, cx));
        kept
    }

    /// Says in the status bar when an agent takes or releases a task.
    /// `kept` means the claim closed the editor and kept unsaved text.
    fn announce_claim(&mut self, id: IssueId, before: Option<&Claim>, kept: bool, cx: &App) {
        let Some(issue) = self.store.read(cx).get(id) else {
            return;
        };
        match (before, &issue.claim) {
            (None, Some(claim)) => {
                let who = claim.actor.label();
                self.message = if kept {
                    format!(
                        "{who} took {}. Your unsaved text is kept: press e after the claim ends",
                        issue.key
                    )
                } else {
                    format!("{who} took {}", issue.key)
                }
                .into();
            }
            (Some(claim), None) => {
                let who = claim.actor.label();
                self.message = if self.pipeline.has_conflict(&issue.key) {
                    format!(
                        "{who} released {}. Press e to get your unsaved text back",
                        issue.key
                    )
                } else {
                    format!("{who} released {}", issue.key)
                }
                .into();
            }
            _ => {}
        }
    }

    /// The palette commands: the fixed ones, plus one per status and label.
    fn rebuild_commands(&mut self, cx: &mut Context<Self>) {
        let catalog = &self.store.read(cx).catalog;
        let mut out: Vec<String> = catalog
            .all_statuses()
            .iter()
            .map(|s| format!("status {}", status_word(&s.name)))
            .collect();
        out.extend(COMMANDS.iter().map(|c| c.to_string()));
        out.extend(
            catalog
                .team_names
                .iter()
                .map(|t| format!("team {}", t.key.to_lowercase())),
        );
        out.extend(
            catalog
                .projects
                .iter()
                .map(|p| format!("project {}", p.key)),
        );
        out.extend(catalog.views.iter().map(|v| format!("view {}", v.key)));
        let store = self.store.read(cx);
        if store.trackers.len() > 1 {
            out.extend(
                store
                    .trackers
                    .iter()
                    .map(|t| format!("tracker {}", t.name.to_lowercase())),
            );
        }
        out.extend(
            catalog
                .labels
                .iter()
                .map(|l| format!("label {}", l.name.to_lowercase())),
        );
        // `open <key>` for every task, so typing part of a key finds it.
        out.extend(
            self.store
                .read(cx)
                .iter()
                .filter(|i| !i.deleted)
                .map(|i| format!("open {}", i.key.to_lowercase())),
        );
        // The tasks a new dependency can point at. Suggested only after the
        // verb, so typing a key still finds `open`.
        for verb in ["depends", "blocks"] {
            out.extend(
                self.store
                    .read(cx)
                    .iter()
                    .filter(|i| !i.deleted && !i.status.is_closed())
                    .map(|i| format!("{verb} {}", i.key.to_lowercase())),
            );
        }
        self.palette
            .update(cx, |palette, _| palette.set_commands(out));
    }

    /// Sends a write to the server. Without a server, as in tests, the
    /// app's own change is all there is.
    fn send(&mut self, id: IssueId, write: Write, kind: WriteKind, cx: &App) {
        if let Some(key) = self.store.read(cx).get(id).map(|i| i.key.clone()) {
            self.send_to(id, key, write, kind);
        }
    }

    /// Sends a write for the issue `id` under `key`, also when the store
    /// does not hold the issue.
    fn send_to(&mut self, id: IssueId, key: SharedString, write: Write, kind: WriteKind) {
        if let Some(backend) = &self.backend {
            backend.write(&key, write, kind);
            self.pipeline.note_sent(id, key);
        }
    }

    /// Changes the app's own copy of an issue.
    fn edit_issue<R>(
        &mut self,
        id: IssueId,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Issue, &Catalog) -> R,
    ) -> Option<R> {
        self.store.update(cx, |store, cx| {
            cx.notify();
            let catalog = store.catalog.clone();
            store.modify(id, |issue| f(issue, &catalog))
        })
    }

    /// Restores what `settings` holds. The theme is applied before the
    /// window opens, in `main`.
    fn apply_settings(&mut self, settings: &UiSettings, cx: &mut Context<Self>) {
        crate::shell::theme::set_zoom(settings.zoom, cx);
        let sort = (
            match settings.sort.as_str() {
                "status" => SortKey::Status,
                "added" => SortKey::Added,
                "llm" => SortKey::Llm,
                _ => SortKey::Key,
            },
            settings.sort_reversed,
        );
        let group = GroupBy::parse(&settings.group).unwrap_or(GroupBy::None);
        let collapsed = settings.collapsed_groups.iter().cloned().collect();
        self.layout.split_sizes = (settings.split_side, settings.split_stacked);
        self.window_frame = settings.window;
        self.layout.sidebar_open = settings.sidebar_open;
        self.mascot_on = settings.mascot;
        self.sync_mascot(cx);
        self.hide_closed = settings.hide_closed;
        self.hide_closed_days = settings.hide_closed_days.clamp(1, MAX_HIDE_DAYS);
        self.open_last_view = settings.open_last_view;
        let vim = settings.vim_editor;
        self.task.update(cx, |task, cx| {
            task.vim_enabled = vim;
            cx.notify();
        });
        let scope = if settings.open_last_view {
            self.restore_task = settings.last_task.clone();
            Scope::from_setting(&settings.sidebar_scope).unwrap_or_default()
        } else {
            Scope::All
        };
        let hide_closed_after = self.hide_closed.then_some(self.hide_closed_days);
        let strike_done = settings.strike_done;
        let hidden_columns = settings
            .hidden_columns
            .iter()
            .filter_map(|name| Column::hideable_from_setting(name))
            .collect();
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.active = scope.clone();
            cx.notify();
        });
        let open_tabs = settings
            .open_tabs
            .iter()
            .map(|(k, v)| (SharedString::from(k.clone()), *v))
            .collect();
        let panel_heights = settings.panel_heights.clone();
        self.task.update(cx, |task, cx| {
            task.open_tabs = open_tabs;
            task.panel_heights = panel_heights;
            cx.notify();
        });
        self.list_do(cx, |list, cx| {
            list.sort = sort;
            list.group = group;
            list.collapsed = collapsed;
            list.scope = scope;
            list.hide_closed_after = hide_closed_after;
            list.hidden_columns = hidden_columns;
            list.strike_done = strike_done;
            list.reload(None, cx);
        });
    }

    /// The settings as they are now.
    pub fn current_settings(&self, cx: &App) -> UiSettings {
        let list = self.list.read(cx);
        let mut collapsed: Vec<_> = list.collapsed.iter().cloned().collect();
        collapsed.sort();
        // Settings is never the last view: the app reopens what was under it.
        let scope = match &list.scope {
            Scope::Settings => &self.settings_return.0,
            scope => scope,
        };
        let task = self.task.read(cx);
        let last_task = task
            .detail()
            .and_then(|d| self.store.read(cx).get(d.issue))
            .map(|i| i.key.to_string())
            .or_else(|| self.restore_task.clone());
        UiSettings {
            theme: if cx.theme().is_dark() {
                "dark"
            } else {
                "light"
            }
            .into(),
            color_theme: crate::shell::theme::color_theme(cx).id().into(),
            sort: match list.sort.0 {
                SortKey::Key => "key",
                SortKey::Status => "status",
                SortKey::Added => "added",
                SortKey::Llm => "llm",
            }
            .into(),
            sort_reversed: list.sort.1,
            group: list.group.label().to_lowercase(),
            collapsed_groups: collapsed,
            sidebar_open: self.layout.sidebar_open,
            sidebar_scope: scope.to_setting(),
            split_side: self.layout.split_sizes.0,
            split_stacked: self.layout.split_sizes.1,
            open_tabs: self
                .task
                .read(cx)
                .open_tabs
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect(),
            panel_heights: task.panel_heights.clone(),
            window: self.window_frame,
            zoom: crate::shell::theme::zoom(cx),
            hide_closed: self.hide_closed,
            hide_closed_days: self.hide_closed_days,
            vim_editor: task.vim_enabled,
            open_last_view: self.open_last_view,
            last_task,
            hidden_columns: list
                .hidden_columns
                .iter()
                .map(|c| c.setting().to_string())
                .collect(),
            mascot: self.mascot_on,
            strike_done: list.strike_done,
        }
    }

    /// The window moved or changed size. A drag does this every frame, so
    /// the save waits until the window rests for `SAVE_FRAME_AFTER`.
    pub fn window_moved(&mut self, frame: [f32; 4], cx: &mut Context<Self>) {
        self.window_frame = Some(frame);
        self.persist_soon(cx);
    }

    /// Saves the settings once they rest for `SAVE_FRAME_AFTER`. A drag
    /// changes them every frame, and this saves them once at the end.
    pub fn persist_soon(&mut self, cx: &mut Context<Self>) {
        self.save_soon = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_FRAME_AFTER).await;
            this.update(cx, |this, cx| this.persist(cx)).ok();
        }));
    }

    /// Saves the settings when they changed. Called after every change, so
    /// closing the app, or a crash, keeps them. Only the window frame waits;
    /// see `window_moved`.
    fn persist(&mut self, cx: &mut Context<Self>) {
        self.save_soon = None;
        let now = self.current_settings(cx);
        self.settings_values.update(cx, |shown, cx| {
            if *shown != now {
                *shown = now.clone();
                cx.notify();
            }
        });
        if now == self.saved {
            return;
        }
        if let Err(err) = self.settings.save(&now) {
            self.message = format!("Could not save settings: {err}").into();
        }
        self.saved = now;
    }

    // Readers, mostly for tests.

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected(&self, cx: &App) -> usize {
        self.list.read(cx).selected()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    #[cfg(test)]
    pub fn task_view(&self) -> &Entity<TaskView> {
        &self.task
    }

    pub fn detail(&self, cx: &App) -> Option<Detail> {
        self.task.read(cx).detail()
    }

    #[cfg(test)]
    pub fn create_title(&self, cx: &App) -> String {
        self.create.read(cx).input.read(cx).value().to_string()
    }

    #[cfg(test)]
    pub fn create_description(&self, cx: &App) -> String {
        self.create
            .read(cx)
            .description
            .read(cx)
            .value()
            .to_string()
    }

    #[cfg(test)]
    pub fn create_notice(&self, cx: &App) -> Option<SharedString> {
        self.create.read(cx).notice()
    }

    /// Opens `id` in the task view, without any list selection.
    #[cfg(test)]
    pub fn open_for_test(&mut self, id: IssueId, cx: &mut Context<Self>) {
        self.task.update(cx, |task, cx| task.show(id, cx));
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn create_open(&self, cx: &App) -> bool {
        self.create.read(cx).is_open()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn sidebar_open(&self) -> bool {
        self.layout.sidebar_open
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn scope(&self, cx: &App) -> Scope {
        self.list.read(cx).scope.clone()
    }

    pub fn sidebar_focused(&self, window: &Window, cx: &App) -> bool {
        self.sidebar.read(cx).focus.is_focused(window)
    }

    #[cfg(test)]
    pub fn palette_typed(&self, cx: &App) -> String {
        self.palette.read(cx).typed(cx)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn palette_open(&self, cx: &App) -> bool {
        self.palette.read(cx).is_open()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn visible_count(&self, cx: &App) -> usize {
        self.list.read(cx).visible().len()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn filter_text(&self, cx: &App) -> SharedString {
        self.list.read(cx).filter.read(cx).value()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn editor(&self, cx: &App) -> Entity<EditorState> {
        self.task.read(cx).editor.clone()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn editor_vim(&self, cx: &App) -> VimMode {
        self.task.read(cx).editor_vim
    }

    /// The width the list and the task view share, and the width at which
    /// the list fits most of its content, as laid out.
    #[cfg(test)]
    pub fn split_room(&self, window: &Window, cx: &App) -> (gpui_kit::Pixels, gpui_kit::Pixels) {
        let room = self.layout.split_side.read(cx).container_size();
        (
            room,
            self.list.read(cx).comfortable_width(window.rem_size(), cx),
        )
    }

    /// The task view's size beside the list, as laid out.
    #[cfg(test)]
    pub fn detail_size(&self, cx: &App) -> Option<gpui_kit::Pixels> {
        self.layout.split_side.read(cx).sizes().get(1).copied()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn palette_selected(&self, cx: &App) -> usize {
        self.palette.read(cx).selected()
    }

    /// The `n`th issue in sort order, ignoring groups.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn visible_issue<'a>(&self, n: usize, cx: &'a App) -> &'a Issue {
        let id = self.list.read(cx).visible()[n];
        self.store.read(cx).get(id).expect("visible issue")
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows<'a>(&self, cx: &'a App) -> &'a [Row] {
        self.list.read(cx).rows()
    }

    /// The server's base URL, once it connected.
    #[cfg(test)]
    pub fn server_url(&self, cx: &App) -> Option<SharedString> {
        self.store.read(cx).server_url.clone()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected_issue_key(&self, cx: &App) -> Option<SharedString> {
        self.selected_issue(cx)
            .and_then(|id| self.store.read(cx).get(id))
            .map(|i| i.key.clone())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn help_open(&self) -> bool {
        self.help_open
    }

    #[cfg(test)]
    pub fn set_message(&mut self, message: String) {
        self.message = message.into();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The issue with this ID. Panics when the store lacks it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn issue<'a>(&self, id: IssueId, cx: &'a App) -> &'a Issue {
        self.store.read(cx).get(id).expect("issue in the store")
    }

    /// The issue with this key. Panics when the store lacks it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn issue_by_key<'a>(&self, key: &str, cx: &'a App) -> &'a Issue {
        self.store.read(cx).by_key(key).expect("issue in the store")
    }

    /// The claim on the task with this key, if an agent holds it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn claim_of<'a>(&self, key: &str, cx: &'a App) -> Option<&'a Claim> {
        self.issue_by_key(key, cx).claim.as_ref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn preview_offset(&self, cx: &App) -> gpui_kit::Pixels {
        self.task.read(cx).preview_offset()
    }

    /// A copy of every issue and the catalog, so a test can build a snapshot.
    #[cfg(test)]
    pub fn issues_and_catalog(&self, cx: &App) -> (Vec<Issue>, Catalog) {
        let store = self.store.read(cx);
        (store.iter().cloned().collect(), store.catalog.clone())
    }

    /// Runs `f` on the issue with this ID, or says in the status bar that it
    /// is gone. Menus, drags and clicks call this when they act, so a
    /// snapshot in between cannot send them to another issue.
    pub fn with_issue(
        &mut self,
        id: IssueId,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, IssueId, &mut Context<Self>),
    ) {
        if self.store.read(cx).get(id).is_some() {
            f(self, id, cx);
        } else {
            self.message = "That task is gone".into();
            cx.notify();
        }
    }

    fn selected_issue(&self, cx: &App) -> Option<IssueId> {
        self.list.read(cx).selected_issue()
    }

    /// The task commands act on: the open one, else the selected one.
    fn target(&self, cx: &App) -> Option<IssueId> {
        // The Settings view hides the task, so commands do not act on it.
        if self.settings_shown(cx) {
            return None;
        }
        self.task
            .read(cx)
            .detail()
            .map(|d| d.issue)
            .or(self.selected_issue(cx))
    }

    fn editing(&self, cx: &App) -> bool {
        self.task.read(cx).editing()
    }

    /// `"insert"` while a text field has focus, `"normal"` otherwise.
    pub fn vim_mode(&self, window: &Window, cx: &App) -> &'static str {
        let list = self.list.read(cx);
        let typing = self.palette.read(cx).is_open()
            || self.create.read(cx).is_open()
            || self.doc_view.read(cx).editor_focused(window, cx)
            || self.answers.read(cx).focused(window, cx)
            || self.settings_view.read(cx).typing(window, cx)
            || list.filter.focus_handle(cx).contains_focused(window, cx)
            || self.task.read(cx).editor_focused(window, cx);
        if typing { "insert" } else { "normal" }
    }

    /// Whether the task view, or the editor inside it, holds focus.
    pub fn detail_focused(&self, window: &Window, cx: &App) -> bool {
        self.task.read(cx).focused(window, cx)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn show_focus_outline(&self, window: &Window, cx: &App) -> bool {
        self.task.read(cx).show_focus_outline(window, cx)
    }

    fn focus_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn apply_filter(&mut self, query: &str, cx: &mut Context<Self>) {
        self.list_do(cx, |list, cx| list.set_query(query, cx));
        self.record_view(true, cx);
    }

    // Sorting and groups.

    /// Runs a click on an entry of the column heading menu.
    fn toggle_column(&mut self, action: &ToggleColumn, _: &mut Window, cx: &mut Context<Self>) {
        let column = action.0;
        self.list_do(cx, |list, cx| list.toggle_column(column, cx));
        self.persist(cx);
    }

    pub fn sort_by(&mut self, key: SortKey, cx: &mut Context<Self>) {
        self.list_do(cx, |list, cx| list.sort_by(key, cx));
        self.persist(cx);
    }

    pub fn group_by(&mut self, group: GroupBy, cx: &mut Context<Self>) {
        self.list_do(cx, |list, cx| list.group_by(group, cx));
        self.persist(cx);
    }

    fn cycle_group(&mut self, _: &CycleGroup, _: &mut Window, cx: &mut Context<Self>) {
        let next = self.list.read(cx).group.next();
        self.group_by(next, cx);
    }

    fn set_group_collapsed(&mut self, title: &str, collapsed: bool, cx: &mut Context<Self>) {
        self.list_do(cx, |list, cx| {
            list.set_group_collapsed(title, collapsed, cx)
        });
        self.persist(cx);
    }

    fn toggle_group(&mut self, _: &ToggleGroup, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((title, collapsed)) = self.list.read(cx).current_group() {
            self.set_group_collapsed(&title, !collapsed, cx);
        }
    }

    fn close_group(&mut self, _: &CloseGroup, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((title, _)) = self.list.read(cx).current_group() {
            self.set_group_collapsed(&title, true, cx);
        }
    }

    fn open_group(&mut self, _: &OpenGroup, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((title, _)) = self.list.read(cx).current_group() {
            self.set_group_collapsed(&title, false, cx);
        }
    }

    pub fn close_all_groups(&mut self, _: &CloseAllGroups, _: &mut Window, cx: &mut Context<Self>) {
        self.list_do(cx, |list, cx| list.close_all_groups(cx));
        self.persist(cx);
    }

    pub fn open_all_groups(&mut self, _: &OpenAllGroups, _: &mut Window, cx: &mut Context<Self>) {
        self.list_do(cx, |list, cx| list.open_all_groups(cx));
        self.persist(cx);
    }

    // Moving in the list.

    /// Ctrl-N in the palette or the filter.
    fn select_next(&mut self, _: &SelectNext, w: &mut Window, cx: &mut Context<Self>) {
        if self.palette.read(cx).is_open() {
            self.palette
                .update(cx, |palette, cx| palette.select_next(cx));
        } else {
            self.move_down(&MoveDown, w, cx);
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, w: &mut Window, cx: &mut Context<Self>) {
        if self.palette.read(cx).is_open() {
            self.palette
                .update(cx, |palette, cx| palette.select_prev(cx));
        } else {
            self.move_up(&MoveUp, w, cx);
        }
    }

    /// Ctrl-C: closes the help, and works like Esc while typing. Elsewhere it
    /// does nothing, as in nvim's normal mode.
    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        let task = self.task.read(cx);
        let editor_normal = task.editor_focused(window, cx) && task.editor_vim == VimMode::Normal;
        let typing = self.vim_mode(window, cx) == "insert" && !editor_normal;
        if self.help_open || typing {
            self.back(&Back, window, cx);
        }
    }

    pub fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.list_do(cx, |list, cx| list.move_down(cx));
        self.record_view(true, cx);
    }

    pub fn move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.list_do(cx, |list, cx| list.move_up(cx));
        self.record_view(true, cx);
    }

    fn go_top(&mut self, _: &GoTop, _: &mut Window, cx: &mut Context<Self>) {
        self.list_do(cx, |list, cx| list.select(0, cx));
        self.record_view(true, cx);
    }

    fn go_bottom(&mut self, _: &GoBottom, _: &mut Window, cx: &mut Context<Self>) {
        self.list_do(cx, |list, cx| list.select(usize::MAX, cx));
        self.record_view(true, cx);
    }

    /// Opens the issue with `key` in the task view, from a link in the
    /// preview. The list selects it too when it shows the issue.
    pub fn open_key(&mut self, key: &str, cx: &mut Context<Self>) {
        if (self.doc_shown(cx) && !self.doc_view.read(cx).editing)
            || self.inbox_shown(cx)
            || self.settings_shown(cx)
        {
            self.set_scope(Scope::All, cx);
        }
        let Some(id) = self.store.read(cx).by_key(key).map(|i| i.id) else {
            self.message = self
                .open_elsewhere(key, cx)
                .unwrap_or_else(|| format!("{key} is not in this workspace"))
                .into();
            cx.notify();
            return;
        };
        if self.editing(cx) {
            self.message = "Save or leave the editor first".into();
            cx.notify();
            return;
        }
        if let Some(row) = self.list.read(cx).row_of(id) {
            self.list_do(cx, |list, cx| list.select(row, cx));
        }
        self.task.update(cx, |task, cx| task.show(id, cx));
        self.fetch_open_details(cx);
        self.message = format!("Opened {key}").into();
        cx.notify();
    }

    /// A click on a task row: select it and open it to the side. Focus
    /// stays in the list, so j / k keep working.
    pub fn click_row(
        &mut self,
        id: IssueId,
        click: RowClick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.list.read(cx).row_of(id) else {
            return;
        };
        match click {
            RowClick::Plain => {
                self.list_do(cx, |list, cx| {
                    // A plain click ends a multi-selection, as in a file
                    // manager. A range after it starts at this row.
                    list.clear_marks();
                    list.select(row, cx);
                });
                self.open_selected(cx);
            }
            // A mark click selects the row but keeps the open task, so a
            // run of clicks does not reload the right panel.
            RowClick::Range => self.list_do(cx, |list, cx| {
                list.mark_range(id);
                list.select(row, cx);
            }),
            RowClick::Toggle => self.list_do(cx, |list, cx| {
                list.toggle_marked(id);
                list.select(row, cx);
            }),
        }
        self.focus_list(window, cx);
    }

    /// A click on a group header: select it and open or close the group.
    pub fn click_header(
        &mut self,
        row: usize,
        title: &str,
        collapsed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.list_do(cx, |list, cx| list.select(row, cx));
        self.focus_list(window, cx);
        self.set_group_collapsed(title, !collapsed, cx);
    }

    /// Enter: on a group header, opens or closes the group. On a task, opens
    /// it. When the task view already shows it, Enter moves focus into it.
    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((title, collapsed)) = self.list.read(cx).selected_header() {
            self.set_group_collapsed(&title, !collapsed, cx);
            return;
        }
        let Some(id) = self.selected_issue(cx) else {
            return;
        };
        if self.detail(cx).is_some_and(|d| d.issue == id) {
            if !self.detail_focused(window, cx) {
                self.focus_detail(&FocusDetail, window, cx);
            }
            return;
        }
        self.task.update(cx, |task, cx| task.show(id, cx));
        self.fetch_open_details(cx);
        cx.notify();
    }

    /// Shows the selected task in the task view, without moving focus.
    fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_issue(cx) else {
            return;
        };
        if self.detail(cx).is_some_and(|d| d.issue == id) {
            return;
        }
        if self.editing(cx) {
            // A click on another task saves the edit and opens that task.
            // A conflict waits for the owner, so a stray click never
            // writes over the agent's text.
            if self.task.read(cx).conflict {
                self.message = "Resolve the conflict first".into();
                cx.notify();
                return;
            }
            self.save_editor(cx);
        }
        self.task.update(cx, |task, cx| task.show(id, cx));
        self.fetch_open_details(cx);
        cx.notify();
    }

    fn focus_detail(&mut self, _: &FocusDetail, window: &mut Window, cx: &mut Context<Self>) {
        if self.inbox_shown(cx) {
            self.inbox
                .update(cx, |view, cx| view.take_focus(window, cx));
            return;
        }
        if self.doc_shown(cx) {
            self.doc_view
                .update(cx, |view, cx| view.take_focus(window, cx));
            return;
        }
        if self.detail(cx).is_none() {
            self.open(&Open, window, cx);
        }
        self.task.update(cx, |task, cx| task.take_focus(window, cx));
        cx.notify();
    }

    /// `h` and `Ctrl-A h`: from the task view to the list, and from the
    /// list into the sidebar when it is open.
    fn focus_list_action(&mut self, _: &FocusList, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar_focused(window, cx) {
            return;
        }
        if self.sidebar_shown() && self.focus.is_focused(window) {
            let focus = self.sidebar.read(cx).focus.clone();
            window.focus(&focus, cx);
            cx.notify();
            return;
        }
        self.focus_list(window, cx);
    }

    // The sidebar.

    /// Shows the issues of one team or project, or all of them. The
    /// selection stays on its issue when the new scope has it.
    pub fn set_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        if scope == Scope::Settings && !self.settings_shown(cx) {
            self.settings_return = (self.list.read(cx).scope.clone(), self.selected_issue(cx));
        }
        let name = scope.name(&self.store.read(cx).catalog);
        // A saved view's issues come from the server. Until they arrive,
        // the list says it is loading.
        let keep = matches!(&scope, Scope::View(n)
            if self.store.read(cx).view.as_ref().is_some_and(|v| v.name == *n));
        if !keep {
            self.store.update(cx, |store, cx| {
                store.view = None;
                cx.notify();
            });
        }
        self.close_task_outside(&scope, cx);
        if let Scope::View(view) = &scope {
            self.request(backend::Request::ViewIssues {
                name: view.to_string(),
            });
        }
        self.list.update(cx, |list, cx| {
            list.scope = scope.clone();
            cx.notify();
        });
        self.sync_doc_view(cx);
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.active = scope.clone();
            cx.notify();
        });
        let selected = self.selected_issue(cx);
        self.list_do(cx, |list, cx| {
            list.scope = scope;
            list.reload(selected, cx);
        });
        self.message = format!("Showing {name}").into();
        self.persist(cx);
    }

    /// LLM-149: a task from another team, project or saved view does not
    /// stay open when the list moves away from it. An open editor stays.
    fn close_task_outside(&mut self, scope: &Scope, cx: &mut Context<Self>) {
        self.check_view_task = matches!(scope, Scope::View(name)
            if self.store.read(cx).view.as_ref().is_none_or(|v| v.name != *name));
        let Some(detail) = self.task.read(cx).detail() else {
            return;
        };
        if detail.mode == DetailMode::Preview && !self.task_fits(scope, detail.issue, cx) {
            self.task.update(cx, |task, cx| task.close(cx));
        }
    }

    /// Whether the task panel may show `id` beside the list of `scope`.
    /// Only a team, a project and a saved view leave tasks out. A saved
    /// view whose issues have not come yet holds every task for now.
    fn task_fits(&self, scope: &Scope, id: IssueId, cx: &App) -> bool {
        let store = self.store.read(cx);
        let members = store.view.as_ref();
        match scope {
            Scope::View(name) if members.is_none_or(|v| v.name != *name) => true,
            Scope::Team(_) | Scope::Project(_) | Scope::View(_) => store
                .get(id)
                .is_none_or(|issue| scope.matches(issue, members)),
            _ => true,
        }
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, window: &mut Window, cx: &mut Context<Self>) {
        self.set_sidebar_open(!self.sidebar_shown(), window, cx);
    }

    pub fn sidebar_shown(&self) -> bool {
        self.layout.sidebar_shown()
    }

    /// The owner shows or hides the sidebar by hand. Showing it in a narrow
    /// window overrides the auto-hide until the window widens.
    fn set_sidebar_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.layout.set_sidebar_open(open);
        self.sync_mascot(cx);
        self.leave_hidden_sidebar(window, cx);
        cx.notify();
        self.persist(cx);
    }

    /// Hides the sidebar when the window gets narrower than
    /// `SIDEBAR_AUTO_HIDE`, and shows it again when the window widens. It
    /// never changes `sidebar_open`, the owner's choice.
    fn update_narrow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.layout.set_narrow(is_narrow(window)) {
            return;
        }
        self.sync_mascot(cx);
        self.leave_hidden_sidebar(window, cx);
        cx.notify();
    }

    /// Runs the cat's timer only while the sidebar is on screen and the
    /// cat is on.
    fn sync_mascot(&mut self, cx: &mut Context<Self>) {
        let shown = self.mascot_on && self.sidebar_shown();
        self.mascot
            .update(cx, |mascot, cx| mascot.set_shown(shown, cx));
    }

    #[cfg(test)]
    pub fn mascot(&self) -> &Entity<Mascot> {
        &self.mascot
    }

    /// Moves focus from a sidebar that is no longer on screen to the list.
    fn leave_hidden_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sidebar_shown() && self.sidebar_focused(window, cx) {
            self.focus_list(window, cx);
        }
    }

    fn sidebar_step(&mut self, step: isize, cx: &mut Context<Self>) {
        let scope = self
            .sidebar
            .update(cx, |sidebar, cx| sidebar.step(step, cx));
        self.set_scope(scope, cx);
        self.record_view(true, cx);
    }

    fn sidebar_down(&mut self, _: &SidebarDown, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_step(1, cx);
    }

    fn sidebar_up(&mut self, _: &SidebarUp, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_step(-1, cx);
    }

    fn sidebar_top(&mut self, _: &SidebarTop, _: &mut Window, cx: &mut Context<Self>) {
        self.set_scope(Scope::All, cx);
    }

    fn sidebar_bottom(&mut self, _: &SidebarBottom, _: &mut Window, cx: &mut Context<Self>) {
        let scope = self.sidebar.update(cx, |sidebar, cx| sidebar.last(cx));
        self.set_scope(scope, cx);
    }

    fn leave_sidebar(&mut self, _: &LeaveSidebar, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_shown(cx) {
            self.settings_view
                .update(cx, |view, cx| view.take_focus(window, cx));
            return;
        }
        if self.inbox_shown(cx) {
            self.inbox
                .update(cx, |view, cx| view.take_focus(window, cx));
            return;
        }
        if self.doc_shown(cx) {
            self.doc_view
                .update(cx, |view, cx| view.take_focus(window, cx));
            return;
        }
        self.focus_list(window, cx);
    }

    /// A click on a sidebar row moves focus to the list, so j / k move
    /// through the issues it now shows.
    pub fn focus_list_from_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_shown(cx) {
            self.settings_view
                .update(cx, |view, cx| view.take_focus(window, cx));
            return;
        }
        self.focus_list(window, cx);
    }

    // The Settings view.

    pub fn settings_shown(&self, cx: &App) -> bool {
        self.list.read(cx).scope == Scope::Settings
    }

    pub fn list(&self) -> &Entity<IssueList> {
        &self.list
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn settings_view(&self) -> &Entity<SettingsView> {
        &self.settings_view
    }

    /// `:settings`, `Cmd-,` and the app menu.
    pub fn show_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings_shown(cx) {
            self.set_scope(Scope::Settings, cx);
        }
        self.settings_view
            .update(cx, |view, cx| view.take_focus(window, cx));
    }

    /// Back to the scope and the selected task under the Settings view.
    fn leave_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (scope, selected) = std::mem::take(&mut self.settings_return);
        self.set_scope(scope, cx);
        if let Some(row) = selected.and_then(|id| self.list.read(cx).row_of(id)) {
            self.list_do(cx, |list, cx| list.select(row, cx));
        }
        self.leave_sidebar(&LeaveSidebar, window, cx);
    }

    pub fn set_hide_closed(&mut self, on: bool, cx: &mut Context<Self>) {
        self.hide_closed = on;
        self.apply_hide_closed(cx);
    }

    pub fn set_hide_closed_days(&mut self, days: u32, cx: &mut Context<Self>) {
        let days = days.clamp(1, MAX_HIDE_DAYS);
        if days != self.hide_closed_days {
            self.hide_closed_days = days;
            self.apply_hide_closed(cx);
        }
    }

    fn apply_hide_closed(&mut self, cx: &mut Context<Self>) {
        let after = self.hide_closed.then_some(self.hide_closed_days);
        let selected = self.selected_issue(cx);
        self.list_do(cx, |list, cx| {
            list.hide_closed_after = after;
            list.reload(selected, cx);
        });
        self.persist(cx);
    }

    pub fn set_vim_editor(&mut self, on: bool, cx: &mut Context<Self>) {
        self.task.update(cx, |task, cx| {
            task.vim_enabled = on;
            // An open editor must not stay stuck in normal mode.
            if !on {
                task.editor_vim = VimMode::Insert;
            }
            cx.notify();
        });
        self.persist(cx);
    }

    /// The Settings switch for the cat. Off, the cat and its strip leave
    /// the sidebar. On again, it walks in as when the sidebar opens.
    pub fn set_mascot(&mut self, on: bool, cx: &mut Context<Self>) {
        self.mascot_on = on;
        self.sync_mascot(cx);
        cx.notify();
        self.persist(cx);
    }

    /// The experimental Settings switch for the line through Done titles.
    pub fn set_strike_done(&mut self, on: bool, cx: &mut Context<Self>) {
        self.list_do(cx, |list, cx| {
            list.strike_done = on;
            cx.notify();
        });
        self.persist(cx);
    }

    pub fn set_open_last_view(&mut self, on: bool, cx: &mut Context<Self>) {
        self.open_last_view = on;
        self.persist(cx);
    }

    /// The Settings switch and `:theme` both come here.
    pub fn set_dark(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mode = if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
        crate::shell::theme::set_mode(mode, cx);
        window.refresh();
        self.persist(cx);
    }

    /// The Settings theme picker and `:theme <name>` both come here.
    pub fn set_color_theme(
        &mut self,
        name: crate::shell::theme::ColorTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::shell::theme::set_color_theme(name, cx);
        window.refresh();
        self.persist(cx);
    }

    /// Asks for the open task's history. Without a server, as in tests on
    /// demo data, there is none, and the task view has no Activity tab.
    fn request_history(&self, cx: &App) {
        let Some(key) = self
            .history_for
            .and_then(|id| self.store.read(cx).get(id))
            .map(|i| i.key.to_string())
        else {
            return;
        };
        self.request(backend::Request::History { key });
    }

    fn request(&self, request: backend::Request) {
        if let Some(backend) = &self.backend {
            backend.request(request);
        }
    }

    /// Asks again for the issues of the saved view the list shows, once
    /// issues stop changing for a moment.
    fn refresh_view(&mut self, cx: &mut Context<Self>) {
        let Scope::View(name) = self.list.read(cx).scope.clone() else {
            return;
        };
        if self.backend.is_none() || self.view_refresh.is_some() {
            return;
        }
        self.view_refresh = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(VIEW_REFRESH_AFTER).await;
            this.update(cx, |this, _| {
                this.view_refresh = None;
                this.request(backend::Request::ViewIssues {
                    name: name.to_string(),
                });
            })
            .ok();
        }));
    }

    /// `:view save <name>`: the sidebar scope and the filter, as a saved
    /// view on the server.
    fn save_view(&mut self, name: &str, cx: &mut Context<Self>) -> String {
        if name.is_empty() {
            return "Name the view: :view save <name>".into();
        }
        let list = self.list.read(cx);
        let filter = Filter::parse(&list.filter.read(cx).value());
        let query = match filter.to_query(&list.scope, &self.store.read(cx).catalog) {
            Ok(query) => query,
            Err(message) => return message,
        };
        if self.backend.is_none() {
            return "No server to save the view on".into();
        }
        self.request(backend::Request::CreateView(Box::new(
            llmtodo_client::types::CreateView {
                name: name.to_string(),
                query,
            },
        )));
        format!("Saving the view {name}")
    }

    // Documents.

    /// The document the sidebar picked, if it picked one.
    fn open_doc_id(&self, cx: &App) -> Option<llmtodo_core::DocumentId> {
        match &self.list.read(cx).scope {
            Scope::Document(id) => id.parse().ok(),
            _ => None,
        }
    }

    /// Shows the picked document, and asks for its text when the view has
    /// none or the server has a newer version. The owner's edit is never
    /// replaced: the save finds out.
    fn sync_doc_view(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.open_doc_id(cx) else {
            if self.doc_view.read(cx).open.is_some() {
                self.doc_view.update(cx, |view, cx| view.close(cx));
            }
            return;
        };
        let newer = self
            .store
            .read(cx)
            .catalog
            .documents
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.version);
        let view = self.doc_view.read(cx);
        let held = view.doc().filter(|d| d.id == id).map(|d| d.version);
        let ask = view.open != Some(id) || held.is_none() || (newer > held && !view.editing);
        self.doc_view.update(cx, |view, cx| view.show(id, cx));
        if ask {
            self.request(backend::Request::Document { id });
        }
    }

    pub fn doc_shown(&self, cx: &App) -> bool {
        self.open_doc_id(cx).is_some()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn doc_view(&self) -> &Entity<DocView> {
        &self.doc_view
    }

    /// `E` on a document: the editor, with the owner's refused text when a
    /// save failed.
    pub fn edit_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.refuse_read_only_tracker(cx) {
            return;
        }
        let Some(id) = self.open_doc_id(cx) else {
            return;
        };
        let draft = self
            .doc_draft
            .as_ref()
            .filter(|(d, _)| *d == id)
            .map(|(_, t)| t.clone());
        if draft.is_some() {
            self.message =
                "Your unsaved text is back. :w saves it, :w! overwrites the server's version"
                    .into();
        }
        self.doc_view
            .update(cx, |view, cx| view.start_editing(draft, window, cx));
        cx.notify();
    }

    /// Saves the document editor's text, if it changed.
    fn save_document(&mut self, force: bool, cx: &mut Context<Self>) {
        let view = self.doc_view.read(cx);
        let (Some(doc), true) = (view.doc().cloned(), view.editing) else {
            return;
        };
        let text = view.editor.read(cx).value().to_string();
        let base = view.base;
        let has_draft = self.doc_draft.as_ref().is_some_and(|(d, _)| *d == doc.id);
        if text == doc.body.clone().unwrap_or_default() && !force && !has_draft {
            self.message = format!("No changes in {}", doc.title).into();
            return;
        }
        if self.doc_stale && !force {
            self.message =
                "The server has a newer version, so :w would overwrite it. :w! does".into();
            return;
        }
        if self.backend.is_none() {
            self.message = "No server to save the document on".into();
            return;
        }
        self.doc_draft = Some((doc.id, text.clone()));
        self.request(backend::Request::SaveDocument {
            id: doc.id,
            body: text,
            base,
            force,
        });
        self.message = format!("Saving {}", doc.title).into();
    }

    /// Esc or the Preview button in the document editor: save and show the
    /// preview.
    pub fn leave_doc_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.doc_view.read(cx).editing {
            return;
        }
        self.save_document(false, cx);
        self.doc_view
            .update(cx, |view, cx| view.stop_editing(window, cx));
        cx.notify();
    }

    // The owner inbox.

    pub fn inbox_shown(&self, cx: &App) -> bool {
        self.list.read(cx).scope == Scope::Inbox
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn inbox(&self) -> &Entity<InboxView> {
        &self.inbox
    }

    /// `g i` and `:inbox`: every open owner question, with focus on them.
    pub fn show_inbox(&mut self, _: &ShowInbox, window: &mut Window, cx: &mut Context<Self>) {
        self.help_open = false;
        if !self.inbox_shown(cx) {
            self.set_scope(Scope::Inbox, cx);
        }
        self.inbox
            .update(cx, |view, cx| view.take_focus(window, cx));
        cx.notify();
    }

    #[cfg(test)]
    pub fn answers(&self) -> &Entity<AnswerInputs> {
        &self.answers
    }

    pub fn show_message(&mut self, message: String, cx: &mut Context<Self>) {
        self.message = message.into();
        cx.notify();
    }

    /// LLM-199: shows `text` in a small card above the pointer for
    /// [`FLASH_FOR`]. It confirms a click that changes nothing else in the
    /// window, such as a copy. The card draws on the next frame.
    pub fn flash(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = text.into();
        let view = crate::shell::tooltip::Card::text(text.clone()).build(window, cx);
        let expire = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FLASH_FOR).await;
            this.update(cx, |this, cx| {
                this.flash = None;
                cx.notify();
            })
            .ok();
        });
        self.flash = Some(Flash {
            text,
            at: window.mouse_position(),
            view,
            _expire: expire,
        });
        cx.notify();
    }

    /// The text of the flash card on screen, if one shows.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn flash_text(&self) -> Option<&str> {
        self.flash.as_ref().map(|f| f.text.as_ref())
    }

    /// Sends Approve or Request changes for a ticket in User review, with
    /// the text of its feedback field. The server moves the ticket, so the
    /// app waits for its copy instead of changing its own.
    pub fn answer_review(
        &mut self,
        issue: IssueId,
        verdict: llmtodo_api_types::ReviewVerdict,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use llmtodo_api_types::ReviewVerdict;
        if self.refuse_read_only_tracker(cx) {
            return;
        }
        let Some(key) = self.store.read(cx).get(issue).map(|i| i.key.clone()) else {
            return;
        };
        if self.task.read(cx).review_sending.contains_key(&issue) {
            self.show_message("Wait: the last review answer is still sending".into(), cx);
            return;
        }
        let feedback = self.answers.read(cx).feedback_text(issue, cx);
        if verdict == ReviewVerdict::ChangesRequested && feedback.is_empty() {
            self.show_message(
                format!("Write what to change on {key} in the feedback field first"),
                cx,
            );
            return;
        }
        if self.backend.is_none() {
            self.show_message("No server, so the review answer was not sent".into(), cx);
            return;
        }
        if verdict == ReviewVerdict::Approved && self.panel_open(cx) {
            self.approved_with_panel.insert(issue);
        } else {
            self.approved_with_panel.remove(&issue);
        }
        self.task.update(cx, |task, cx| {
            task.review_sending.insert(issue, verdict);
            task.review_error.remove(&issue);
            cx.notify();
        });
        self.message = match verdict {
            ReviewVerdict::Approved => format!("Approving {key}"),
            ReviewVerdict::ChangesRequested => format!("Asking for changes on {key}"),
        }
        .into();
        self.send(
            issue,
            Write::Review(llmtodo_api_types::SubmitReview { verdict, feedback }),
            WriteKind::Review,
            cx,
        );
        cx.notify();
    }

    /// The server took the review answer. The message says where the
    /// ticket went.
    fn review_done(&mut self, key: &str, cx: &mut Context<Self>) {
        use llmtodo_api_types::ReviewVerdict;
        let Some(issue) = self.store.read(cx).by_key(key).map(|i| i.id) else {
            return;
        };
        let sent = self
            .task
            .update(cx, |task, _| task.review_sending.remove(&issue));
        let Some(verdict) = sent else {
            return;
        };
        let with_panel = self.approved_with_panel.remove(&issue);
        if self.batch_answered(issue, key, None, cx) {
            return;
        }
        self.answers
            .update(cx, |a, cx| a.clear_feedback_later(issue, cx));
        let status = self.store.read(cx).get(issue).map(|i| i.status.clone());
        self.message = match (verdict, status) {
            (ReviewVerdict::Approved, Some(s)) if s.is_closed() => {
                format!("Approved {key}. It moved to {}", s.name)
            }
            (ReviewVerdict::Approved, _) => {
                format!("Approved {key}. An agent will merge its PR and close it")
            }
            (ReviewVerdict::ChangesRequested, s) => format!(
                "Asked for changes on {key}. It moved to {} for an agent",
                s.map_or_else(|| "Todo".into(), |s| s.name)
            ),
        }
        .into();
        if verdict == ReviewVerdict::Approved {
            self.open_next_review(&[issue], with_panel, cx);
        }
        cx.notify();
    }

    /// Whether a task panel is open, in preview or in the editor.
    pub(crate) fn panel_open(&self, cx: &App) -> bool {
        self.task.read(cx).detail().is_some()
    }

    /// LLM-155: after the owner approves a review ticket or sets it to
    /// Done, selects the first row in the list that is still in a review
    /// status. `done` names the tickets that just left review. The list
    /// scrolls only when that row is out of view. With no review ticket
    /// left, nothing changes. LLM-168: the task panel shows the row only
    /// when `open` is set, which says a panel was open when the owner
    /// acted.
    pub(crate) fn open_next_review(
        &mut self,
        done: &[IssueId],
        open: bool,
        cx: &mut Context<Self>,
    ) {
        let next = {
            let store = self.store.read(cx);
            self.list
                .read(cx)
                .rows()
                .iter()
                .enumerate()
                .find_map(|(row, r)| match r {
                    Row::Issue(id) if !done.contains(id) => store
                        .get(*id)
                        .filter(|i| i.status.review && !i.deleted)
                        .map(|_| (row, *id)),
                    _ => None,
                })
        };
        let Some((row, id)) = next else { return };
        self.list_do(cx, |list, cx| list.select(row, cx));
        if open {
            self.task.update(cx, |task, cx| task.show(id, cx));
            self.fetch_open_details(cx);
        }
    }

    /// The server refused the review answer. The panel shows why.
    fn review_failed(&mut self, key: &str, message: String, cx: &mut Context<Self>) {
        let issue = self.store.read(cx).by_key(key).map(|i| i.id);
        if let Some(issue) = issue {
            self.approved_with_panel.remove(&issue);
        }
        let sent = issue.filter(|issue| {
            self.task
                .update(cx, |task, _| task.review_sending.remove(issue))
                .is_some()
        });
        let text: SharedString = format!("Could not send the review of {key}: {message}").into();
        if let Some(issue) = sent {
            self.task.update(cx, |task, cx| {
                task.review_error.insert(issue, text.clone());
                cx.notify();
            });
            if self.batch_answered(issue, key, Some(message), cx) {
                return;
            }
        }
        self.message = text;
        cx.notify();
    }

    /// Answers owner question `number` of `issue`. A single option letter
    /// picks that option. The app's copy changes at once, and so does
    /// `waiting_on` when no question stays open, as the server does. A
    /// claimed task takes answers too: the agent that holds it waits on them.
    pub fn answer_question(
        &mut self,
        issue: IssueId,
        number: u32,
        answer: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.refuse_read_only_tracker(cx) {
            return;
        }
        let answer = answer.trim().to_string();
        let id = format!("Q{number}");
        if answer.is_empty() {
            self.show_message(format!("Type an answer to {id} first"), cx);
            return;
        }
        let has_text = self
            .store
            .read(cx)
            .get(issue)
            .is_some_and(|i| i.description_loaded);
        let result = if has_text {
            self.edit_issue(issue, cx, |issue, _| {
                let text = llmtodo_core::questions::answer(&issue.description, &id, &answer)
                    .map_err(|e| format!("Could not answer {} {id}: {e}", issue.key))?;
                let qs = llmtodo_core::questions(&text);
                let given = qs
                    .iter()
                    .find(|q| q.number == number)
                    .and_then(|q| q.answer.clone())
                    .unwrap_or_default();
                let open = qs.iter().filter(|q| q.is_open()).count();
                issue.description = text.into();
                Ok::<_, String>(answered(issue, &id, &given, open))
            })
        } else {
            // The inbox shows questions from the server's list, and the app
            // may not hold the text. The answer leaves the list at once, as
            // it does with the text, and the server's copy follows.
            self.store.update(cx, |store, cx| {
                cx.notify();
                let key = store.get(issue)?.key.clone();
                let Some(question) = store
                    .open_questions_of(issue)
                    .iter()
                    .find(|q| q.number == number)
                    .cloned()
                else {
                    return Some(Err(format!("Could not answer {key} {id}: it is not open")));
                };
                let given = question.answer_text(&answer);
                let open = store.take_open_question(issue, number)?;
                store.modify(issue, |issue| Ok(answered(issue, &id, &given, open)))
            })
        };
        let Some(result) = result else {
            return;
        };
        match result {
            Ok(message) => {
                self.answers
                    .update(cx, |a, cx| a.clear(issue, number, window, cx));
                if self.answers.read(cx).focused(window, cx) {
                    if self.inbox_shown(cx) {
                        self.inbox
                            .update(cx, |view, cx| view.take_focus(window, cx));
                    } else {
                        window.focus(&self.task.read(cx).focus.clone(), cx);
                    }
                }
                self.message = message.clone().into();
                self.offer_answer_undo(issue, number, message, cx);
                let update = llmtodo_client::types::UpdateIssue {
                    description: llmtodo_client::types::BodyChange {
                        patches: vec![llmtodo_core::Patch::AnswerQuestion { id, answer }],
                        ..Default::default()
                    },
                    ..Default::default()
                };
                self.send(
                    issue,
                    Write::Update(Box::new(update)),
                    WriteKind::Answer,
                    cx,
                );
            }
            Err(message) => self.message = message.into(),
        }
        cx.notify();
    }

    /// Shows the Undo button for a few seconds. Only the newest answer has
    /// one.
    fn offer_answer_undo(
        &mut self,
        issue: IssueId,
        number: u32,
        message: String,
        cx: &mut Context<Self>,
    ) {
        let expire = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(UNDO_ANSWER_FOR).await;
            this.update(cx, |this, cx| {
                this.answer_undo = None;
                cx.notify();
            })
            .ok();
        });
        self.answer_undo = Some(AnswerUndo {
            issue,
            number,
            message: message.into(),
            _expire: expire,
        });
    }

    /// Asks the server to reopen the question of the newest answer. The
    /// server can refuse, so the question comes back only with its copy.
    pub fn undo_answer(&mut self, cx: &mut Context<Self>) {
        if self.refuse_read_only_tracker(cx) {
            return;
        }
        let Some(undo) = self.answer_undo.take() else {
            self.show_message("No answer to undo".into(), cx);
            return;
        };
        let id = format!("Q{}", undo.number);
        if let Some(key) = self.store.read(cx).get(undo.issue).map(|i| i.key.clone()) {
            self.message = format!("Undoing the answer to {key} {id}").into();
            self.undoing = Some((undo.issue, undo.number));
            self.send(
                undo.issue,
                Write::UndoAnswer { id },
                WriteKind::UndoAnswer,
                cx,
            );
        }
        cx.notify();
    }

    /// The server reopened the question: it is back in the list, selected.
    fn undo_answer_done(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some((issue, number)) = self.undoing.take() {
            self.message = format!("Reopened {key} Q{number}").into();
            self.inbox
                .update(cx, |view, cx| view.select_question(issue, number, cx));
        }
        cx.notify();
    }

    /// An answer the server refused has nothing to undo.
    fn drop_answer_undo_for(&mut self, key: &str, cx: &App) {
        let id = self.store.read(cx).by_key(key).map(|i| i.id);
        if self.answer_undo.as_ref().map(|u| u.issue) == id {
            self.answer_undo = None;
        }
    }

    // Marks and bulk actions.

    /// `x`: marks the selected task, or clears its mark, and moves down so
    /// a run of `x` marks a run of tasks.
    fn toggle_mark(&mut self, _: &ToggleMark, _: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_issue(cx) else {
            return;
        };
        self.list_do(cx, |list, cx| {
            list.toggle_marked(id);
            list.move_down(cx);
        });
    }

    fn clear_marks(&mut self, cx: &mut Context<Self>) -> bool {
        let had = !self.list.read(cx).marked.is_empty();
        if had {
            self.list_do(cx, |list, cx| {
                list.clear_marks();
                cx.notify();
            });
            self.message = "Cleared the marks".into();
        }
        had
    }

    /// The marked tasks the store still has, in list order, then the rest.
    pub fn marked(&self, cx: &App) -> Vec<IssueId> {
        let list = self.list.read(cx);
        let store = self.store.read(cx);
        let mut out: Vec<IssueId> = list
            .visible()
            .iter()
            .copied()
            .filter(|id| list.marked.contains(id))
            .collect();
        let mut rest: Vec<IssueId> = list
            .marked
            .iter()
            .copied()
            .filter(|id| !out.contains(id) && store.get(*id).is_some())
            .collect();
        rest.sort();
        out.extend(rest);
        out
    }

    // Creating issues.

    /// The team and project a new issue goes to: the sidebar's team or
    /// project, else the selected issue's team, else the first team.
    fn new_issue_place(&self, cx: &App) -> (Option<SharedString>, Option<SharedString>) {
        let store = self.store.read(cx);
        let selected_team = self
            .selected_issue(cx)
            .and_then(|id| store.get(id))
            .map(|i| i.team.clone());
        let first_team = store.catalog.team_names.first().map(|t| t.key.clone());
        match self.list.read(cx).scope.clone() {
            Scope::Team(key) => (Some(key), None),
            Scope::Project(slug) => (selected_team.or(first_team), Some(slug)),
            _ => (selected_team.or(first_team), None),
        }
    }

    /// `c`: opens the form, which says where the issue will go.
    fn new_issue(&mut self, _: &NewIssue, window: &mut Window, cx: &mut Context<Self>) {
        if self.refuse_read_only_tracker(cx) {
            return;
        }
        let (team, project) = self.new_issue_place(cx);
        let catalog = &self.store.read(cx).catalog;
        let mut place = match &team {
            Some(key) => format!("in {}", Scope::Team(key.clone()).name(catalog)),
            None => "in the default team".to_string(),
        };
        if let Some(slug) = &project {
            place.push_str(&format!(
                ", project {}",
                Scope::Project(slug.clone()).name(catalog)
            ));
        }
        let place: SharedString = place.into();
        self.create
            .update(cx, |form, cx| form.open(place, window, cx));
        cx.notify();
    }

    /// Sends a new issue to the server. The text is a title with optional
    /// `status:`, `label:`, `priority:` and `assignee:` words. Unknown
    /// words stop the create, so nothing goes out half right. Returns
    /// whether the issue went to the server.
    pub fn create_issue(
        &mut self,
        text: &str,
        description: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.refuse_read_only_tracker(cx) {
            return false;
        }
        let draft = Draft::parse(text);
        let (team, project) = self.new_issue_place(cx);
        let catalog = self.store.read(cx).catalog.clone();
        let result = (|| {
            if draft.title.is_empty() {
                return Err("Type a title for the new issue".to_string());
            }
            let status = match &draft.status {
                Some(word) => {
                    let team = team.as_deref().unwrap_or_default();
                    match catalog.find_status(team, word) {
                        Some(s) => Some(s.name.to_string()),
                        None => return Err(format!("Unknown status: {word}")),
                    }
                }
                None => None,
            };
            let mut labels = Vec::new();
            for word in &draft.labels {
                match catalog.find_label(word) {
                    Some(l) => labels.push(l.name.to_string()),
                    None => return Err(format!("Unknown label: {word}")),
                }
            }
            let priority = match &draft.priority {
                Some(word) => match create::priority(word) {
                    Some(p) => Some(p),
                    None => return Err(format!("Unknown priority: {word}")),
                },
                None => None,
            };
            // The app shows the owner as `me`; the server's `me` is the
            // caller, which is the owner here too.
            let assignee = draft.assignee.clone();
            Ok(llmtodo_client::types::CreateIssue {
                team: team.as_ref().map(|t| t.to_string()),
                title: draft.title.clone(),
                description: description.to_string(),
                status,
                priority,
                assignee,
                labels,
                project: project.as_ref().map(|p| p.to_string()),
                milestone: None,
                parent: None,
                wanted_executor: None,
                waiting_on: None,
                relations: Default::default(),
                estimate_minutes: None,
                estimate_cost_usd: None,
            })
        })();
        let mut sent = false;
        self.message = match result {
            Err(message) => message,
            Ok(_) if self.backend.is_none() => "No server to create the issue on".into(),
            Ok(create) => {
                self.request(backend::Request::Create(Box::new(create)));
                sent = true;
                format!("Creating \"{}\"", draft.title)
            }
        }
        .into();
        cx.notify();
        sent
    }

    /// Selects a new issue and opens it. When the list's scope or filter
    /// hides it, the status bar says so.
    fn select_created(&mut self, id: IssueId, key: &str, cx: &mut Context<Self>) {
        self.list_do(cx, |list, cx| list.refilter(cx));
        match self.list.read(cx).row_of(id) {
            Some(row) => {
                self.list_do(cx, |list, cx| list.select(row, cx));
                if !self.editing(cx) {
                    self.task.update(cx, |task, cx| task.show(id, cx));
                }
            }
            None => {
                self.message =
                    format!("Created {key}. The list hides it: :all and Esc show every issue")
                        .into();
            }
        }
    }

    /// `:team LLM` and `:project app` take a key, a slug or a name.
    fn find_place(places: &[crate::model::data::Place], word: &str) -> Option<SharedString> {
        places
            .iter()
            .find(|p| p.key.eq_ignore_ascii_case(word) || p.name.eq_ignore_ascii_case(word))
            .map(|p| p.key.clone())
    }

    // Scrolling the task view.

    fn scroll_down(&mut self, _: &ScrollDown, window: &mut Window, cx: &mut Context<Self>) {
        self.task
            .update(cx, |task, cx| task.scroll_lines(true, window, cx));
    }

    fn scroll_up(&mut self, _: &ScrollUp, window: &mut Window, cx: &mut Context<Self>) {
        self.task
            .update(cx, |task, cx| task.scroll_lines(false, window, cx));
    }

    fn half_page_down(&mut self, _: &HalfPageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.task
            .update(cx, |task, cx| task.scroll_half_page(true, cx));
    }

    fn half_page_up(&mut self, _: &HalfPageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.task
            .update(cx, |task, cx| task.scroll_half_page(false, cx));
    }

    fn scroll_top(&mut self, _: &ScrollTop, _: &mut Window, cx: &mut Context<Self>) {
        self.task.update(cx, |task, cx| task.scroll_top(cx));
    }

    fn scroll_bottom(&mut self, _: &ScrollBottom, _: &mut Window, cx: &mut Context<Self>) {
        self.task.update(cx, |task, cx| task.scroll_bottom(cx));
    }

    fn back(&mut self, _: &Back, window: &mut Window, cx: &mut Context<Self>) {
        // Esc in an answer field goes back to the questions it answers.
        if self.answers.read(cx).focused(window, cx) {
            if self.inbox_shown(cx) {
                self.inbox
                    .update(cx, |view, cx| view.take_focus(window, cx));
            } else {
                window.focus(&self.task.read(cx).focus.clone(), cx);
            }
            cx.notify();
            return;
        }
        if self.doc_view.read(cx).editor_focused(window, cx) {
            self.leave_doc_editor(window, cx);
            return;
        }
        if self.doc_view.read(cx).focused(window, cx) {
            self.focus_list(window, cx);
            return;
        }
        let filter = self.list.read(cx).filter.clone();
        if self.help_open {
            self.help_open = false;
            cx.notify();
        } else if self.claims.read(cx).is_open() {
            self.claims
                .update(cx, |claims, cx| claims.close(window, cx));
        } else if self.palette.read(cx).is_open() {
            self.palette
                .update(cx, |palette, cx| palette.close(window, cx));
        } else if self.create.read(cx).is_open() {
            self.create.update(cx, |form, cx| form.close(window, cx));
        } else if self.settings_shown(cx) {
            self.leave_settings(window, cx);
        } else if filter.focus_handle(cx).contains_focused(window, cx) {
            self.focus_list(window, cx);
        } else if self.task.read(cx).focus.contains_focused(window, cx) {
            // Leave the task view, keep it open.
            self.focus_list(window, cx);
        } else if self
            .detail(cx)
            .is_none_or(|d| d.mode == DetailMode::Preview)
            && self.clear_marks(cx)
        {
            cx.notify();
        } else if let Some(detail) = self.detail(cx) {
            match detail.mode {
                // With vim keys off there is no normal mode: Esc saves.
                DetailMode::Editing
                    if self.task.read(cx).vim_enabled
                        && self.task.read(cx).editor_vim == VimMode::Insert =>
                {
                    self.task.update(cx, |task, cx| {
                        task.editor_vim = VimMode::Normal;
                        cx.notify();
                    });
                    return;
                }
                DetailMode::Editing => {
                    self.save_editor(cx);
                    // Back in the preview, still inside the task view.
                    let focus = self.task.read(cx).focus.clone();
                    window.focus(&focus, cx);
                    cx.notify();
                    return;
                }
                DetailMode::Preview => self.task.update(cx, |task, cx| task.close(cx)),
            }
            self.focus_list(window, cx);
        } else if !filter.read(cx).value().is_empty() {
            filter.update(cx, |state, cx| state.set_value("", window, cx));
            self.apply_filter("", cx);
        }
    }

    // The view history.

    /// What the window shows now.
    fn current_view(&self, cx: &App) -> View {
        View {
            scope: self.list.read(cx).scope.clone(),
            task: self.detail(cx).map(|detail| {
                let tab = self.open_tab(cx).map_or(0, |(ix, _)| ix);
                (detail.issue, tab)
            }),
            claims: self.claims.read(cx).is_open(),
        }
    }

    /// Notes the view the window shows. `walking` means the owner stepped
    /// through rows or the data changed, so the view takes the current
    /// place in the history instead of adding one.
    fn record_view(&mut self, walking: bool, cx: &App) {
        if self.store.read(cx).is_empty() {
            return;
        }
        let view = self.current_view(cx);
        if walking {
            self.views.replace(view);
        } else {
            self.views.visit(view);
        }
    }

    /// `Ctrl-O` and mouse button 4: the view before this one.
    fn view_back(&mut self, _: &ViewBack, window: &mut Window, cx: &mut Context<Self>) {
        self.step_view(true, window, cx);
    }

    /// `Ctrl-I` and mouse button 5: the view after this one.
    fn view_forward(&mut self, _: &ViewForward, window: &mut Window, cx: &mut Context<Self>) {
        self.step_view(false, window, cx);
    }

    fn step_view(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        // An overlay over the view closes first, as with Esc.
        if self.help_open || self.palette.read(cx).is_open() || self.create.read(cx).is_open() {
            self.back(&Back, window, cx);
            return;
        }
        if self.editing(cx) || self.doc_view.read(cx).editing {
            self.message = "Save or leave the editor first".into();
            cx.notify();
            return;
        }
        // Observers run after this event, so note the view it started from.
        self.record_view(false, cx);
        let store = self.store.read(cx);
        let exists = |view: &View| view.exists(store);
        let target = if back {
            self.views.back(exists)
        } else {
            self.views.forward(exists)
        };
        match target {
            Some(view) => self.show_view(view, window, cx),
            None => {
                self.message = if back {
                    "No earlier view"
                } else {
                    "No later view"
                }
                .into();
                cx.notify();
            }
        }
    }

    /// Shows a view from the history.
    fn show_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        if self.list.read(cx).scope != view.scope {
            self.set_scope(view.scope.clone(), cx);
        }
        // LLM-149: a task opened beside another team or project stays
        // closed, so back and forward never bring it in by surprise.
        let task = view
            .task
            .filter(|(id, _)| self.task_fits(&view.scope, *id, cx));
        match task {
            Some((id, tab)) => {
                if let Some(row) = self.list.read(cx).row_of(id) {
                    self.list_do(cx, |list, cx| list.select(row, cx));
                }
                self.task.update(cx, |task, cx| task.show(id, cx));
                self.set_tab(tab, cx);
                self.fetch_open_details(cx);
            }
            None => self.task.update(cx, |task, cx| task.close(cx)),
        }
        if view.claims != self.claims.read(cx).is_open() {
            self.claims.update(cx, |claims, cx| {
                if view.claims {
                    claims.show(window, cx);
                } else {
                    claims.close(window, cx);
                }
            });
        }
        if !view.claims {
            self.leave_sidebar(&LeaveSidebar, window, cx);
        }
        let store = self.store.read(cx);
        let name = match task.and_then(|(id, _)| store.get(id)) {
            _ if view.claims => "claims".to_string(),
            Some(issue) if !matches!(view.scope, Scope::Inbox | Scope::Document(_)) => {
                issue.key.to_string()
            }
            _ => view.scope.name(&store.catalog).to_string(),
        };
        self.message = format!("Showing {name}").into();
        // The tab may have shrunk since, so note what is really open.
        self.record_view(true, cx);
        cx.notify();
    }

    fn focus_filter(&mut self, _: &FocusFilter, window: &mut Window, cx: &mut Context<Self>) {
        let filter = self.list.read(cx).filter.clone();
        filter.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    fn open_palette(&mut self, _: &OpenPalette, window: &mut Window, cx: &mut Context<Self>) {
        self.palette
            .update(cx, |palette, cx| palette.open(window, cx));
        cx.notify();
    }

    fn toggle_help(&mut self, _: &ToggleHelp, _: &mut Window, cx: &mut Context<Self>) {
        self.help_open = !self.help_open;
        cx.notify();
    }

    /// `g c` and `:claims`: the list of tasks agents hold.
    pub fn show_claims(&mut self, _: &ShowClaims, window: &mut Window, cx: &mut Context<Self>) {
        self.help_open = false;
        self.claims.update(cx, |claims, cx| claims.show(window, cx));
        cx.notify();
    }

    #[cfg(test)]
    pub fn task(&self) -> &Entity<TaskView> {
        &self.task
    }

    #[cfg(test)]
    pub fn claims(&self) -> &Entity<ClaimsList> {
        &self.claims
    }

    /// Selects `id` in the list and shows it in the task view, with focus
    /// on the list. A task the filter or the sidebar hides still opens in
    /// the task view. An open editor stays as it is.
    pub fn open_task(&mut self, id: IssueId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.store.read(cx).get(id).map(|i| i.key.clone()) else {
            return;
        };
        if let Some(row) = self.list.read(cx).row_of(id) {
            self.list_do(cx, |list, cx| list.select(row, cx));
        } else {
            self.message = format!("{key} is hidden by the filter or the sidebar").into();
        }
        if !self.editing(cx) {
            self.task.update(cx, |task, cx| task.show(id, cx));
            self.fetch_open_details(cx);
        }
        self.focus_list(window, cx);
        cx.notify();
    }

    // Descriptions.

    /// `Cmd-S` and `:w`: save the editor text and keep editing.
    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        if self.doc_view.read(cx).editing {
            self.save_document(false, cx);
        } else if !self.refuse_read_only_tab(cx) {
            self.save_description(false, cx);
        }
        cx.notify();
    }

    /// LLM-83: `e`, `E` and `:w` in a read-only tab, such as Activity, say
    /// so in the status bar instead of failing silently. Returns true when
    /// it refused. An open editor holds the whole description, so the tab
    /// does not matter then.
    fn refuse_read_only_tab(&mut self, cx: &mut Context<Self>) -> bool {
        if self.editing(cx) {
            return false;
        }
        let Some((ix, tabs)) = self.open_tab(cx) else {
            return false;
        };
        let Some(tab) = tabs.get(ix).filter(|t| t.read_only.is_some()) else {
            return false;
        };
        self.message = format!("{} is read only", tab.title).into();
        cx.notify();
        true
    }

    /// Sends the editor text to the server with the version the editor
    /// started from. `force` skips that check, for `:w!` after a refused
    /// save. In the conflict view, the save builds on the server's version
    /// shown beside the editor, so a merged text goes in without `force`.
    /// The preview shows the new text at once.
    fn save_description(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(detail) = self.detail(cx).filter(|d| d.mode == DetailMode::Editing) else {
            return;
        };
        let id = detail.issue;
        let Some(issue) = self.store.read(cx).get(id) else {
            return;
        };
        let (key, description, version) =
            (issue.key.clone(), issue.description.clone(), issue.version);
        let text = self.task.read(cx).editor.read(cx).value();
        if text == description && !force && !self.pipeline.has_conflict(&key) {
            self.message = format!("No changes in {key}").into();
            return;
        }
        let in_conflict = self.task.read(cx).conflict;
        let base = if in_conflict {
            version
        } else {
            self.pipeline.base_version(id, version)
        };
        if in_conflict {
            self.task
                .update(cx, |task, cx| task.set_conflict(false, cx));
        }
        self.save_text(id, text, base, force, cx);
    }

    /// Saves `text` as the description of `id`, built on version `base`.
    /// The store shows it at once. One save per task is on its way at a
    /// time; the next waits for the version the first one makes.
    fn save_text(
        &mut self,
        id: IssueId,
        text: SharedString,
        base: i64,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.store.read(cx).get(id).map(|i| i.key.clone()) else {
            return;
        };
        self.edit_issue(id, cx, |issue, _| issue.description = text.clone());
        self.message = if self.backend.is_some() {
            format!("Saving {key}")
        } else {
            format!("Saved {key}")
        }
        .into();
        // One save per task at a time. The next one waits for the version
        // the first one makes, or the server would refuse it as stale.
        if self.backend.is_some() && self.pipeline.is_saving(&key) {
            self.pipeline.queue(key, text.to_string(), force);
            return;
        }
        self.send_description(id, text.to_string(), base, force, cx);
    }

    /// Sends one description save and keeps its text until the server
    /// answers.
    fn send_description(
        &mut self,
        id: IssueId,
        text: String,
        base: i64,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.store.read(cx).get(id).map(|i| i.key.clone()) else {
            return;
        };
        self.pipeline.note_saving(key, text.clone());
        let update = llmtodo_client::types::UpdateIssue {
            description: llmtodo_client::types::BodyChange {
                body: Some(text),
                base_version: (!force).then_some(base),
                force,
                patches: Vec::new(),
            },
            ..Default::default()
        };
        self.send(
            id,
            Write::Update(Box::new(update)),
            WriteKind::Description,
            cx,
        );
    }

    pub fn edit_description(
        &mut self,
        _: &EditDescription,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.doc_shown(cx) {
            self.edit_document(window, cx);
            return;
        }
        if self.refuse_read_only_tab(cx) {
            return;
        }
        let Some(id) = self.target(cx) else {
            return;
        };
        if !self.editable(id, cx) || !self.has_text(id, cx) {
            return;
        }
        let Some(issue) = self.store.read(cx).get(id) else {
            return;
        };
        let (key, description, version) =
            (issue.key.clone(), issue.description.clone(), issue.version);
        // After a refused save, the editor opens with the owner's text, in
        // the conflict view beside the server's version.
        let draft = self.pipeline.conflict_text(&key).cloned();
        let conflict = draft.is_some();
        let text: SharedString = match draft {
            Some(draft) => {
                self.message = conflict_help(&key).into();
                draft.into()
            }
            None => description,
        };
        self.pipeline.begin_editing(id, version);
        self.task.update(cx, |task, cx| {
            task.start_editing(id, text, window, cx);
            task.set_conflict(conflict, cx);
        });
        cx.notify();
    }

    /// `Shift+E`: writes the description to a file, opens it in `$EDITOR`,
    /// and saves the text each time the editor writes the file.
    pub fn edit_external(&mut self, _: &EditExternal, _: &mut Window, cx: &mut Context<Self>) {
        if self.doc_shown(cx) {
            self.message =
                "$EDITOR works on issue descriptions. Press e to edit the document".into();
            cx.notify();
            return;
        }
        if self.refuse_read_only_tab(cx) {
            return;
        }
        let Some(id) = self.target(cx) else {
            return;
        };
        if !self.editable(id, cx) || !self.has_text(id, cx) {
            return;
        }
        let Some(issue) = self.store.read(cx).get(id) else {
            return;
        };
        let (key, text, version) = (
            issue.key.clone(),
            issue.description.to_string(),
            issue.version,
        );
        if self.pipeline.has_conflict(&key) {
            self.message = format!(
                "{key} has unsaved text from a refused save. Press e to compare both versions first"
            )
            .into();
            cx.notify();
            return;
        }
        let path = match self.external.open(key.clone(), id, text, version) {
            Ok(path) => path,
            Err(message) => {
                self.message = message.into();
                cx.notify();
                return;
            }
        };
        self.start_external_poll(cx);
        self.message = format!("Opening {key} in $EDITOR").into();
        let launcher = self.external.launcher();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { launcher(&path) }).await;
            this.update(cx, |this, cx| {
                this.message = match result {
                    Ok(()) => format!("{key} is open in $EDITOR. Each write saves it"),
                    Err(e) => format!("Could not open $EDITOR: {e}"),
                }
                .into();
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    #[cfg(test)]
    pub fn set_external(
        &mut self,
        dir: PathBuf,
        launcher: crate::model::external_editor::Launcher,
    ) {
        self.external = ExternalEdits::new(dir, launcher);
    }

    /// Polls the `$EDITOR` files until the last session ends. The next
    /// `Shift+E` starts it again.
    fn start_external_poll(&mut self, cx: &mut Context<Self>) {
        if self._external_poll.is_some() {
            return;
        }
        self._external_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(EXTERNAL_POLL).await;
                match this.update(cx, |this, cx| this.poll_external(cx)) {
                    Ok(true) => {}
                    _ => break,
                }
            }
        }));
    }

    #[cfg(test)]
    pub fn external_polling(&self) -> bool {
        self._external_poll.is_some()
    }

    /// Saves each `$EDITOR` file the editor wrote since the last check, and
    /// ends the edits whose editor exited, whose file is gone, or whose
    /// task went to the Trash. Draws only when something happened. Returns
    /// whether an edit is still open, so the poll goes on.
    pub fn poll_external(&mut self, cx: &mut Context<Self>) -> bool {
        let store = self.store.read(cx);
        let poll = self
            .external
            .poll(|id| store.get(id).is_some_and(|i| !i.deleted));
        for save in poll.saves.iter() {
            if !self.editable(save.id, cx) {
                continue;
            }
            self.save_text(save.id, save.text.clone().into(), save.base, false, cx);
            if self.backend.is_none() {
                self.message = format!("Saved {} from $EDITOR", save.key).into();
            }
        }
        if let Some(key) = poll.detached.first() {
            self.message = format!(
                "$EDITOR returned at once, so {key} stays open for saves. Set EDITOR to wait, such as code --wait"
            )
            .into();
        } else if let Some(key) = poll.ended.first() {
            self.message = format!("Closed {key} in $EDITOR").into();
        }
        if !poll.is_empty() {
            cx.notify();
        }
        let open = !self.external.is_empty();
        if !open {
            self._external_poll = None;
        }
        open
    }

    /// `:theirs`: drops the owner's text from a refused save and keeps the
    /// server's version. A file open in `$EDITOR` gets that version too, so
    /// its next write builds on it.
    fn keep_theirs(&mut self, window: &mut Window, cx: &mut Context<Self>) -> String {
        let Some(id) = self.target(cx) else {
            return "Open a task first".into();
        };
        if !self.has_text(id, cx) {
            return self.message.to_string();
        }
        let Some(issue) = self.store.read(cx).get(id) else {
            return String::new();
        };
        let (key, description, version) = (
            issue.key.clone(),
            issue.description.to_string(),
            issue.version,
        );
        let open_conflict =
            self.task.read(cx).conflict && self.detail(cx).is_some_and(|d| d.issue == id);
        if self.pipeline.drop_conflict(&key).is_none() && !open_conflict {
            return format!("{key} has no conflict to resolve");
        }
        if open_conflict {
            self.pipeline.end_editing();
            self.task.update(cx, |task, cx| {
                task.set_conflict(false, cx);
                task.set_mode(DetailMode::Preview, cx);
            });
            let focus = self.task.read(cx).focus.clone();
            window.focus(&focus, cx);
        }
        self.external.take_server_text(&key, description, version);
        format!("Kept the server's version of {key}")
    }

    /// Saves the editor text and shows the preview, inside the task view.
    pub fn show_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editing(cx) {
            return;
        }
        self.save_editor(cx);
        let focus = self.task.read(cx).focus.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        if self.detail(cx).is_none() {
            return;
        }
        self.save_description(false, cx);
        self.task
            .update(cx, |task, cx| task.set_mode(DetailMode::Preview, cx));
        self.pipeline.end_editing();
    }

    // Tabs.

    /// The open tab of the task view, clamped to the tabs it has now.
    pub fn open_tab(&self, cx: &App) -> Option<(usize, std::rc::Rc<Vec<sections::Tab>>)> {
        self.task.read(cx).open_tab(cx)
    }

    pub fn set_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.task.update(cx, |task, cx| task.set_tab(ix, cx));
        cx.notify();
        self.persist(cx);
    }

    fn next_tab(&mut self, _: &NextTab, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((ix, tabs)) = self.open_tab(cx)
            && !tabs.is_empty()
        {
            self.set_tab((ix + 1) % tabs.len(), cx);
        }
    }

    fn prev_tab(&mut self, _: &PrevTab, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((ix, tabs)) = self.open_tab(cx)
            && !tabs.is_empty()
        {
            self.set_tab((ix + tabs.len() - 1) % tabs.len(), cx);
        }
    }

    // App menu and macOS shortcuts; see app_menus.rs.

    /// Sets the UI zoom, says so in the status bar and saves it.
    pub fn set_zoom(&mut self, level: f32, cx: &mut Context<Self>) {
        crate::shell::theme::set_zoom(level, cx);
        let level = crate::shell::theme::zoom(cx);
        self.message = format!("Zoom {}%", (level * 100.).round()).into();
        self.persist(cx);
        cx.notify();
    }

    fn zoom_in(&mut self, _: &app_menus::ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(
            crate::shell::theme::zoom_step(crate::shell::theme::zoom(cx), true),
            cx,
        );
    }

    fn zoom_out(&mut self, _: &app_menus::ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(
            crate::shell::theme::zoom_step(crate::shell::theme::zoom(cx), false),
            cx,
        );
    }

    fn zoom_reset(&mut self, _: &app_menus::ZoomReset, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(1.0, cx);
    }

    /// Saves the settings, then closes the window. The app quits with it.
    fn close_window(
        &mut self,
        _: &app_menus::CloseWindow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.persist(cx);
        window.remove_window();
    }

    fn minimize(&mut self, _: &app_menus::Minimize, window: &mut Window, _: &mut Context<Self>) {
        window.minimize_window();
    }

    fn toggle_full_screen(
        &mut self,
        _: &app_menus::ToggleFullScreen,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.toggle_fullscreen();
    }

    fn open_settings(
        &mut self,
        _: &app_menus::OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_settings(window, cx);
    }

    // Writes to one issue. Each takes the issue's ID.

    /// False, with a message, when an agent holds the task. Every edit
    /// checks this, so a claimed task stays read-only.
    fn editable(&mut self, id: IssueId, cx: &mut Context<Self>) -> bool {
        if self.refuse_read_only_tracker(cx) {
            return false;
        }
        let Some(issue) = self.store.read(cx).get(id) else {
            return false;
        };
        let Some(claim) = &issue.claim else {
            return true;
        };
        self.message = format!(
            "{} is locked: {} is working on it. :release ends the claim",
            issue.key,
            claim.actor.label()
        )
        .into();
        cx.notify();
        false
    }

    /// `:release`: the owner ends an agent's claim. Claims never time out,
    /// so this is how a stale one goes away.
    pub fn release(&mut self, id: IssueId, cx: &mut Context<Self>) {
        if self.refuse_read_only_tracker(cx) {
            return;
        }
        let Some((key, claim)) = self.edit_issue(id, cx, |i, _| (i.key.clone(), i.claim.take()))
        else {
            return;
        };
        match claim {
            Some(claim) => {
                self.message = format!("Released {key} from {}", claim.actor.label()).into();
                self.send(id, Write::Release, WriteKind::Release, cx);
            }
            None => self.message = format!("{key} has no claim").into(),
        }
        cx.notify();
    }

    /// `verb` is `depends`, `blocks`, `undepend` or `unblock`, and `other` is
    /// the key of the other task. The server refuses a cycle, and the
    /// message then names it.
    pub fn change_dependency(
        &mut self,
        id: IssueId,
        verb: &str,
        other: &str,
        cx: &mut Context<Self>,
    ) {
        use llmtodo_client::types::{CreateRelation, RelationInput, RemoveRelation};
        let other = other.trim().to_uppercase();
        let (kind, add, done) = match verb {
            "depends" => (RelationInput::BlockedBy, true, "now depends on"),
            "blocks" => (RelationInput::Blocks, true, "now blocks"),
            "undepend" => (RelationInput::BlockedBy, false, "no longer depends on"),
            _ => (RelationInput::Blocks, false, "no longer blocks"),
        };
        cx.notify();
        if other.is_empty() {
            self.message = format!("Name a task, such as :{verb} LLM-9").into();
            return;
        }
        let store = self.store.read(cx);
        let Some(key) = store.get(id).map(|i| i.key.clone()) else {
            return;
        };
        if store.by_key(&other).is_none() {
            self.message = format!("Unknown task: {other}").into();
            return;
        }
        if key == other {
            self.message = format!("{key} cannot depend on itself").into();
            return;
        }
        // An order edge goes away with the order, not with :undepend.
        let direction = if verb == "undepend" {
            "blocked-by"
        } else {
            "blocks"
        };
        let only_order = !add && {
            let edges: Vec<_> = store
                .relations(id)
                .into_iter()
                .flatten()
                .filter(|r| {
                    r.kind == llmtodo_api_types::RelationType::Blocks
                        && r.direction == direction
                        && r.issue.key.to_string() == other
                })
                .collect();
            !edges.is_empty()
                && edges
                    .iter()
                    .all(|r| r.source == llmtodo_api_types::RelationSource::Order)
        };
        if only_order {
            self.message = format!(
                "{key} and {other} are in their parent's order. :step none takes {key} out"
            )
            .into();
            return;
        }
        if !self.editable(id, cx) {
            return;
        }
        let write = if add {
            Write::AddRelation(CreateRelation {
                kind,
                issue: other.clone(),
            })
        } else {
            Write::RemoveRelation(RemoveRelation {
                kind,
                issue: other.clone(),
            })
        };
        self.message = format!("{key} {done} {other}").into();
        self.send(id, write, WriteKind::Dependency, cx);
    }

    /// Opens the palette with `text` typed, such as `depends `.
    pub fn open_palette_with(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.palette
            .update(cx, |palette, cx| palette.open_with(text, window, cx));
        cx.notify();
    }

    /// Sets the status of one task. A review ticket set to Done opens the
    /// next ticket to review, as Approve does.
    pub fn set_status(&mut self, id: IssueId, status: Status, cx: &mut Context<Self>) {
        let open = self.panel_open(cx);
        if self.set_status_only(id, status, cx) {
            self.open_next_review(&[id], open, cx);
        }
    }

    /// Sets the status of one task. Returns whether the task left a review
    /// status for Done.
    pub(crate) fn set_status_only(
        &mut self,
        id: IssueId,
        status: Status,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.editable(id, cx) {
            return false;
        }
        let store = self.store.read(cx);
        let Some(issue) = store.get(id) else {
            return false;
        };
        let (key, team) = (issue.key.clone(), issue.team.clone());
        let was_review = issue.status.review;
        // A group header names a status by name; use this team's own.
        let Some(status) = store
            .catalog
            .find_status(&team, &status.name)
            .or_else(|| store.catalog.teams.is_empty().then_some(status.clone()))
        else {
            self.message = format!("Team {team} has no status {}", status.name).into();
            cx.notify();
            return false;
        };
        let approved = was_review && status.is_done();
        self.edit_issue(id, cx, |issue, _| issue.status = status.clone());
        self.message = format!("{key} is {}", status.name).into();
        let update = llmtodo_client::types::UpdateIssue {
            status: Some(status.name.to_string()),
            ..Default::default()
        };
        self.send(id, Write::Update(Box::new(update)), WriteKind::Status, cx);
        self.list_do(cx, |list, cx| list.refresh_keeping_selection(cx));
        approved
    }

    /// Gives the task the parent with key `parent`, or makes it a top-level
    /// task with `None`. A drop onto a parent group sends it. The task
    /// leaves the old parent's order.
    pub fn set_parent(
        &mut self,
        id: IssueId,
        parent: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        if !self.editable(id, cx) {
            return;
        }
        let store = self.store.read(cx);
        let Some(issue) = store.get(id) else {
            return;
        };
        if issue.parent == parent {
            return;
        }
        let key = issue.key.clone();
        if parent.as_ref() == Some(&key) {
            self.message = format!("{key} cannot be its own parent").into();
            cx.notify();
            return;
        }
        self.edit_issue(id, cx, |issue, _| {
            issue.parent = parent.clone();
            issue.order_step = None;
        });
        self.message = match &parent {
            Some(p) => format!("{key} is now under {p}"),
            None => format!("{key} has no parent now"),
        }
        .into();
        let update = llmtodo_client::types::UpdateIssue {
            parent: parent
                .as_ref()
                .map_or(llmtodo_client::types::Change::Clear, |p| {
                    llmtodo_client::types::Change::Set(p.to_string())
                }),
            ..Default::default()
        };
        self.send(id, Write::Update(Box::new(update)), WriteKind::Parent, cx);
        self.list_do(cx, |list, cx| list.refresh_keeping_selection(cx));
    }

    /// Sets who the task waits on, or clears it with `None`.
    pub fn set_waiting_on(
        &mut self,
        id: IssueId,
        waiting: Option<WaitingOn>,
        cx: &mut Context<Self>,
    ) {
        if !self.editable(id, cx) {
            return;
        }
        let Some(message) = self.edit_issue(id, cx, |issue, _| {
            issue.waiting_on = waiting;
            match waiting {
                Some(WaitingOn::Agent) => format!("{} waits on an agent", issue.key),
                Some(WaitingOn::Owner) => format!("{} waits on you", issue.key),
                None => format!("{} waits on no one", issue.key),
            }
        }) else {
            return;
        };
        self.message = message.into();
        let update = llmtodo_client::types::UpdateIssue {
            waiting_on: waiting.map_or(
                llmtodo_client::types::Change::Clear,
                llmtodo_client::types::Change::Set,
            ),
            ..Default::default()
        };
        self.send(
            id,
            Write::Update(Box::new(update)),
            WriteKind::WaitingOn,
            cx,
        );
    }

    pub fn toggle_label(&mut self, id: IssueId, label: Label, cx: &mut Context<Self>) {
        if !self.editable(id, cx) {
            return;
        }
        let Some((message, update)) = self.edit_issue(id, cx, |issue, catalog| {
            let mut update = llmtodo_client::types::UpdateIssue::default();
            let message = if let Some(pos) = issue.labels.iter().position(|l| *l == label) {
                issue.labels.remove(pos);
                update.remove_labels.push(label.name.to_string());
                format!("Removed {} from {}", label.name, issue.key)
            } else {
                issue.labels.push(label.clone());
                catalog.sort_labels(&mut issue.labels);
                update.add_labels.push(label.name.to_string());
                format!("Added {} to {}", label.name, issue.key)
            };
            (message, update)
        }) else {
            return;
        };
        self.message = message.into();
        self.send(id, Write::Update(Box::new(update)), WriteKind::Labels, cx);
        self.list_do(cx, |list, cx| list.refresh_keeping_selection(cx));
    }

    /// Sets or clears the assignee. `who` is `me`, or an actor name such as
    /// `claude-code/opus-5.5/high`. The server checks the name.
    fn assign(&mut self, id: IssueId, who: Option<String>, cx: &mut Context<Self>) {
        if self.refuse_read_only_tracker(cx) {
            return;
        }
        let wire = who.clone().unwrap_or_default();
        // The app's own guess until the server answers with the real actor.
        let known = who.as_ref().and_then(|name| {
            self.store
                .read(cx)
                .iter()
                .filter_map(|i| i.assignee.clone())
                .find(|a| a.name().eq_ignore_ascii_case(name))
        });
        let Some(message) = self.edit_issue(id, cx, |issue, _| match &who {
            Some(name) => {
                issue.assignee =
                    Some(known.unwrap_or_else(|| Assignee::Person(name.clone().into())));
                let shown = issue.assignee.as_ref().map(Assignee::label);
                format!(
                    "Assigned {} to {}",
                    issue.key,
                    shown.unwrap_or_else(|| OWNER_NAME.into())
                )
            }
            None => {
                issue.assignee = None;
                format!("Unassigned {}", issue.key)
            }
        }) else {
            return;
        };
        self.message = message.into();
        let update = llmtodo_client::types::UpdateIssue {
            assignee: llmtodo_client::types::Change::text(Some(wire)),
            ..Default::default()
        };
        self.send(id, Write::Update(Box::new(update)), WriteKind::Assignee, cx);
    }

    /// Copies one task's ID. Returns whether it copied.
    pub fn copy_key(&mut self, id: IssueId, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.store.read(cx).get(id).map(|i| i.key.clone()) else {
            return false;
        };
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(key.to_string()));
        self.message = format!("Copied {key}").into();
        cx.notify();
        true
    }

    /// LLM-190: copies the keys of several tasks, comma-separated, in the
    /// order of `ids`. Returns how many it copied, or None for none.
    pub fn copy_keys(&mut self, ids: &[IssueId], cx: &mut Context<Self>) -> Option<usize> {
        let store = self.store.read(cx);
        let keys: Vec<String> = ids
            .iter()
            .filter_map(|id| store.get(*id).map(|i| i.key.to_string()))
            .collect();
        if keys.is_empty() {
            return None;
        }
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(keys.join(",")));
        self.message = format!("Copied {} IDs", keys.len()).into();
        cx.notify();
        Some(keys.len())
    }

    /// Moves an issue to the Trash. Nothing is lost: `:restore` brings back
    /// the last one, so no confirmation is needed.
    pub fn delete_issue(&mut self, id: IssueId, cx: &mut Context<Self>) {
        if !self.editable(id, cx) {
            return;
        }
        if let Some(issue) = self.store.read(cx).get(id).filter(|i| i.deleted) {
            self.message = format!("{} is already in the Trash", issue.key).into();
            cx.notify();
            return;
        }
        let Some(key) = self.edit_issue(id, cx, |issue, _| {
            issue.deleted = true;
            issue.key.clone()
        }) else {
            return;
        };
        self.last_deleted.push((id, key.clone()));
        self.send(id, Write::Trash, WriteKind::Trash, cx);
        if self.detail(cx).is_some_and(|d| d.issue == id) {
            self.task.update(cx, |task, cx| task.close(cx));
        }
        self.message = format!("Moved {key} to Trash. :restore undoes it").into();
        // The task that moves up into this row takes the selection.
        let row = self.selected(cx);
        self.list_do(cx, |list, cx| {
            list.refilter(cx);
            list.select(row, cx);
        });
        // The selection moved because the row went away, not because the
        // owner went somewhere.
        self.record_view(true, cx);
    }

    /// `:restore` on a task in the Trash, such as the one selected in the
    /// Trash view, brings it back. The task below takes the selection.
    pub fn restore_trashed(&mut self, id: IssueId, cx: &mut Context<Self>) {
        if self.refuse_read_only_tracker(cx) {
            return;
        }
        let Some(key) = self.edit_issue(id, cx, |issue, _| {
            issue.deleted = false;
            issue.key.clone()
        }) else {
            return;
        };
        self.last_deleted.retain(|(d, _)| *d != id);
        self.send(id, Write::Restore, WriteKind::Restore, cx);
        self.message = format!("Restored {key}").into();
        let row = self.selected(cx);
        self.list_do(cx, |list, cx| {
            list.refilter(cx);
            list.select(row, cx);
        });
        // The selection moved because the row went away, not because the
        // owner went somewhere.
        self.record_view(true, cx);
    }

    /// `:restore` brings back the task it acts on when that one is in the
    /// Trash. Otherwise it brings back the last task this run moved there.
    /// The store may have lost that copy, so the restore goes out by key,
    /// and the server's answer adds the task back.
    fn restore_issue(&mut self, cx: &mut Context<Self>) {
        if self.refuse_read_only_tracker(cx) {
            return;
        }
        if let Some(id) = self
            .target(cx)
            .filter(|id| self.store.read(cx).get(*id).is_some_and(|i| i.deleted))
        {
            self.restore_trashed(id, cx);
            return;
        }
        let Some((id, key)) = self.last_deleted.pop() else {
            self.message = "Nothing to restore".into();
            cx.notify();
            return;
        };
        // The store's copy, when it still has one, carries the current key.
        let held = self.edit_issue(id, cx, |issue, _| {
            issue.deleted = false;
            issue.key.clone()
        });
        let key = held.clone().unwrap_or(key);
        self.send_to(id, key.clone(), Write::Restore, WriteKind::Restore);
        self.message = if held.is_some() || self.backend.is_none() {
            format!("Restored {key}")
        } else {
            format!("Restoring {key}")
        }
        .into();
        self.list_do(cx, |list, cx| list.reload(Some(id), cx));
    }
}
