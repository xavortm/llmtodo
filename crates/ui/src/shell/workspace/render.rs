//! Draws the window: the split, the help overlay and the task menu. The
//! title bar and the status bar are their own views, in `bars.rs`.

use super::*;
use crate::shell::backdrop::Slot;

/// The sidebar's width, in rem: `w_56`.
const SIDEBAR_REM: f32 = 14.;

impl Workspace {
    // Layout.

    /// Below this width for the list and the task view together, the task
    /// view moves under the list. In rem, so it follows the font size.
    const STACK_BELOW_REM: f32 = 64.;
    /// The list keeps room for status, key, a readable title, tags,
    /// assignee and date.
    const LIST_MIN_WIDTH_REM: f32 = 30.;
    /// The task view keeps room for its key, a title and the Edit / Preview
    /// buttons.
    const DETAIL_MIN_WIDTH_REM: f32 = 22.;
    const LIST_MIN_HEIGHT_REM: f32 = 10.;
    const DETAIL_MIN_HEIGHT_REM: f32 = 12.;

    /// Whether the task view sits under the list, because the window is too
    /// narrow for both side by side.
    pub fn stacked(&self, window: &Window) -> bool {
        let rem = window.rem_size();
        window.viewport_size().width < rem * Self::STACK_BELOW_REM
    }

    /// The task view width beside the list, before the owner drags the
    /// divider. `room` is the width the two share.
    fn auto_detail_width(
        &self,
        room: gpui_kit::Pixels,
        rem: gpui_kit::Pixels,
        cx: &App,
    ) -> gpui_kit::Pixels {
        let list_want = self.list.read(cx).comfortable_width(rem, cx);
        crate::shell::split::detail_width(
            room,
            list_want,
            rem * Self::LIST_MIN_WIDTH_REM,
            rem * Self::DETAIL_MIN_WIDTH_REM,
        )
    }

