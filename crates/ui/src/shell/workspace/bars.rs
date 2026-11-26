//! LLM-348: the title bar and the status bar, each as its own cached view.
//!
//! A cat frame or a cursor blink draws the workspace again. The bars used
//! to be built inside `Workspace::render`, so each such frame built them
//! too. Now the workspace draws them with `.cached()`, and each bar draws
//! again only when what it shows changes.
//!
//! Each bar keeps what it last drew, as a `TitleShown` or `StatusShown`.
//! It works out what it shows now whenever the workspace, the store, the
//! list or the task view notifies, and draws again only when that differs.
//! A focus change refreshes the whole window, so the vim mode needs no
//! observer. The workspace also compares on each of its own renders, so a
//! change that notified none of those still reaches the bar one frame on.

use super::*;
use crate::model::data::ProjectStyle;
use gpui_kit::WeakEntity;

/// The open tracker's badge, when there is a choice of trackers.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TrackerBadge {
    label: String,
    tip: &'static str,
    real: bool,
}

/// Everything the title bar shows.
#[derive(Clone, PartialEq)]
pub(super) struct TitleShown {
    update: Option<Entity<UpdateButton>>,
    name: SharedString,
    place: TitlePlace,
    icon: Option<ProjectStyle>,
    tracker: Option<TrackerBadge>,
    sidebar_open: bool,
    /// LLM-592: the ticket key of a preview build.
    preview: Option<SharedString>,
}

/// The title bar's colors. A preview build paints the bar yellow, with
/// text that keeps 4.5:1 on it (LLM-592).
#[derive(Clone, Copy)]
struct BarColors {
    fill: Option<gpui_kit::Hsla>,
    text: gpui_kit::Hsla,
    muted: gpui_kit::Hsla,
    border: gpui_kit::Hsla,
}

impl BarColors {
    fn new(preview: bool, cx: &App) -> Self {
        let theme = cx.theme();
        if preview {
            let (fill, text) = crate::shell::theme::preview_bar(theme);
            Self {
                fill: Some(fill),
                text,
                muted: text,
                border: text.opacity(0.35),
            }
        } else {
            Self {
                fill: None,
                text: theme.foreground,
                muted: theme.muted_foreground,
                border: theme.border,
            }
        }
    }
}

/// Everything the status bar shows.
#[derive(Clone, PartialEq)]
pub(super) struct StatusShown {
    mode: &'static str,
    count: String,
    /// The answer the Undo button takes back.
    offer: Option<SharedString>,
    message: Option<SharedString>,
    connection: Connection,
    /// Offline mode on a shared database (LLM-471).
    sync: llmtodo_api_types::SyncState,
    /// A wait that lasted past its show-after delay (LLM-470).
    wait: Option<crate::model::activity::Wait>,
    claimed: usize,
}

impl Workspace {
    pub(super) fn title_shown(&self, cx: &App) -> TitleShown {
        let (name, place) = self.title_place(cx);
        let tracker = self.store.read(cx).open_tracker().map(|t| {
            // The real one says so plainly, so the owner never edits real
            // data by mistake.
            let (label, tip) = if t.read_only {
                (
                    format!("{}: read-only", t.name),
                    "This preview can read your real tracker but cannot change it",
                )
            } else if t.real {
                (
                    format!("{}: real data", t.name),
                    "Changes here change your real tracker",
                )
            } else {
                (t.name.to_string(), "Changes here stay in this tracker")
            };
            TrackerBadge {
                label,
                tip,
                real: t.real,
            }
        });
        TitleShown {
            update: self.update_button(),
            name,
            place,
            icon: self.title_project_style(cx),
            tracker,
            sidebar_open: self.sidebar_shown(),
            preview: self.preview.clone(),
        }
    }

    /// The ticket key when this app is a preview build of it (LLM-592).
    pub fn preview_ticket(&self) -> Option<&SharedString> {
        self.preview.as_ref()
    }

