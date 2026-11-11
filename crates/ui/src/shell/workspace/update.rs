//! Restart to update: the title bar button and what a click does.
//! `model/update.rs` holds the parts with no GPUI.

use std::time::Duration;

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::ButtonCustomVariant;
use gpui_kit::{Animation, AnimationExt as _, Hsla, hsla, linear_color_stop, linear_gradient};

use super::*;
use crate::model::update::{Restart, UpdateWatch};

/// How often the app reads the installed bundle's commit.
const CHECK_EVERY: Duration = Duration::from_secs(5);
/// How long a restart waits for saves on their way, in steps of 100 ms.
/// Saves still out after that stay as kept text.
const SAVE_WAIT_STEPS: u32 = 50;
/// One full turn of the button's gradient. Slow, so it draws the eye
/// without looking busy.
const GRADIENT_TURN: Duration = Duration::from_secs(6);

/// The button is bright yellow in both themes: the one exception to the
/// monochrome rule. The text stays dark on it.
fn yellow(lightness: f32) -> Hsla {
    hsla(48. / 360., 0.96, lightness, 1.)
}
const AMBER_HUE: f32 = 40. / 360.;

/// The gradient at `delta` of a turn: its angle goes once around.
fn update_gradient(delta: f32) -> gpui_kit::Background {
    linear_gradient(
        90. + 360. * delta,
        linear_color_stop(yellow(0.62), 0.),
        linear_color_stop(hsla(AMBER_HUE, 0.96, 0.53, 1.), 1.),
    )
}

pub struct UpdateState {
    watch: UpdateWatch,
    /// The data folder whose server the restart stops.
    data_dir: PathBuf,
    /// The CLI to stop the server with, when the bundle has none.
    cli: Option<PathBuf>,
    /// The installed commit, when it differs from this build.
    ready: Option<String>,
    restarting: bool,
    /// Starts the helper. Tests record the restart instead.
    launch: fn(&Restart) -> std::io::Result<()>,
    _poll: gpui_kit::Task<()>,
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
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let alive = this.update(cx, |this, cx| this.check_for_update(cx));
                if alive.is_err() {
                    break;
                }
                cx.background_executor().timer(CHECK_EVERY).await;
            }
        });
        self.update = Some(UpdateState {
            watch,
            data_dir,
            cli,
            ready: None,
            restarting: false,
            launch: spawn_helper,
            _poll: poll,
        });
    }

    fn check_for_update(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.update else {
            return;
        };
        let newer = state.watch.newer();
        if newer != state.ready {
            state.ready = newer;
            cx.notify();
        }
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
        let restart = Restart::new(&state.data_dir, &state.watch.bundle, state.cli.clone());
        match (state.launch)(&restart) {
            Ok(_) => cx.quit(),
            Err(err) => {
                state.restarting = false;
                self.message = format!("Could not restart: {err}").into();
                cx.notify();
            }
        }
    }

    /// The title bar button. It shows only when a new build is installed.
    pub(super) fn render_update_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.update.as_ref()?;
        let commit = state.ready.as_ref()?;
        let short: String = commit.chars().take(7).collect();
        let tip = format!("A new build is installed ({short}). Your unsaved text is kept");
        Some(
            crate::shell::tooltip::with_card(
                "restart-update-tip",
                Button::new("restart-update")
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(yellow(0.55))
                            .foreground(hsla(0., 0., 0.1, 1.))
                            .hover(yellow(0.68))
                            .active(hsla(AMBER_HUE, 0.96, 0.45, 1.)),
                    )
                    .xsmall()
                    .icon(IconName::RefreshCw)
                    .label(if state.restarting {
                        "Restarting…"
                    } else {
                        "Restart to update"
                    })
                    .disabled(state.restarting)
                    .on_click(cx.listener(|this, _, _, cx| this.restart_to_update(cx)))
                    // The animation runs only while the button renders, so it
                    // costs nothing when no update is ready.
                    .with_animation(
                        "restart-update-gradient",
                        Animation::new(GRADIENT_TURN).repeat(),
                        |button, delta| button.bg(update_gradient(delta)),
                    ),
                crate::shell::tooltip::Card::text(tip),
            )
            .into_any_element(),
        )
    }
}