    /// Until the owner drags the divider, resizes the task view whenever
    /// the room or the list's content changes, so the list grows first.
    /// It does nothing while the room and the wanted width stay the same,
    /// so it never fights a drag.
    fn sync_auto_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.layout.split_sizes.0.is_some() || self.stacked(window) || self.detail(cx).is_none()
        {
            return;
        }
        let state = self.layout.split_side.read(cx);
        let room = state.container_size();
        let Some(current) = state.sizes().get(1).copied() else {
            return;
        };
        if room <= gpui_kit::px(0.) || current <= gpui_kit::px(0.) {
            return;
        }
        let want = self.auto_detail_width(room, window.rem_size(), cx);
        if self.layout.auto_split == Some((room, want)) {
            return;
        }
        self.layout.auto_split = Some((room, want));
        if (f32::from(current) - f32::from(want)).abs() >= 0.5 {
            self.layout
                .split_side
                .update(cx, |state, cx| state.resize_panel(1, want, window, cx));
        }
    }

    /// The list and the task view, side by side or stacked. Drag the
    /// divider to resize the task view. Each layout keeps its own size.
    /// `filters_open` runs from 0 to 1: how far the filter panel is open.
    fn render_split(
        &self,
        filters_open: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let stacked = self.stacked(window);
        let rem = window.rem_size();
        let viewport = window.viewport_size();
        // The task view never takes so much room that the list loses its
        // titles, and never gets so small that its own title bar breaks.
        let (group, saved, range) = if stacked {
            // The title bar and the status bar take about 4.5 rem.
            let room = viewport.height - rem * 4.5;
            let max =
                (room - rem * Self::LIST_MIN_HEIGHT_REM).max(rem * Self::DETAIL_MIN_HEIGHT_REM);
            (
                gpui_kit::component::resizable::v_resizable("main-split-v")
                    .with_state(&self.layout.split_stacked),
                self.layout
                    .split_sizes
                    .1
                    .map(gpui_kit::px)
                    .unwrap_or(rem * 22.),
                rem * Self::DETAIL_MIN_HEIGHT_REM..max,
            )
        } else {
            let max = (viewport.width - rem * Self::LIST_MIN_WIDTH_REM)
                .max(rem * Self::DETAIL_MIN_WIDTH_REM);
            (
                h_resizable("main-split-h").with_state(&self.layout.split_side),
                self.layout
                    .split_sizes
                    .0
                    .map(gpui_kit::px)
                    .unwrap_or_else(|| {
                        let room = self.layout.split_side.read(cx).container_size();
                        let room = if room > gpui_kit::px(0.) {
                            room
                        } else {
                            viewport.width
                        };
                        self.auto_detail_width(room, rem, cx)
                    }),
                rem * Self::DETAIL_MIN_WIDTH_REM..max,
            )
        };
        let size = saved.clamp(range.start, range.end);
        let open = self.detail(cx).is_some();
        // The filter panel sits at the right of the list and pushes it
        // narrower. Nothing covers the list, so it takes clicks and shows
        // each filter change at once. With animations on, its column grows
        // and shrinks as the sidebar's does, both ways, and the panel fades
        // with it (LLM-467).
        // LLM-566: the list image sits behind the list, the activity strip
        // and the filter panel.
        let list = h_flex()
            .size_full()
            .relative()
            .overflow_hidden()
            .children({
                // LLM-576: a list image may scroll with the rows.
                let scroll = self.list.read(cx).scroll_handle().clone();
                crate::shell::backdrop::layer_with(
                    &[Slot::List],
                    Some(std::rc::Rc::new(move || {
                        scroll.0.borrow().base_handle.offset()
                    })),
                    cx,
                )
            })
            .child(
                // The rows' hover fills draw under the list, as a view of
                // their own (LLM-345).
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(self.list.read(cx).fill_layer.clone())
                            .child(cached(&self.list)),
                    )
                    // LLM-560: tasks created and completed per day, under
                    // a project's list only.
                    .when(
                        self.activity_strip
                            && !crate::shell::simplify::on(cx)
                            && self.day_chart.read(cx).slug(cx).is_some(),
                        |this| {
                            this.child(self.day_chart.clone().cached(
                                gpui_kit::StyleRefinement::default().w_full().flex_none().h(
                                    gpui_kit::px(crate::issues::day_chart::Metrics::now(cx).height),
                                ),
                            ))
                        },
                    ),
            )
            .when(filters_open > 0., |this| {
                // Cached, over its background and its rows' hover fills,
                // so a hover or a list change does not draw it (LLM-345).
                // The panel keeps its full width and rides the column's
                // left edge, so it slides out to the right as it shrinks.
                let fills = self.filter_panel.read(cx).fill_layer.clone();
                let panel_rem = crate::issues::filter_panel::PANEL_REM;
                this.child(
                    div()
                        .h_full()
                        .flex_none()
                        .w(gpui_kit::rems(panel_rem * filters_open))
                        .relative()
                        .overflow_hidden()
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .w(gpui_kit::rems(panel_rem))
                                .opacity(filters_open)
                                .bg(crate::shell::backdrop::surface(
                                    cx.theme().background,
                                    &[Slot::Window, Slot::List],
                                    cx,
                                ))
                                .child(fills)
                                .child(cached(&self.filter_panel)),
                        ),
                )
            });
        group
            .with_handle_appearance(std::rc::Rc::new(Self::divider))
            .child(list.into_any_element())
            .child(
                // Fixed: only a drag on the divider changes its size, never
                // the content of the task that is open.
                resizable_panel()
                    .visible(open)
                    .size(size)
                    .size_range(range)
                    .fixed()
                    .when(open, |this| {
                        // With animations on, the task view fades in and
                        // slides a short way from the right.
                        // The tab bar's hover background draws over the
                        // task view, as a view of its own (LLM-220).
                        let tab_hover = self.task.read(cx).tab_hover.clone();
                        // LLM-579: the task shader draws as a surface only
                        // once the panel has faded in.
                        let fade: crate::shell::backdrop::ParentFade = Default::default();
                        let layer_fade = fade.clone();
                        this.child(crate::shell::motion::slide_in(
                            div()
                                .size_full()
                                .relative()
                                .children({
                                    // LLM-576: a task image may scroll
                                    // with the description.
                                    let scroll = self.task.read(cx).preview_scroll.clone();
                                    crate::shell::backdrop::layer_faded(
                                        &[Slot::Task],
                                        Some(std::rc::Rc::new(move || scroll.offset())),
                                        layer_fade,
                                        cx,
                                    )
                                })
                                .child(cached(&self.task))
                                .child(tab_hover),
                            "task-slide",
                            1.5,
                            Some(fade),
                            cx,
                        ))
                    }),
            )
    }

    /// The line between the list and the task view. It is 2 px, so the
    /// owner can see where to drag, and darkens on hover and while dragged.
    fn divider(
        handle: &gpui_kit::base::ResizeHandleContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        use gpui_kit::base::ResizeHandleState;
        let theme = cx.theme();
        let color = match handle.state() {
            ResizeHandleState::Idle => theme.border,
            ResizeHandleState::Hovered => theme.muted_foreground.opacity(0.6),
            _ => theme.foreground.opacity(0.7),
        };
        let line = div().flex_none().bg(color);
        Some(
            match handle.axis() {
                gpui_kit::Axis::Horizontal => line.h_full().w(gpui_kit::px(2.)),
                gpui_kit::Axis::Vertical => line.w_full().h(gpui_kit::px(2.)),
            }
            .into_any_element(),
        )
    }

    fn key_context(&self, window: &Window, cx: &App) -> KeyContext {
        let mut context = KeyContext::default();
        context.add(CONTEXT);
        // The claims list takes its own keys, so no list key fires under it.
        // A day list opened by hover leaves the keys to the list.
        let mode = if self.claims.read(cx).is_open() || self.day_tasks.read(cx).is_pinned() {
            "overlay"
        } else {
            self.vim_mode(window, cx)
        };
        context.set("mode", mode);
        context
    }

    /// The sidebar column: the scope rows, and the cat below them. The cat
    /// is a sibling of the sidebar view, not a child, and the sidebar is
    /// cached. So a new cat frame does not draw the rows again, and they
    /// count every issue. The sidebar notifies itself when its data changes.
    fn render_sidebar(&self, open: f32, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let panel = v_flex()
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(gpui_kit::rems(SIDEBAR_REM))
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            // The sidebar keeps its own color, so the window image draws
            // again over it, lined up with the window (LLM-566).
            .children(crate::shell::backdrop::layer(
                &[Slot::Window, Slot::Sidebar],
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.sidebar.read(cx).fill_layer.clone())
                    .child(cached(&self.sidebar)),
            )
            .when(self.mascot_on, |this| {
                this.child(div().flex_none().pb_1().child(self.mascot.clone()))
            });
        // `open` runs from 0 to 1. With animations on, the column grows and
        // shrinks, so the view beside it moves with it, and the panel rides
        // its right edge, so it slides in and out. Off, it is 0 or 1.
        div()
            .w(gpui_kit::rems(SIDEBAR_REM * open))
            .flex_none()
            .h_full()
            .relative()
            .overflow_hidden()
            .child(panel)
    }

    /// The scope's project name and its repo or folder, for the title bar.
    pub(crate) fn title_place(&self, cx: &App) -> (SharedString, TitlePlace) {
        let store = self.store.read(cx);
        let tracker = store
            .trackers
            .get(store.tracker)
            .map_or_else(|| SharedString::from("llmtodo"), |t| t.name.clone());
        let home = std::env::var("HOME").ok();
        title_place(
            &self.list.read(cx).scope,
            &store.catalog,
            &tracker,
            self.project_dir.as_deref(),
            home.as_deref(),
        )
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn not_primary(&self) -> Option<&Option<SharedString>> {
        self.not_primary.as_ref()
    }

    /// A bar under the title bar while this machine is not the primary
    /// copy. The server still serves reads, so the app keeps working, but
    /// every write is refused until a `sync pull` here.
    fn render_not_primary(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let holder = self.not_primary.as_ref()?;
        let theme = cx.theme();
        let who = match holder {
            Some(host) => format!("{host} holds the sync lock. "),
            None => String::new(),
        };
        Some(
            h_flex()
                .id("not-primary")
                .test_support()
                .flex_none()
                .px_3()
                .py_1()
                .gap_2()
                .text_sm()
                .bg(theme.muted)
                .text_color(theme.foreground)
                .border_b_1()
                .border_color(theme.border)
                .child(Icon::new(IconName::Lock).small())
                .child(div().flex_1().min_w_0().child(format!(
                    "This machine is not the primary copy, so you can read but not write. {who}Run `llmtodo sync pull` here to take over."
                )))
                .child(crate::shell::tooltip::with_card(
                    "check-primary-tip",
                    Button::new("check-primary")
                        .ghost()
                        .xsmall()
                        .label("Check again")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.run_command("sync", window, cx);
                            this.flash("Checking", window, cx);
                        })),
                    crate::shell::tooltip::Card::text("Ask the server again (:sync)"),
                )),
        )
    }

    fn render_help(&self, cx: &mut Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme();
        let mono = theme.mono_font_family.clone();
        div()
            .absolute()
            .top_10()
            .left_4()
            .right_4()
            .flex()
            .justify_center()
            .child(
                h_flex()
                    .id("help")
                    .flex_wrap()
                    .items_start()
                    .gap_x_8()
                    .gap_y_4()
                    .p_4()
                    .bg(theme.popover)
                    .text_color(theme.popover_foreground)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius)
                    .shadow_lg()
                    .children(HELP.iter().map(|(title, rows)| {
                        // No row may wrap: keys and text both stay on one
                        // line, and the column grows to fit the longest row.
                        v_flex()
                            .flex_none()
                            .gap_1()
                            .child(div().text_sm().pb_1().whitespace_nowrap().child(*title))
                            .children(rows.iter().map(|(keys, what)| {
                                h_flex()
                                    .gap_3()
                                    .text_xs()
                                    .whitespace_nowrap()
                                    .child(
                                        div()
                                            .w_32()
                                            .flex_none()
                                            .font_family(mono.clone())
                                            .text_color(theme.foreground)
                                            .child(*keys),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_color(theme.muted_foreground)
                                            .child(*what),
                                    )
                            }))
                    })),
            )
    }
}