    /// Marks this app as a preview build of `key`, or as no preview.
    pub fn set_preview_ticket(&mut self, key: Option<SharedString>, cx: &mut Context<Self>) {
        self.preview = key;
        cx.notify();
    }

    /// The style of the scope's project, for its icon before its name in
    /// the title bar. None for other scopes and for development runs.
    fn title_project_style(&self, cx: &App) -> Option<ProjectStyle> {
        if self.project_dir.is_some() {
            return None;
        }
        let catalog = &self.store.read(cx).catalog;
        let slug = super::scope_project(&self.list.read(cx).scope, catalog)?;
        catalog.projects.iter().find(|p| p.key == slug)?;
        Some(catalog.project_style(&slug))
    }

    pub(super) fn status_shown(&self, window: &Window, cx: &App) -> StatusShown {
        let task = self.task.read(cx);
        let mode = if task.editing() && task.editor_focused(window, cx) {
            task.editor_vim.name()
        } else {
            self.vim_mode(window, cx)
        };
        let list = self.list.read(cx);
        let count = list.visible().len();
        let mut count = match &list.scope {
            Scope::All => format!("{count} issues"),
            Scope::Settings => String::new(),
            scope => format!(
                "{count} issues in {}",
                scope.name(&self.store.read(cx).catalog)
            ),
        };
        let hidden = list.hidden_closed();
        if hidden > 0 {
            count.push_str(&format!(", {hidden} old closed hidden"));
        }
        let marked = list.marked.len();
        if marked > 0 {
            count.push_str(&format!(", {marked} marked"));
        }
        let store = self.store.read(cx);
        // The offer repeats the answer message, so that message shows once.
        let offer = self.answer_undo.as_ref().map(|u| u.message.clone());
        let message = (offer.as_ref() != Some(&self.message) && !self.message.is_empty())
            .then(|| self.message.clone());
        StatusShown {
            mode,
            count,
            offer,
            message,
            connection: store.connection.clone(),
            sync: store.sync,
            wait: self.activity.shown(cx.background_executor().now()),
            claimed: store.claim_count(),
        }
    }

    /// The wait the status bar shows now, for tests.
    #[cfg(test)]
    pub(crate) fn wait_shown(&self, cx: &App) -> Option<crate::model::activity::Wait> {
        self.activity.shown(cx.background_executor().now())
    }

    /// The status bar's height, which its cached view must be given. It is
    /// what the bar's content measures: a line of `text_xs`, or the Undo
    /// button when it is taller, with the padding and the top border.
    pub(super) fn status_bar_height(&self, window: &Window, cx: &App) -> gpui_kit::Pixels {
        let rem = window.rem_size();
        let mut style = window.text_style();
        style.font_size = gpui_kit::rems(0.75).into();
        let mut line = style.line_height_in_pixels(rem);
        if self.status_bar.read(cx).offers_undo() {
            line = line.max(rem * 1.25);
        }
        gpui_kit::px(1.) + rem * 0.5 + line
    }

    /// Lets each bar catch a change that notified nothing it observes. The
    /// check runs after this frame, so a bar drawn from its cache now draws
    /// again on the next one.
    pub(super) fn sync_bars(&self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title_bar.clone();
        if title
            .read(cx)
            .shown
            .as_ref()
            .is_some_and(|shown| *shown != self.title_shown(cx))
        {
            window.defer(cx, move |window, cx| {
                title.update(cx, |bar, cx| bar.refresh(window, cx))
            });
        }
        let status = self.status_bar.clone();
        if status
            .read(cx)
            .shown
            .as_ref()
            .is_some_and(|shown| *shown != self.status_shown(window, cx))
        {
            window.defer(cx, move |window, cx| {
                status.update(cx, |bar, cx| bar.refresh(window, cx))
            });
        }
    }

    /// A click on the folder in the title bar copies it.
    fn copy_project_path(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(path.to_string()));
        self.message = format!("Copied {path}").into();
        self.flash("Copied", window, cx);
    }
}

