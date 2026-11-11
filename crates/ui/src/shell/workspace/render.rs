//! Draws the window: the split, the title bar, the status bar, the help
//! overlay and the task menu.

use super::*;

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
    fn render_split(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        group
            .with_handle_appearance(std::rc::Rc::new(Self::divider))
            .child(self.list.clone().into_any_element())
            .child(
                // Fixed: only a drag on the divider changes its size, never
                // the content of the task that is open.
                resizable_panel()
                    .visible(open)
                    .size(size)
                    .size_range(range)
                    .fixed()
                    .when(open, |this| this.child(self.task.clone())),
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
        let mode = if self.claims.read(cx).is_open() {
            "overlay"
        } else {
            self.vim_mode(window, cx)
        };
        context.set("mode", mode);
        context
    }

    /// The project name on the left, its folder on the right. A click on
    /// the folder copies it. Restart to update sits left of the folder.
    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let update = self.render_update_button(cx);
        let theme = cx.theme();
        let (name, path) = self.project.clone();
        let hover = theme.foreground.opacity(0.08);
        let pressed = theme.foreground.opacity(0.16);
        let sidebar_open = self.sidebar_shown();
        // Which tracker is open, when there is a choice. The real one says
        // so plainly, so the owner never edits real data by mistake.
        let tracker = self.store.read(cx).open_tracker().map(|t| {
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
            h_flex()
                .id("open-tracker")
                .test_support()
                .flex_none()
                .gap_1()
                .px_1p5()
                .rounded(theme.radius)
                .border_1()
                .border_color(if t.real {
                    theme.foreground
                } else {
                    theme.border
                })
                .text_xs()
                .text_color(if t.real {
                    theme.foreground
                } else {
                    theme.muted_foreground
                })
                .when(t.real, |this| {
                    this.font_weight(gpui_kit::FontWeight::SEMIBOLD)
                })
                .tooltip(crate::shell::tooltip::Card::text(tip).builder())
                .child(Icon::new(IconName::Database).xsmall())
                .child(label)
        });
        // Owner rule: the title bar is the plain page color, with no fill
        // and no bottom border.
        TitleBar::new().bg(theme.background).border_b_0().child(
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
                                    this.toggle_sidebar(&ToggleSidebar, window, cx);
                                })),
                            crate::shell::tooltip::Card::text(if sidebar_open {
                                "Hide sidebar (Cmd-B)"
                            } else {
                                "Show sidebar (Cmd-B)"
                            }),
                        ))
                        .child(div().text_color(theme.foreground).child(name))
                        .children(tracker),
                )
                .child(
                    h_flex().gap_2().children(update).child(
                        div()
                            .id("project-path")
                            .px_1()
                            .rounded(theme.radius)
                            .cursor_pointer()
                            .font_family(theme.mono_font_family.clone())
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .hover(move |s| s.bg(hover))
                            .active(move |s| s.bg(pressed))
                            .tooltip(crate::shell::tooltip::Card::text("Copy path").builder())
                            .child(path.clone())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                    path.to_string(),
                                ));
                                this.message = format!("Copied {path}").into();
                                cx.notify();
                            })),
                    ),
                ),
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
                        })),
                    crate::shell::tooltip::Card::text("Ask the server again (:sync)"),
                )),
        )
    }

    fn render_help(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

    fn render_status_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let task = self.task.read(cx);
        let mode = if task.editing() && task.editor_focused(window, cx) {
            task.editor_vim.name()
        } else {
            self.vim_mode(window, cx)
        };
        let count = self.visible_count(cx);
        let scope = self.list.read(cx).scope.clone();
        let mut count = match scope {
            Scope::All => format!("{count} issues"),
            Scope::Settings => String::new(),
            _ => format!(
                "{count} issues in {}",
                scope.name(&self.store.read(cx).catalog)
            ),
        };
        let hidden = self.list.read(cx).hidden_closed();
        if hidden > 0 {
            count.push_str(&format!(", {hidden} old closed hidden"));
        }
        let marked = self.list.read(cx).marked.len();
        if marked > 0 {
            count.push_str(&format!(", {marked} marked"));
        }
        let connection = self.store.read(cx).connection.clone();
        let claimed = crate::issues::claims::claim_rows(self.store.read(cx)).len();
        let theme = cx.theme();
        let hover = theme.foreground.opacity(0.08);
        let pressed = theme.foreground.opacity(0.16);
        let radius = theme.radius;
        // The offer repeats the answer message, so that message shows once.
        let offer = self.answer_undo.as_ref().map(|u| u.message.clone());
        let plain_message = (offer.as_ref() != Some(&self.message)).then(|| self.message.clone());
        let undo = crate::shell::widgets::button("answer-undo-button", false)
            .xsmall()
            .label("Undo")
            .tooltip("Undo this answer (u in the inbox)")
            .on_click(cx.listener(|this, _, _, cx| this.undo_answer(cx)));
        StatusBar::new()
            .left(
                h_flex()
                    .gap_3()
                    .text_xs()
                    .child(
                        div()
                            .font_family(theme.mono_font_family.clone())
                            .child(mode.to_uppercase()),
                    )
                    .child(count)
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
                    .children(plain_message),
            )
            .right(
                h_flex()
                    .gap_3()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .map(|this| match &connection {
                        Connection::Local => this,
                        Connection::Connecting => this.child("Connecting…"),
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
                                    this.show_claims(&ShowClaims, window, cx)
                                })),
                        )
                    })
                    .child("? keys"),
            )
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .on_action(cx.listener(Self::sidebar_down))
            .on_action(cx.listener(Self::sidebar_up))
            .on_action(cx.listener(Self::sidebar_top))
            .on_action(cx.listener(Self::sidebar_bottom))
            .on_action(cx.listener(Self::leave_sidebar))
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
            .on_action(cx.listener(|this, _: &app_menus::MenuHelp, window, cx| {
                this.toggle_help(&ToggleHelp, window, cx)
            }))
            .relative()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_title_bar(cx))
            .children(self.render_not_primary(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(self.sidebar_shown(), |this| {
                        this.child(self.sidebar.clone())
                    })
                    .map(|this| {
                        if self.settings_shown(cx) {
                            this.child(self.settings_view.clone())
                        } else if self.inbox_shown(cx) {
                            this.child(self.inbox.clone())
                        } else if self.doc_shown(cx) {
                            this.child(self.doc_view.clone())
                        } else {
                            this.child(self.render_split(window, cx))
                        }
                    }),
            )
            .child(self.render_status_bar(window, cx))
            .when(palette_open, |this| this.child(self.palette.clone()))
            .when(self.create.read(cx).is_open(), |this| {
                this.child(self.create.clone())
            })
            .when(self.help_open, |this| this.child(self.render_help(cx)))
            .when(claims_open, |this| this.child(self.claims.clone()))
    }
}
