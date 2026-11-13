//! The window around the features: the workspace, the title and status
//! bars, the sidebar, the palette and shared widgets.

pub mod app_icon;
pub mod app_menus;
pub mod icons;
pub mod issue_menu;
pub mod layout;
pub mod mascot;
pub mod palette;
pub mod sidebar;
pub mod split;
pub mod theme;
pub mod tooltip;
pub mod unzoomed;
pub mod view_events;
pub mod view_history;
pub mod widgets;
pub mod workspace;

#[cfg(test)]
mod app_menus_tests;
#[cfg(test)]
mod cache_tests;
#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod perf_tests;
#[cfg(test)]
mod sidebar_tests;
#[cfg(test)]
pub mod tests;
#[cfg(test)]
mod title_bar_tests;
#[cfg(test)]
mod trackers_tests;
#[cfg(test)]
mod update_tests;