/// The entities each bar reads, so it can watch them.
pub(super) struct Watched<'a> {
    pub workspace: &'a Entity<Workspace>,
    pub store: &'a Entity<IssueStore>,
    pub list: &'a Entity<IssueList>,
    pub task: &'a Entity<TaskView>,
}

impl Watched<'_> {
    /// Calls `refresh` whenever one of them notifies.
    fn observe<V: 'static>(
        &self,
        window: &mut Window,
        cx: &mut Context<V>,
        refresh: fn(&mut V, &mut Window, &mut Context<V>),
    ) -> Vec<Subscription> {
        vec![
            cx.observe_in(self.workspace, window, move |this, _, window, cx| {
                refresh(this, window, cx)
            }),
            cx.observe_in(self.store, window, move |this, _, window, cx| {
                refresh(this, window, cx)
            }),
            cx.observe_in(self.list, window, move |this, _, window, cx| {
                refresh(this, window, cx)
            }),
            cx.observe_in(self.task, window, move |this, _, window, cx| {
                refresh(this, window, cx)
            }),
        ]
    }
}

/// The title bar: the project of the scope on the left, its repo or folder
/// on the right. A click on the repo opens it; a click on the folder
/// copies it. Restart to update sits left of them.
pub struct TitleBarView {
    workspace: WeakEntity<Workspace>,
    /// What the bar showed when it last drew. None before the first draw.
    shown: Option<TitleShown>,
    _subscriptions: Vec<Subscription>,
}

impl TitleBarView {
    pub(super) fn new(watched: Watched, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            workspace: watched.workspace.downgrade(),
            shown: None,
            _subscriptions: watched.observe(window, cx, Self::refresh),
        }
    }

    /// Draws again when what the bar shows changed.
    fn refresh(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let now = workspace.read(cx).title_shown(cx);
        if self.shown.as_ref() != Some(&now) {
            self.shown = Some(now);
            cx.notify();
        }
    }

    /// Runs `f` on the workspace, from a click in the bar.
    fn on_workspace(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>),
    ) {
        self.workspace
            .update(cx, |workspace, cx| f(workspace, window, cx))
            .ok();
    }

    /// The repo as a link, or the folder as a copy button. Neither for a
    /// scope that is not one project.
    fn render_place(
        &self,
        place: TitlePlace,
        colors: BarColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = cx.theme();
        let hover = colors.text.opacity(0.08);
        let pressed = colors.text.opacity(0.16);
        let mono = theme.mono_font_family.clone();
        let muted = colors.muted;
        let radius = theme.radius;
        match place {
            TitlePlace::None => None,
            TitlePlace::Repo { label, url } => {
                let tip = if url.starts_with("https://github.com/") {
                    "Open on GitHub"
                } else {
                    "Open the repo"
                };
                Some(
                    div()
                        .id("project-repo")
                        .test_support()
                        .px_1()
                        .rounded(radius)
                        .hover(move |s| s.bg(hover))
                        .active(move |s| s.bg(pressed))
                        .tooltip(crate::shell::tooltip::Card::text(tip).builder())
                        .child(
                            crate::shell::widgets::link("project-repo-link", url, cx)
                                .font_family(mono)
                                .text_xs()
                                .child(crate::shell::widgets::card_text(label, cx)),
                        )
                        .into_any_element(),
                )
            }
            TitlePlace::Folder(path) => Some(
                div()
                    .id("project-path")
                    .test_support()
                    .px_1()
                    .rounded(radius)
                    .cursor_pointer()
                    .font_family(mono)
                    .text_xs()
                    .text_color(muted)
                    .hover(move |s| s.bg(hover))
                    .active(move |s| s.bg(pressed))
                    .tooltip(crate::shell::tooltip::Card::text("Copy path").builder())
                    .child(crate::shell::widgets::card_text(path.clone(), cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let path = path.clone();
                        this.on_workspace(window, cx, move |workspace, window, cx| {
                            workspace.copy_project_path(&path, window, cx)
                        });
                    }))
                    .into_any_element(),
            ),
        }
    }
}

