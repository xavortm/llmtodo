//! Counts `render` calls per view type, for tests that check which views
//! draw again. Each `#[gpui::test]` runs on its own thread, so a
//! thread-local count keeps tests apart.

use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static COUNTS: RefCell<HashMap<&'static str, usize>> = RefCell::default();
}

/// One more render of `T`. Each counted view calls this first in `render`.
pub fn bump<T>() {
    let name = std::any::type_name::<T>();
    COUNTS.with(|c| *c.borrow_mut().entry(name).or_default() += 1);
}

/// How often `T` rendered since the last `reset`.
pub fn of<T>() -> usize {
    let name = std::any::type_name::<T>();
    COUNTS.with(|c| c.borrow().get(name).copied().unwrap_or(0))
}

pub fn reset() {
    COUNTS.with(|c| c.borrow_mut().clear());
}
