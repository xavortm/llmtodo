//! LLM-591: Try it in the review panel. The model starts the preview and
//! tracks it; the workspace only hands it the ticket's recorded build.

use super::*;

impl Workspace {
    /// Opens the preview build of `id` on demo data, or brings its open
    /// preview forward. A failed start shows in the review panel.
    pub(super) fn try_it(&mut self, id: IssueId, cx: &mut Context<Self>) {
        let launch = {
            let store = self.store.read(cx);
            let (Some(issue), Some(build)) = (store.get(id), store.preview_build(id)) else {
                return;
            };
            let host = crate::model::backend::local_host();
            // The panel shows the reason when the build cannot run.
            let Ok(launch) = crate::model::try_it::check(&issue.key, build, host.as_deref()) else {
                return;
            };
            launch
        };
        let previews = self.task.read(cx).previews.clone();
        // A failure is kept as the preview's state, which the panel draws.
        let _ = previews.open(&launch);
        self.task.update(cx, |_, cx| cx.notify());
    }
}