impl Render for TitleBarView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::render_count::bump::<Self>();
        let Some(workspace) = self.workspace.upgrade() else {
            return div().into_any_element();
        };
        let shown = workspace.read(cx).title_shown(cx);
        self.shown = Some(shown.clone());
        let TitleShown {
            update,
            name,
            place,
            icon,
            tracker,
            sidebar_open,
            preview,
        } = shown;
        let colors = BarColors::new(preview.is_some(), cx);
        let place = self.render_place(place, colors, cx);
        let theme = cx.theme();
        let project_icon = icon.map(|style| {
            div()
                .id("title-project-icon")
                .test_support()
                .flex_none()
                .child(crate::shell::icons::project_icon(&style, colors.muted, cx).small())
        });
        let preview_label = preview.map(|key| {
            div()
                .id("title-preview")
                .test_support()
                .flex_none()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(colors.text)
                .child(crate::model::preview::title(&key))
        });
        let tracker = tracker.map(|t| {
            h_flex()
                .id("open-tracker")
                .test_support()
                .flex_none()
                .gap_1()
                .px_1p5()
                .rounded(theme.radius)
                .border_1()
                .border_color(if t.real { colors.text } else { colors.border })
                .text_xs()
                .text_color(if t.real { colors.text } else { colors.muted })
                .when(t.real, |this| {
                    this.font_weight(gpui_kit::FontWeight::SEMIBOLD)
                })
                .tooltip(crate::shell::tooltip::Card::text(t.tip).builder())
                .child(Icon::new(IconName::Database).xsmall())
                .child(crate::shell::widgets::card_text(t.label, cx))
        });
        // Owner rule: the title bar is the plain page color, with no fill,
        // and always has a border under it. Zoom never changes its size
        // (LLM-212). A preview build is the one exception: it fills the bar
        // with yellow, so it never passes for the real app.
        let fill = colors.fill.unwrap_or_else(|| {
            crate::shell::backdrop::surface(
                theme.background,
                &[crate::shell::backdrop::Slot::Window],
                cx,
            )
        });
        crate::shell::unzoomed::unzoomed(
            TitleBar::new()
                .bg(fill)
                .border_b_1()
                .border_color(colors.border)
                .child(
                    h_flex()
                        .w_full()
                        .pr_2()
                        .justify_between()
                        .text_sm()
                        .child(
                            h_flex()
                                .gap_1()
                                .child(crate::shell::tooltip::with_card(
                                    "toggle-sidebar-tip",
                                    Button::new("toggle-sidebar")
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::PanelLeft)
                                        .selected(sidebar_open)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.on_workspace(
                                                window,
                                                cx,
                                                |workspace, window, cx| {
                                                    workspace.toggle_sidebar(
                                                        &ToggleSidebar,
                                                        window,
                                                        cx,
                                                    )
                                                },
                                            );
                                        })),
                                    crate::shell::tooltip::Card::text(if sidebar_open {
                                        "Hide sidebar (Cmd-B)"
                                    } else {
                                        "Show sidebar (Cmd-B)"
                                    }),
                                ))
                                .children(project_icon)
                                .child(
                                    div()
                                        .id("project-name")
                                        .test_support()
                                        .text_color(colors.text)
                                        .child(name),
                                )
                                .children(tracker),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .children(preview_label)
                                .children(update)
                                .children(place),
                        ),
                ),
        )
        .into_any_element()
    }
}

/// The status bar: the vim mode, the count, the last message and the
/// answer's Undo on the left; the connection and the claims on the right.
pub struct StatusBarView {
    workspace: WeakEntity<Workspace>,
    /// What the bar showed when it last drew. None before the first draw.
    shown: Option<StatusShown>,
    _subscriptions: Vec<Subscription>,
}

