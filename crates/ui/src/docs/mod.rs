//! The document view: one project document, shown in place of the issue
//! list when the sidebar picks it. It shows a Markdown preview, and `e`
//! opens the editor. Esc or Cmd-S saves with the version the edit started
//! from, so the server refuses a save when someone changed the text first.
//! A refused save keeps the owner's text in the editor, and `:w!`
//! overwrites.
//!
//! The view holds what it shows. It sends nothing itself: the workspace
//! asks the server and hands the answers back.

use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Editor, EditorState},
    text::TextView,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, actions, div,
    prelude::FluentBuilder as _, px,
};
use llmtodo_api_types::Document;
use llmtodo_core::DocumentId;

use crate::model::store::IssueStore;
use crate::shell::tooltip::{Card, with_card};
use crate::shell::view_events::{ViewEvent, ViewEvents};

pub const CONTEXT: &str = "DocView";

actions!(
    llmtodo_docs,
    [DocScrollDown, DocScrollUp, DocTop, DocBottom]
);

pub fn bind_keys(cx: &mut App) {
    use gpui_kit::KeyBinding;
    let ctx = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("j", DocScrollDown, ctx),
        KeyBinding::new("down", DocScrollDown, ctx),
        KeyBinding::new("ctrl-n", DocScrollDown, ctx),
        KeyBinding::new("k", DocScrollUp, ctx),
        KeyBinding::new("up", DocScrollUp, ctx),
        KeyBinding::new("ctrl-p", DocScrollUp, ctx),
        KeyBinding::new("g g", DocTop, ctx),
        KeyBinding::new("shift-g", DocBottom, ctx),
    ]);
}

pub struct DocView {
    store: Entity<IssueStore>,
    events: ViewEvents,
    /// The document the sidebar picked, before and after it loads.
    pub open: Option<DocumentId>,
    /// The copy the server sent last, with its body.
    pub doc: Option<Document>,
    pub editing: bool,
    /// The version the edit started from. A save sends it.
    pub base: i64,
    pub editor: Entity<EditorState>,
    pub focus: FocusHandle,
    scroll: ScrollHandle,
}

impl DocView {
    pub fn new(
        store: Entity<IssueStore>,
        events: ViewEvents,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language(crate::task::fold_highlighter::LANGUAGE)
                .line_number(true)
                .soft_wrap(true);
            state.set_highlighter_factory(crate::task::fold_highlighter::factory(), cx);
            state
        });
        Self {
            store,
            events,
            open: None,
            doc: None,
            editing: false,
            base: 0,
            editor,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
        }
    }

    /// Shows `id`, from the top. Its text arrives with `set_doc`.
    pub fn show(&mut self, id: DocumentId, cx: &mut Context<Self>) {
        if self.open != Some(id) {
            self.open = Some(id);
            self.doc = None;
            self.editing = false;
            self.scroll.set_offset(Default::default());
        }
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        self.doc = None;
        self.editing = false;
        cx.notify();
    }

    /// A fresh copy from the server. While the owner edits, the editor
    /// keeps the owner's text and the version it started from.
    pub fn set_doc(&mut self, doc: Document, cx: &mut Context<Self>) {
        if self.open == Some(doc.id) {
            self.doc = Some(doc);
            cx.notify();
        }
    }

    pub fn start_editing(
        &mut self,
        text: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = &self.doc else { return };
        let text = text.unwrap_or_else(|| doc.body.clone().unwrap_or_default());
        self.base = doc.version;
        self.editing = true;
        self.editor.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    pub fn stop_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn editor_focused(&self, window: &Window, cx: &App) -> bool {
        self.editor.focus_handle(cx).contains_focused(window, cx)
    }

    pub fn focused(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx) || self.editor_focused(window, cx)
    }

    pub fn take_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing {
            self.editor.update(cx, |state, cx| state.focus(window, cx));
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    fn scroll_by(&mut self, delta: gpui_kit::Pixels, cx: &mut Context<Self>) {
        let mut offset = self.scroll.offset();
        let max = self.scroll.max_offset().y;
        offset.y = (offset.y - delta).clamp(-max, px(0.));
        self.scroll.set_offset(offset);
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (title, project, version) = match &self.doc {
            Some(d) => (
                SharedString::from(d.title.clone()),
                Some(d.project.clone()),
                Some(d.version),
            ),
            None => ("Loading…".into(), None, None),
        };
        let project = project.map(|slug| {
            let catalog = &self.store.read(cx).catalog;
            crate::model::scope::Scope::Project(slug.into()).name(catalog)
        });
        let editing = self.editing;
        let events = (self.events.clone(), self.events.clone());
        h_flex()
            .h_10()
            .flex_none()
            .px_4()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                Icon::new(IconName::FileText)
                    .small()
                    .text_color(theme.muted_foreground),
            )
            .child(div().flex_1().min_w_0().truncate().text_sm().child(title))
            .when_some(project, |this, p| {
                this.child(div().text_xs().text_color(theme.muted_foreground).child(p))
            })
            .when_some(version, |this, v| {
                this.child(
                    div()
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.muted_foreground)
                        .child(format!("v{v}")),
                )
            })
            .child(
                h_flex()
                    .gap_1()
                    .child(with_card(
                        "doc-edit-tip",
                        Button::new("doc-edit")
                            .ghost()
                            .xsmall()
                            .label("Edit")
                            .selected(editing)
                            .disabled(self.doc.is_none())
                            .on_click(move |_, window, cx| {
                                events.0.emit(ViewEvent::EditDocument, window, cx);
                            }),
                        Card::text("Edit the document (e)"),
                    ))
                    .child(with_card(
                        "doc-preview-tip",
                        Button::new("doc-preview")
                            .ghost()
                            .xsmall()
                            .label("Preview")
                            .selected(!editing)
                            .on_click(move |_, window, cx| {
                                events.1.emit(ViewEvent::LeaveDocEditor, window, cx);
                            }),
                        Card::text("Save and show the preview (Esc)"),
                    )),
            )
    }
}