/// A view drawn from its cache until it notifies. A cat frame or an
/// update-button frame draws the workspace again, because each is a view
/// inside it, but not the views drawn this way (LLM-169). So each such view
/// must notify itself on every change it shows: it observes the store, and
/// the workspace notifies it when it sets the view's fields.
fn cached<V: Render>(view: &Entity<V>) -> impl IntoElement + use<V> {
    view.clone()
        .cached(gpui_kit::StyleRefinement::default().size_full())
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::render_count::bump::<Self>();
        self.sync_auto_split(window, cx);
        // Only when the owner was typing: focus elsewhere stays there. The
        // preview takes focus after this frame, once it is drawn.
        if std::mem::take(&mut self.refocus_after_lock)
            && self.task.read(cx).editor_focused(window, cx)
        {
            let focus = self.task.read(cx).focus.clone();
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
        }
        let palette_open = self.palette.read(cx).is_open();
        let claims_open = self.claims.read(cx).is_open();
        let sidebar_open =
            crate::shell::motion::panel_open("sidebar-open", self.sidebar_shown(), window, cx);
        let filters_open = crate::shell::motion::panel_open(
            "filter-panel-open",
            self.filter_panel.read(cx).is_open(),
            window,
            cx,
        );
        self.sync_bars(window, cx);
        let status_height = self.status_bar_height(window, cx);
        let theme = cx.theme();
        v_flex()
            .id("workspace")
            .key_context(self.key_context(window, cx))
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::toggle_column))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::go_top))
            .on_action(cx.listener(Self::go_bottom))
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::back))
            .on_action(cx.listener(Self::view_back))
            .on_action(cx.listener(Self::view_forward))
            // Mouse buttons 4 and 5 walk the view history.
            .on_mouse_down(
                gpui_kit::MouseButton::Navigate(gpui_kit::NavigationDirection::Back),
                cx.listener(|this, _, window, cx| this.view_back(&ViewBack, window, cx)),
            )
            .on_mouse_down(
                gpui_kit::MouseButton::Navigate(gpui_kit::NavigationDirection::Forward),
                cx.listener(|this, _, window, cx| this.view_forward(&ViewForward, window, cx)),
            )
            .on_action(cx.listener(Self::focus_filter))
            .on_action(cx.listener(Self::open_palette))
            .on_action(cx.listener(Self::edit_description))
            .on_action(cx.listener(Self::edit_external))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::toggle_help))
            .on_action(cx.listener(Self::show_claims))
            .on_action(cx.listener(Self::toggle_activity_strip))
            .on_action(cx.listener(Self::toggle_backgrounds))
            .on_action(cx.listener(Self::show_inbox))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::focus_list_action))
            .on_action(cx.listener(Self::focus_detail))
            .on_action(cx.listener(Self::scroll_down))
            .on_action(cx.listener(Self::scroll_up))
            .on_action(cx.listener(Self::half_page_down))
            .on_action(cx.listener(Self::half_page_up))
            .on_action(cx.listener(Self::scroll_top))
            .on_action(cx.listener(Self::scroll_bottom))
            .on_action(cx.listener(Self::cycle_group))
            .on_action(cx.listener(Self::toggle_group))
            .on_action(cx.listener(Self::close_group))
            .on_action(cx.listener(Self::open_group))
            .on_action(cx.listener(Self::close_all_groups))
            .on_action(cx.listener(Self::open_all_groups))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::new_issue))
            .on_action(cx.listener(Self::toggle_mark))
            .on_action(cx.listener(Self::pick_priority))
            .on_action(cx.listener(Self::sidebar_down))
            .on_action(cx.listener(Self::sidebar_up))
            .on_action(cx.listener(Self::sidebar_top))
            .on_action(cx.listener(Self::sidebar_bottom))
            .on_action(cx.listener(Self::leave_sidebar))
            .on_action(cx.listener(Self::sidebar_toggle_group))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::minimize))
            .on_action(cx.listener(Self::toggle_full_screen))
            .on_action(cx.listener(Self::open_settings))
            .on_action(
                cx.listener(|this, _: &app_menus::MenuNewIssue, window, cx| {
                    this.new_issue(&NewIssue, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &app_menus::MenuPalette, window, cx| {
                this.open_palette(&OpenPalette, window, cx)
            }))
            .on_action(cx.listener(|this, _: &app_menus::MenuFilter, window, cx| {
                this.focus_filter(&FocusFilter, window, cx)
            }))
            .on_action(cx.listener(|this, _: &app_menus::MenuClaims, window, cx| {
                this.show_claims(&ShowClaims, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &app_menus::MenuActivityStrip, window, cx| {
                    this.toggle_activity_strip(&ToggleActivityStrip, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &app_menus::MenuEditBackgrounds, window, cx| {
                    this.open_background_panel(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &app_menus::MenuBackgrounds, window, cx| {
                    this.toggle_backgrounds(&ToggleBackgrounds, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &app_menus::MenuHelp, window, cx| {
                this.toggle_help(&ToggleHelp, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &app_menus::MenuCheckForUpdates, _, cx| {
                    this.check_for_updates(cx)
                }),
            )
            .relative()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            // LLM-566: the window image, under every panel.
            .children(crate::shell::backdrop::layer(&[Slot::Window], cx))
            // Each bar is its own cached view, so a cat frame or a cursor
            // blink draws neither (LLM-348).
            .child(
                self.title_bar.clone().cached(
                    gpui_kit::StyleRefinement::default()
                        .w_full()
                        .flex_none()
                        .h(gpui_kit::component::TITLE_BAR_HEIGHT),
                ),
            )
            .children(self.render_not_primary(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(sidebar_open > 0., |this| {
                        this.child(self.render_sidebar(sidebar_open, cx))
                    })
                    .map(|this| {
                        if self.settings_shown(cx) {
                            this.child(cached(&self.settings_view))
                        } else if self.inbox_shown(cx) {
                            this.child(cached(&self.inbox))
                        } else if self.doc_shown(cx) {
                            this.child(cached(&self.doc_view))
                        } else {
                            this.child(self.render_split(filters_open, window, cx))
                        }
                    }),
            )
            .child(
                self.status_bar.clone().cached(
                    gpui_kit::StyleRefinement::default()
                        .w_full()
                        .flex_none()
                        .h(status_height),
                ),
            )
            .when(palette_open, |this| this.child(self.palette.clone()))
            .when(self.create.read(cx).is_open(), |this| {
                this.child(self.create.clone())
            })
            .when(self.help_open, |this| {
                this.child(crate::shell::motion::enter(
                    self.render_help(cx),
                    "help-enter",
                    cx,
                ))
            })
            .when(claims_open, |this| this.child(self.claims.clone()))
            .when(self.day_tasks.read(cx).is_open(), |this| {
                this.child(self.day_tasks.clone())
            })
            .when(self.bg_panel_open, |this| {
                this.child(self.render_background_panel(window, cx))
            })
            .on_drag_move(cx.listener(Self::drag_background_panel))
            .on_action(cx.listener(
                |this, _: &crate::settings::backgrounds::CloseBackgroundPanel, window, cx| {
                    this.close_background_panel(window, cx)
                },
            ))
            .children(self.render_flash())
    }
}

/// What the Backgrounds panel's header carries while it is dragged.
#[derive(Clone)]
pub struct BackgroundPanelDrag;

impl Render for BackgroundPanelDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::Empty
    }
}

/// The gap between the panel and the window's edge.
const PANEL_MARGIN: f32 = 16.;

impl Workspace {
    /// LLM-580: the floating Backgrounds panel. It opens in the middle,
    /// over a dimmed app, so it is easy to find. Once dragged by its
    /// header, the dimming goes and the panel stays where it was put, so
    /// the images show while they change. It stays solid, with a strong
    /// border, so it reads over any image.
    fn render_background_panel(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let bounds = self.bg_panel_bounds.clone();
        let margin = gpui_kit::px(PANEL_MARGIN);
        let header = h_flex()
            .id("bg-panel-header")
            .test_support()
            .justify_between()
            .items_center()
            .pb_2()
            .cursor_grab()
            .child(div().text_sm().child("Backgrounds"))
            .child(
                Button::new("bg-panel-close")
                    .icon(gpui_kit::assets::IconName::Close)
                    .ghost()
                    .xsmall()
                    .tab_stop(false)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.close_background_panel(window, cx)),
                    ),
            )
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(move |this, e: &gpui_kit::MouseDownEvent, _, _| {
                    this.bg_panel_grab = e.position - this.bg_panel_bounds.get().origin;
                }),
            )
            .on_drag(BackgroundPanelDrag, |drag, _, _, cx| {
                cx.new(|_| drag.clone())
            });
        let body = div()
            .id("bg-panel-box")
            .w(gpui_kit::rems(27.))
            .max_h(window.viewport_size().height - margin * 2.)
            .overflow_y_scroll()
            .p_3()
            .rounded(theme.radius_lg)
            .border_2()
            .border_color(theme.foreground.opacity(0.55))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .shadow_lg()
            .occlude()
            .child(header)
            .child(self.bg_panel.clone());
        // The outer box has no padding or border, so the measure inside it
        // starts exactly where the panel starts, and a drag does not jump.
        let panel = div()
            .id("bg-panel")
            .test_support()
            .relative()
            .when_some(self.bg_panel_at, |this, at| {
                this.absolute().left(at.x).top(at.y)
            })
            .child(
                gpui_kit::canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            )
            .child(body);
        match self.bg_panel_at {
            Some(_) => panel.into_any_element(),
            // Not moved yet: centred over the dimmed app.
            None => div()
                .id("bg-panel-scrim")
                .test_support()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::black().opacity(0.55))
                .occlude()
                .child(panel)
                .into_any_element(),
        }
    }

    fn drag_background_panel(
        &mut self,
        e: &gpui_kit::DragMoveEvent<BackgroundPanelDrag>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let size = self.bg_panel_bounds.get().size;
        let view = window.viewport_size();
        let mut at = e.event.position - self.bg_panel_grab;
        // Keep the header on screen.
        at.x = at.x.max(gpui_kit::px(0.)).min(view.width - size.width);
        at.y =
            at.y.max(gpui_kit::px(0.))
                .min(view.height - gpui_kit::px(40.));
        if self.bg_panel_at != Some(at) {
            self.bg_panel_at = Some(at);
            cx.notify();
        }
    }
}

impl Workspace {
    /// LLM-199: the flash card, centred above the pointer. It floats over
    /// every other layer and takes no clicks.
    fn render_flash(&self) -> Option<impl IntoElement + use<>> {
        let flash = self.flash.as_ref()?;
        Some(
            gpui_kit::deferred(
                gpui_kit::anchored()
                    .position(flash.at)
                    .anchor(gpui_kit::Anchor::BottomCenter)
                    .snap_to_window_with_margin(gpui_kit::px(8.))
                    .child(div().id("flash").test_support().child(flash.view.clone())),
            )
            .with_priority(2),
        )
    }
}

/// `text` on one line: line breaks and runs of spaces become one space.
pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
