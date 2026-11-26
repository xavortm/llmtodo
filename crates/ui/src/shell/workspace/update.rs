//! Restart to update: the title bar button and what a click does.
//! `model/update.rs` holds the parts with no GPUI.

use std::time::Duration;

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::ButtonCustomVariant;
use gpui_kit::{EventEmitter, Hsla, hsla, px};

use super::*;
use crate::model::update::{Restart, UpdateWatch};

/// How often the app reads the installed bundle's commit. A publish is
/// rare, so a slow check costs nothing the owner would notice.
pub(crate) const CHECK_EVERY: Duration = Duration::from_secs(30);
/// How long a restart waits for saves on their way, in steps of 100 ms.
/// Saves still out after that stay as kept text.
const SAVE_WAIT_STEPS: u32 = 50;
/// The button is solid gold in both themes: the one exception to the
/// monochrome rule. The text stays dark on it.
fn gold(lightness: f32) -> Hsla {
    hsla(45. / 360., 0.92, lightness, 1.)
}

/// The button's fill at rest: solid gold, so it stands out in the title bar.
pub(crate) const UPDATE_FILL: Hsla = Hsla {
    h: 45. / 360.,
    s: 0.92,
    l: 0.60,
    a: 1.,
};

/// The button's text: near black on the gold.
pub(crate) const UPDATE_TEXT: Hsla = Hsla {
    h: 0.,
    s: 0.,
    l: 0.1,
    a: 1.,
};

pub struct UpdateState {
    watch: UpdateWatch,
    /// The data folder whose server the restart stops.
    data_dir: PathBuf,
    /// The CLI to stop the server with, when the bundle has none.
    cli: Option<PathBuf>,
    /// The installed commit, when it differs from this build.
    ready: Option<String>,
    /// The owner closed the button with its X. It stays hidden until the
    /// next start, and Check for Updates in the app menu still works.
    dismissed: bool,
    restarting: bool,
    /// Starts the helper. Tests record the restart instead.
    launch: fn(&Restart) -> std::io::Result<()>,
    /// The title bar button, as its own view, so a change to it draws
    /// only the button and the workspace, not the cached views.
    button: Entity<UpdateButton>,
    _poll: gpui_kit::Task<()>,
    _click: gpui_kit::Subscription,
    _dismiss: gpui_kit::Subscription,
}

/// The Restart to update button, with an X on its right that hides it.
/// The workspace owns its state and copies it here with `set`. A click
/// emits `RestartClicked`, and the X emits `UpdateDismissed`.
pub struct UpdateButton {
    commit: Option<String>,
    restarting: bool,
}

pub struct RestartClicked;

impl EventEmitter<RestartClicked> for UpdateButton {}

pub struct UpdateDismissed;

impl EventEmitter<UpdateDismissed> for UpdateButton {}

impl UpdateButton {
    fn set(&mut self, commit: Option<String>, restarting: bool, cx: &mut Context<Self>) {
        if self.commit != commit || self.restarting != restarting {
            self.commit = commit;
            self.restarting = restarting;
            cx.notify();
        }
    }
}

fn spawn_helper(restart: &Restart) -> std::io::Result<()> {
    restart.command().spawn().map(|_| ())
}

impl Workspace {
    /// Starts watching the installed bundle. `main` calls this only for the
    /// owner's real app, so a preview or a test never shows the button.
    pub fn watch_for_updates(
        &mut self,
        watch: UpdateWatch,
        data_dir: PathBuf,
        cli: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        // The file read runs off the main thread. Once a new build is
        // found, the watch ends: the button stays until the restart.
        let poll = cx.spawn({
            let watch = watch.clone();
            async move |this, cx| {
                loop {
                    let newer = cx.background_spawn({
                        let watch = watch.clone();
                        async move { watch.newer() }
                    });
                    let newer = newer.await;
                    let found = newer.is_some();
                    let alive = this.update(cx, |this, cx| this.found_update(newer, cx));
                    if alive.is_err() || found {
                        break;
                    }
                    cx.background_executor().timer(CHECK_EVERY).await;
                }
            }
        });
        let button = cx.new(|_| UpdateButton {
            commit: None,
            restarting: false,
        });
        let click = cx.subscribe(&button, |this, _, _: &RestartClicked, cx| {
            this.restart_to_update(cx)
        });
        let dismiss = cx.subscribe(&button, |this, _, _: &UpdateDismissed, cx| {
            this.dismiss_update(cx)
        });
        self.update = Some(UpdateState {
            watch,
            data_dir,
            cli,
            ready: None,
            dismissed: false,
            restarting: false,
            launch: spawn_helper,
            button,
            _poll: poll,
            _click: click,
            _dismiss: dismiss,
        });
    }

    fn found_update(&mut self, newer: Option<String>, cx: &mut Context<Self>) {
        let Some(state) = &mut self.update else {
            return;
        };
        if newer != state.ready {
            state.ready = newer;
            self.sync_update_button(cx);
            // The button comes or goes in the title bar.
            cx.notify();
        }
    }

    /// Copies the update state into the button view.
    fn sync_update_button(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &self.update else {
            return;
        };
        let (commit, restarting) = (state.ready.clone(), state.restarting);
        state
            .button
            .update(cx, |button, cx| button.set(commit, restarting, cx));
    }

    /// Hides the button until the next start.
    pub fn dismiss_update(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.update else {
            return;
        };
        if !state.dismissed {
            state.dismissed = true;
            cx.notify();
        }
    }

