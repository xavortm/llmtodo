//! The views the owner visited, for back and forward. `history.rs` is
//! something else: the Activity and Sessions tabs of a task.

use std::collections::VecDeque;

use crate::model::data::IssueId;
use crate::model::scope::Scope;
use crate::model::store::IssueStore;

/// The most earlier views the history keeps.
const MAX_VIEWS: usize = 100;

/// One place the owner can return to.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub scope: Scope,
    /// The open task and the position of its tab.
    pub task: Option<(IssueId, usize)>,
    pub claims: bool,
}

impl View {
    /// Whether the data still holds this view. A trashed task exists only
    /// in the Trash.
    pub fn exists(&self, store: &IssueStore) -> bool {
        self.scope.exists(&store.catalog)
            && self.task.is_none_or(|(id, _)| {
                store
                    .get(id)
                    .is_some_and(|issue| !issue.deleted || self.scope == Scope::Trash)
            })
    }
}

/// A bounded stack of views with a current one, like a browser's history.
#[derive(Debug)]
pub struct ViewHistory<V> {
    earlier: VecDeque<V>,
    current: Option<V>,
    later: Vec<V>,
}

impl<V> Default for ViewHistory<V> {
    fn default() -> Self {
        Self {
            earlier: VecDeque::new(),
            current: None,
            later: Vec::new(),
        }
    }
}

impl<V: Clone + PartialEq> ViewHistory<V> {
    /// The owner reached `view`. The current view becomes an earlier one,
    /// and the views ahead are dropped, unless `view` is already current.
    pub fn visit(&mut self, view: V) {
        if self.current.as_ref() == Some(&view) {
            return;
        }
        if let Some(current) = self.current.replace(view) {
            self.remember(current);
        }
        self.later.clear();
    }

    /// `view` takes the place of the current one, in both directions. Moving
    /// the list selection does this, so a walk down the list is one view.
    /// An empty history stays empty until the first visit.
    pub fn replace(&mut self, view: V) {
        if let Some(current) = &mut self.current {
            *current = view;
        }
    }

    /// Moves to the newest earlier view that still exists. Views that do
    /// not exist are dropped. With none left the current view stays.
    pub fn back(&mut self, exists: impl Fn(&V) -> bool) -> Option<V> {
        while let Some(view) = self.earlier.pop_back() {
            if exists(&view) {
                if let Some(current) = self.current.replace(view.clone()) {
                    self.later.push(current);
                }
                return Some(view);
            }
        }
        None
    }

    /// Moves to the next later view that still exists, like `back`.
    pub fn forward(&mut self, exists: impl Fn(&V) -> bool) -> Option<V> {
        while let Some(view) = self.later.pop() {
            if exists(&view) {
                if let Some(current) = self.current.replace(view.clone()) {
                    self.remember(current);
                }
                return Some(view);
            }
        }
        None
    }

    fn remember(&mut self, view: V) {
        self.earlier.push_back(view);
        if self.earlier.len() > MAX_VIEWS {
            self.earlier.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ViewHistory;

    fn visited(views: &[i32]) -> ViewHistory<i32> {
        let mut history = ViewHistory::default();
        for view in views {
            history.visit(*view);
        }
        history
    }

    fn any(_: &i32) -> bool {
        true
    }

    #[test]
    fn back_and_forward_walk_the_visited_views() {
        let mut history = visited(&[1, 2, 3]);
        assert_eq!(history.back(any), Some(2));
        assert_eq!(history.back(any), Some(1));
        assert_eq!(history.back(any), None);
        assert_eq!(history.forward(any), Some(2));
        assert_eq!(history.forward(any), Some(3));
        assert_eq!(history.forward(any), None);
    }

    #[test]
    fn visiting_the_current_view_adds_nothing() {
        let mut history = visited(&[1, 1, 1]);
        assert_eq!(history.back(any), None);
    }

    #[test]
    fn a_new_visit_drops_the_forward_views() {
        let mut history = visited(&[1, 2, 3]);
        history.back(any);
        history.visit(9);
        assert_eq!(history.forward(any), None);
        assert_eq!(history.back(any), Some(2));
    }

    #[test]
    fn revisiting_the_view_back_returned_keeps_the_forward_views() {
        let mut history = visited(&[1, 2]);
        assert_eq!(history.back(any), Some(1));
        history.visit(1);
        assert_eq!(history.forward(any), Some(2));
    }

    #[test]
    fn replace_changes_the_current_view_and_keeps_both_directions() {
        let mut history = visited(&[1, 2, 3]);
        history.back(any);
        history.replace(7);
        assert_eq!(history.forward(any), Some(3));
        assert_eq!(history.back(any), Some(7));
        assert_eq!(history.back(any), Some(1));
    }

    #[test]
    fn replace_does_not_start_an_empty_history() {
        let mut history = ViewHistory::default();
        history.replace(1);
        history.visit(2);
        assert_eq!(history.back(any), None);
    }

    #[test]
    fn views_that_no_longer_exist_are_skipped_both_ways() {
        let exists = |view: &i32| *view != 2;
        let mut history = visited(&[1, 2, 3]);
        assert_eq!(history.back(exists), Some(1));
        assert_eq!(history.forward(exists), Some(3));

        let mut history = visited(&[1, 2, 3]);
        history.back(any);
        history.back(any);
        assert_eq!(history.forward(exists), Some(3));
    }

    #[test]
    fn nothing_to_return_to_leaves_the_current_view() {
        let mut history = visited(&[1, 2]);
        assert_eq!(history.back(|view| *view != 1), None);
        history.visit(2);
        assert_eq!(history.forward(any), None);
    }

    #[test]
    fn the_history_keeps_only_the_newest_views() {
        let mut history = visited(&(0..1000).collect::<Vec<_>>());
        let mut steps = 0;
        let mut last = 999;
        while let Some(view) = history.back(any) {
            steps += 1;
            last = view;
        }
        assert_eq!(steps, super::MAX_VIEWS);
        assert_eq!(last, 999 - super::MAX_VIEWS as i32);
    }
}
