//! Where the panes sit: the sidebar's visibility and the split between the
//! list and the task view. The workspace owns it and updates it in its own
//! update pass.

use gpui_kit::component::resizable::ResizableState;
use gpui_kit::{Entity, Pixels};

pub struct Layout {
    pub split_side: Entity<ResizableState>,
    pub split_stacked: Entity<ResizableState>,
    /// Saved task view sizes: beside the list, and under it.
    pub split_sizes: (Option<f32>, Option<f32>),
    /// The room beside the list and the task view width the smart split
    /// last gave it. Only used until the owner drags the divider.
    pub auto_split: Option<(Pixels, Pixels)>,
    /// The owner's choice: show the sidebar or not. Only Cmd-B, the title
    /// bar button and `:sidebar` change it, and it is saved.
    pub sidebar_open: bool,
    /// The window is narrower than the auto-hide width, so the sidebar
    /// hides on its own. `sidebar_open` keeps the owner's choice meanwhile.
    pub narrow: bool,
    /// The owner showed the sidebar by hand while the window was narrow.
    /// It stays until the window widens again.
    pub sidebar_forced: bool,
}

impl Layout {
    pub fn new(
        split_side: Entity<ResizableState>,
        split_stacked: Entity<ResizableState>,
        narrow: bool,
    ) -> Self {
        Self {
            split_side,
            split_stacked,
            split_sizes: (None, None),
            auto_split: None,
            sidebar_open: false,
            narrow,
            sidebar_forced: false,
        }
    }

    /// Whether the sidebar is on screen: the owner wants it, and the window
    /// is wide enough or the owner showed it by hand.
    pub fn sidebar_shown(&self) -> bool {
        self.sidebar_open && (!self.narrow || self.sidebar_forced)
    }

    /// The owner shows or hides the sidebar by hand. Showing it in a narrow
    /// window overrides the auto-hide until the window widens.
    pub fn set_sidebar_open(&mut self, open: bool) {
        self.sidebar_open = open;
        self.sidebar_forced = open && self.narrow;
    }

    /// Notes the window's width. It never changes `sidebar_open`, the
    /// owner's choice. Returns whether `narrow` changed.
    pub fn set_narrow(&mut self, narrow: bool) -> bool {
        if narrow == self.narrow {
            return false;
        }
        self.narrow = narrow;
        if !narrow {
            self.sidebar_forced = false;
        }
        true
    }
}