    /// Check for Updates in the app menu. It reads the installed bundle
    /// now, and restarts onto it when it is a new build.
    pub fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.update else {
            self.message = "Updates work only in the installed app".into();
            cx.notify();
            return;
        };
        // One small file read, so it runs here and the menu answers at once.
        let newer = state.watch.newer().or_else(|| state.ready.clone());
        let Some(newer) = newer else {
            self.message = format!("{} is up to date", app_menus::APP_NAME).into();
            cx.notify();
            return;
        };
        state.dismissed = false;
        self.found_update(Some(newer), cx);
        self.restart_to_update(cx);
    }

    /// Whether the button shows in the title bar.
    #[cfg(test)]
    pub fn update_button_shown(&self) -> bool {
        self.update_button().is_some()
    }

    /// Whether the update watch still reads the bundle.
    #[cfg(test)]
    pub fn update_watching(&self) -> bool {
        self.update.as_ref().is_some_and(|s| !s._poll.is_ready())
    }

    #[cfg(test)]
    pub fn update_restarting(&self) -> bool {
        self.update.as_ref().is_some_and(|s| s.restarting)
    }

    #[cfg(test)]
    pub fn set_update_launcher(&mut self, launch: fn(&Restart) -> std::io::Result<()>) {
        if let Some(state) = &mut self.update {
            state.launch = launch;
        }
    }

    /// The installed commit when a restart would change the build.
    #[cfg(test)]
    pub fn update_ready(&self) -> Option<&str> {
        self.update.as_ref()?.ready.as_deref()
    }

    /// Saves what it can, keeps the rest as drafts, then hands over to the
    /// helper that stops the server and opens the installed bundle.
    pub fn restart_to_update(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &self.update else {
            return;
        };
        if state.restarting || state.ready.is_none() {
            return;
        }
        if self.doc_view.read(cx).editing {
            self.message = "Save or leave the document editor first".into();
            cx.notify();
            return;
        }
        if !self.external.is_empty() {
            self.message = "Close the descriptions open in $EDITOR first".into();
            cx.notify();
            return;
        }
        self.keep_editor_text(cx);
        if let Some(state) = &mut self.update {
            state.restarting = true;
        }
        self.sync_update_button(cx);
        self.message = "Restarting to update".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            for _ in 0..SAVE_WAIT_STEPS {
                let busy = this.update(cx, |this, _| this.pipeline.is_busy());
                if !matches!(busy, Ok(true)) {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
            this.update(cx, |this, cx| this.finish_restart(cx)).ok();
        })
        .detach();
    }

    /// Keeps the open editor's unsaved text, so the next start offers it.
    fn keep_editor_text(&mut self, cx: &mut Context<Self>) {
        let task = self.task.read(cx);
        if !task.editing() {
            return;
        }
        let Some(id) = task.detail().map(|d| d.issue) else {
            return;
        };
        let text = task.editor.read(cx).value().to_string();
        let Some(issue) = self.store.read(cx).get(id) else {
            return;
        };
        let key = issue.key.clone();
        let changed = text != *issue.description;
        let queued = self.pipeline.drop_queued(&key);
        if changed || queued || self.pipeline.has_conflict(&key) {
            self.pipeline.keep_after_lock(key, text);
        }
    }

    fn finish_restart(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.update else {
            return;
        };
        self.pipeline.keep_unsent();
        // The new app reads the kept text, so it must be on disk first.
        crate::model::drafts::flush();
        let restart = Restart::new(&state.data_dir, &state.watch.bundle, state.cli.clone());
        match (state.launch)(&restart) {
            Ok(_) => cx.quit(),
            Err(err) => {
                state.restarting = false;
                self.sync_update_button(cx);
                self.message = format!("Could not restart: {err}").into();
                cx.notify();
            }
        }
    }

    /// The title bar button. It shows only when a new build is installed.
    pub(super) fn update_button(&self) -> Option<Entity<UpdateButton>> {
        let state = self.update.as_ref()?;
        state.ready.as_ref()?;
        if state.dismissed {
            return None;
        }
        Some(state.button.clone())
    }
}

impl Render for UpdateButton {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::render_count::bump::<Self>();
        let Some(commit) = &self.commit else {
            return div().into_any_element();
        };
        let short: String = commit.chars().take(7).collect();
        let tip = format!("A new build is installed ({short}). Your unsaved text is kept");
        // GPUI Kit fills a custom button at rest with only a faint tint of
        // its color, which left the dark text hard to read. The rest fill
        // is set to full gold here, and hover goes a little lighter.
        let style = || {
            ButtonCustomVariant::new(cx)
                .color(gold(0.55))
                .foreground(UPDATE_TEXT)
                .hover(gold(0.66))
                .active(gold(0.50))
        };
        let button = Button::new("restart-update")
            .custom(style())
            .bg(UPDATE_FILL)
            .xsmall()
            // The X joins on the right, so the inner corners are square.
            .rounded_r(px(0.))
            .icon(IconName::RefreshCw)
            .label(if self.restarting {
                "Restarting…"
            } else {
                "Restart to update"
            })
            .disabled(self.restarting)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(RestartClicked)));
        let close = Button::new("dismiss-update")
            .custom(style())
            .bg(UPDATE_FILL)
            .xsmall()
            .rounded_l(px(0.))
            .icon(IconName::X)
            .disabled(self.restarting)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UpdateDismissed)));
        // A 1px gap between the two halves lets the title bar show
        // through as the divider line, as in Zed's split buttons.
        h_flex()
            .gap(px(1.))
            .child(crate::shell::tooltip::with_card(
                "restart-update-tip",
                button,
                crate::shell::tooltip::Card::text(tip),
            ))
            .child(crate::shell::tooltip::with_card(
                "dismiss-update-tip",
                close,
                crate::shell::tooltip::Card::text("Hide until the next start"),
            ))
            .into_any_element()
    }
}