impl StatusBarView {
    pub(super) fn new(watched: Watched, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            workspace: watched.workspace.downgrade(),
            shown: None,
            _subscriptions: watched.observe(window, cx, Self::refresh),
        }
    }

    /// Draws again when what the bar shows changed.
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let now = workspace.read(cx).status_shown(window, cx);
        if self.shown.as_ref() != Some(&now) {
            self.shown = Some(now);
            cx.notify();
        }
    }

    /// Whether the bar shows the answer's Undo button, which is taller
    /// than a line of text.
    fn offers_undo(&self) -> bool {
        self.shown.as_ref().is_some_and(|s| s.offer.is_some())
    }

    fn on_workspace(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>),
    ) {
        self.workspace
            .update(cx, |workspace, cx| f(workspace, window, cx))
            .ok();
    }
}

impl Render for StatusBarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::render_count::bump::<Self>();
        let Some(workspace) = self.workspace.upgrade() else {
            return div().into_any_element();
        };
        let shown = workspace.read(cx).status_shown(window, cx);
        self.shown = Some(shown.clone());
        let StatusShown {
            mode,
            count,
            offer,
            message,
            connection,
            sync,
            wait,
            claimed,
        } = shown;
        let theme = cx.theme();
        let hover = theme.foreground.opacity(0.08);
        let pressed = theme.foreground.opacity(0.16);
        let radius = theme.radius;
        let undo = crate::shell::widgets::button("answer-undo-button", false)
            .xsmall()
            .label("Undo")
            .tooltip("Undo this answer (u in the inbox)")
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_workspace(window, cx, |workspace, _, cx| workspace.undo_answer(cx))
            }));
        let bar = StatusBar::new()
            .h_full()
            .left(
                h_flex()
                    .gap_3()
                    .text_xs()
                    .child(
                        div()
                            .font_family(theme.mono_font_family.clone())
                            .child(mode.to_uppercase()),
                    )
                    .map(|this| {
                        if count.is_empty() {
                            return this;
                        }
                        // The count names the project: a click opens the
                        // palette on `project `, the UI for `:project X`.
                        this.child(
                            div()
                                .id("status-count")
                                .test_support()
                                .px_1()
                                .rounded(radius)
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover))
                                .active(move |s| s.bg(pressed))
                                .child(count)
                                .tooltip(
                                    crate::shell::tooltip::Card::text("Switch project (:project)")
                                        .builder(),
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.on_workspace(window, cx, |workspace, window, cx| {
                                        workspace.open_palette_with("project ", window, cx)
                                    })
                                })),
                        )
                    })
                    .when_some(offer, |this, offer| {
                        this.child(
                            h_flex()
                                .id("answer-undo")
                                .test_support()
                                .gap_2()
                                .child(div().max_w(gpui_kit::px(560.)).truncate().child(offer))
                                .child(undo),
                        )
                    })
                    .when_some(message, |this, message| {
                        // The message keeps to one line. Long text ends in
                        // an ellipsis, and a hover shows all of it.
                        let full = message.clone();
                        this.child(
                            div()
                                .id("status-message")
                                .test_support()
                                .min_w_0()
                                .max_w(gpui_kit::px(560.))
                                .truncate()
                                .child(super::render::one_line(&message))
                                .tooltip(crate::shell::tooltip::Card::text(full).builder()),
                        )
                    }),
            )
            .right(
                h_flex()
                    .gap_3()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    // A wait shows beside the connection, quietly. Waking
                    // stands in for Connecting, since it says more.
                    .when_some(wait, |this, wait| {
                        use crate::model::activity::{Wait, loading_text};
                        let text = match wait {
                            Wait::Waking => "Waking the shared database…".to_string(),
                            Wait::Loading { loaded, total } => loading_text(loaded, total),
                            Wait::Saving => "Saving…".to_string(),
                        };
                        this.child(
                            h_flex()
                                .id("status-wait")
                                .test_support()
                                .gap_1()
                                .child(
                                    gpui_kit::component::spinner::Spinner::new()
                                        .xsmall()
                                        .color(theme.muted_foreground),
                                )
                                .child(text),
                        )
                    })
                    .map(|this| match &connection {
                        Connection::Local => this,
                        Connection::Connecting
                            if wait == Some(crate::model::activity::Wait::Waking) =>
                        {
                            this
                        }
                        Connection::Connecting => this.child("Connecting…"),
                        // Offline on a shared database, the server works on
                        // its local copy. Like Offline below, the owner must
                        // notice it, so it uses the foreground (LLM-471).
                        Connection::Online(_) if sync.syncing => this.child(
                            h_flex()
                                .id("status-sync")
                                .test_support()
                                .gap_1()
                                .child(
                                    gpui_kit::component::spinner::Spinner::new()
                                        .xsmall()
                                        .color(theme.muted_foreground),
                                )
                                .children(crate::model::activity::sync_text(sync)),
                        ),
                        Connection::Online(_) if sync.offline => this.child(
                            h_flex()
                                .id("status-sync")
                                .test_support()
                                .gap_1()
                                .text_color(theme.foreground)
                                .child(Icon::new(IconName::Unplug).xsmall())
                                .children(crate::model::activity::sync_text(sync))
                                .tooltip(
                                    crate::shell::tooltip::Card::text(
                                        "The shared database is out of reach. Changes wait here and sync when the network is back",
                                    )
                                    .builder(),
                                ),
                        ),
                        Connection::Online(url) => this.child(
                            h_flex()
                                .gap_1()
                                .child(Icon::new(IconName::Radio).xsmall())
                                .child(format!("Live · {url}")),
                        ),
                        // The one state the owner must notice, so it uses
                        // the foreground, not muted text.
                        Connection::Offline(_) => this.child(
                            h_flex()
                                .gap_1()
                                .text_color(theme.foreground)
                                .child(Icon::new(IconName::Unplug).xsmall())
                                .child("Offline, retrying"),
                        ),
                        // Distinct from Offline: the server runs, and the
                        // owner checks the network or the database URL.
                        Connection::DatabaseDown { host, .. } => this.child(
                            h_flex()
                                .id("status-database-down")
                                .test_support()
                                .gap_1()
                                .text_color(theme.foreground)
                                .child(Icon::new(IconName::DatabaseZap).xsmall())
                                .child("Shared database unreachable, retrying")
                                .when_some(host.clone(), |this, host| {
                                    this.tooltip(
                                        crate::shell::tooltip::Card::text(format!(
                                            "The server cannot reach {host}"
                                        ))
                                        .builder(),
                                    )
                                }),
                        ),
                    })
                    .when_some(crate::model::activity::refused_text(sync), |this, text| {
                        this.child(
                            div()
                                .id("status-refused")
                                .test_support()
                                .text_color(theme.foreground)
                                .child(text)
                                .tooltip(
                                    crate::shell::tooltip::Card::text(
                                        "The shared database refused these offline changes. Run llmtodo db outbox to see them",
                                    )
                                    .builder(),
                                ),
                        )
                    })
                    .when(claimed > 0, |this| {
                        this.child(
                            h_flex()
                                .id("status-claims")
                                .test_support()
                                .gap_1()
                                .px_1()
                                .rounded(radius)
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover))
                                .active(move |s| s.bg(pressed))
                                .child(Icon::new(IconName::Lock).xsmall())
                                .child(format!("{claimed} claimed"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.on_workspace(window, cx, |workspace, window, cx| {
                                        workspace.show_claims(&ShowClaims, window, cx)
                                    })
                                })),
                        )
                    })
                    .child("? keys"),
            );
        div()
            .id("status-bar")
            .test_support()
            .size_full()
            .child(bar)
            .into_any_element()
    }
}
