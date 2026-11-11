//! Runs what the views ask for. See `shell/view_events.rs`.

use super::*;
use crate::shell::view_events::{IssueAction, ViewEvent};

impl Workspace {
    pub(crate) fn on_view_event(
        &mut self,
        event: ViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ViewEvent::ShowMessage(message) => self.show_message(message, cx),
            ViewEvent::OpenKey(key) => self.open_key(&key, cx),
            ViewEvent::OnIssue(id, action) => self.with_issue(id, cx, |this, id, cx| {
                this.run_issue_action(id, action, window, cx);
            }),
            ViewEvent::MoveDown => self.move_down(&MoveDown, window, cx),
            ViewEvent::MoveUp => self.move_up(&MoveUp, window, cx),
            ViewEvent::ClickRow { id, click } => self.click_row(id, click, window, cx),
            ViewEvent::ClickHeader {
                row,
                title,
                collapsed,
            } => self.click_header(row, &title, collapsed, window, cx),
            ViewEvent::SortBy(key) => self.sort_by(key, cx),
            ViewEvent::GroupBy(group) => self.group_by(group, cx),
            ViewEvent::NewIssue => self.new_issue(&NewIssue, window, cx),
            ViewEvent::CloseAllGroups => self.close_all_groups(&CloseAllGroups, window, cx),
            ViewEvent::OpenAllGroups => self.open_all_groups(&OpenAllGroups, window, cx),
            ViewEvent::PickScope(scope) => {
                self.set_scope(scope, cx);
                self.focus_list_from_sidebar(window, cx);
            }
            ViewEvent::SwitchTracker(ix) => {
                self.switch_tracker(ix, cx);
            }
            ViewEvent::EditDescription => self.edit_description(&EditDescription, window, cx),
            ViewEvent::ShowPreview => self.show_preview(window, cx),
            ViewEvent::SetTab(ix) => self.set_tab(ix, cx),
            ViewEvent::PanelResized => self.persist_soon(cx),
            ViewEvent::RunCommand(command) => self.run_command(&command, window, cx),
            ViewEvent::OpenPaletteWith(text) => self.open_palette_with(&text, window, cx),
            ViewEvent::ChangeDependency { id, verb, other } => {
                self.change_dependency(id, &verb, &other, cx);
            }
            ViewEvent::AnswerQuestion {
                issue,
                number,
                answer,
            } => self.answer_question(issue, number, answer, window, cx),
            ViewEvent::UndoAnswer => self.undo_answer(cx),
            ViewEvent::AnswerReview { issue, verdict } => {
                self.answer_review(issue, verdict, window, cx)
            }
            ViewEvent::EditDocument => self.edit_document(window, cx),
            ViewEvent::LeaveDocEditor => self.leave_doc_editor(window, cx),
            ViewEvent::SetHideClosed(on) => self.set_hide_closed(on, cx),
            ViewEvent::SetHideClosedDays(days) => self.set_hide_closed_days(days, cx),
            ViewEvent::SetVimEditor(on) => self.set_vim_editor(on, cx),
            ViewEvent::SetDark(dark) => self.set_dark(dark, window, cx),
            ViewEvent::SetOpenLastView(on) => self.set_open_last_view(on, cx),
            ViewEvent::SetZoom(level) => self.set_zoom(level, cx),
        }
    }

    fn run_issue_action(
        &mut self,
        id: IssueId,
        action: IssueAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.run_on_marks(id, &action, cx) {
            return;
        }
        match action {
            IssueAction::CopyKey => self.copy_key(id, cx),
            IssueAction::SetStatus(status) => self.set_status(id, status, cx),
            IssueAction::ToggleLabel(label) => self.toggle_label(id, label, cx),
            IssueAction::SetWaitingOn(waiting) => self.set_waiting_on(id, waiting, cx),
            IssueAction::EditDescription => {
                if let Some(row) = self.list.read(cx).row_of(id) {
                    self.list_do(cx, |list, cx| list.select(row, cx));
                }
                self.edit_description(&EditDescription, window, cx);
            }
            IssueAction::Restore => self.restore_trashed(id, cx),
            IssueAction::Delete => self.delete_issue(id, cx),
        }
    }

    /// LLM-153: a status, label or delete from the row menu or a drop
    /// applies to every marked task when `id` is one of them. Returns
    /// whether it ran that way.
    fn run_on_marks(&mut self, id: IssueId, action: &IssueAction, cx: &mut Context<Self>) -> bool {
        let marked = self.marked(cx);
        if marked.len() < 2 || !marked.contains(&id) {
            return false;
        }
        let (verb, arg) = match action {
            IssueAction::SetStatus(status) => ("status", status.name.to_string()),
            IssueAction::ToggleLabel(label) => ("label", label.name.to_string()),
            IssueAction::Delete => ("delete", String::new()),
            _ => return false,
        };
        let Some(message) = self.run_bulk(verb, &arg, cx) else {
            return false;
        };
        self.message = message.into();
        cx.notify();
        true
    }
}