use gpui_kit::component::Disableable as _;

impl Render for DocView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.render_header(cx);
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let body = self.doc.as_ref().and_then(|d| d.body.clone());
        let id = self.open.map(|i| i.to_string()).unwrap_or_default();
        let store = self.store.read(cx);
        let server = store.server_url.clone();
        let link_events = self.events.clone();
        let content = match (&body, self.editing) {
            (_, true) => div()
                .flex_1()
                .min_h_0()
                .child(Editor::new(&self.editor).h_full())
                .into_any_element(),
            (Some(body), false) if !body.trim().is_empty() => {
                let prepared = crate::task::preview::prepare(body, server.as_deref(), |key| {
                    store.by_key(key).is_some()
                });
                v_flex()
                    .id("doc-preview-body")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px_4()
                    .py_3()
                    .child(
                        TextView::markdown(SharedString::from(format!("doc-{id}")), prepared)
                            .text_sm()
                            .on_link_click(move |url, _, window, cx| {
                                match crate::task::preview::link_target(url, server.as_deref()) {
                                    crate::task::preview::LinkTarget::Issue(key) => {
                                        link_events.emit(
                                            ViewEvent::OpenKey(key.to_string()),
                                            window,
                                            cx,
                                        );
                                    }
                                    crate::task::preview::LinkTarget::Browser(url) => {
                                        cx.open_url(&url)
                                    }
                                }
                            }),
                    )
                    .into_any_element()
            }
            (Some(_), false) => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_1()
                .text_sm()
                .text_color(muted)
                .child("This document is empty")
                .child(div().text_xs().child("Press e to write it"))
                .into_any_element(),
            (None, false) => div()
                .p_4()
                .text_sm()
                .text_color(muted)
                .child("Loading the document…")
                .into_any_element(),
        };
        v_flex()
            .id("doc-view")
            .test_support()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &DocScrollDown, window, cx| {
                this.scroll_by(window.line_height() * 3., cx)
            }))
            .on_action(cx.listener(|this, _: &DocScrollUp, window, cx| {
                this.scroll_by(-(window.line_height() * 3.), cx)
            }))
            .on_action(cx.listener(|this, _: &DocTop, _, cx| {
                this.scroll.set_offset(Default::default());
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &DocBottom, _, cx| {
                let max = this.scroll.max_offset().y;
                this.scroll.set_offset(gpui_kit::point(px(0.), -max));
                cx.notify();
            }))
            .flex_1()
            .min_w_0()
            .h_full()
            .child(header)
            .child(content)
    }
}

#[cfg(test)]
mod tests;
