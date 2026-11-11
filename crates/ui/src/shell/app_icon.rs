//! The HardHat app icon.
//!
//! A `.app` bundle gets its icon from `HardHat.icns` through `Info.plist`
//! (see `scripts/bundle-macos`). A bare binary, as from `cargo run`, has no
//! bundle, so the Dock would show a generic icon. GPUI has no icon API, so
//! this sets it through AppKit at startup.

#[cfg(target_os = "macos")]
pub fn set_dock_icon() {
    use objc2::{AllocAnyThread as _, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    const PNG: &[u8] = include_bytes!("../../assets/icon/HardHat-1024.png");

    // GPUI calls `run`'s callback on the main thread.
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let data = NSData::with_bytes(PNG);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    unsafe { app.setApplicationIconImage(Some(&image)) };
}

#[cfg(not(target_os = "macos"))]
pub fn set_dock_icon() {}

/// The Dock badge text for `count` open owner questions. Zero shows no badge.
pub fn badge_label(count: usize) -> Option<String> {
    (count > 0).then(|| count.to_string())
}

/// The count the Dock badge shows now, so a store change that leaves the
/// count alone makes no AppKit call.
static BADGE_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Shows `count` open owner questions on the Dock icon, or no badge at zero.
pub fn set_dock_badge(count: usize) {
    use std::sync::atomic::Ordering;
    if BADGE_COUNT.swap(count, Ordering::Relaxed) == count {
        return;
    }
    show_badge(badge_label(count).as_deref());
}

#[cfg(target_os = "macos")]
fn show_badge(label: Option<&str>) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;

    // Tests run off the main thread, and AppKit needs it.
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let label = label.map(NSString::from_str);
    let tile = NSApplication::sharedApplication(mtm).dockTile();
    tile.setBadgeLabel(label.as_deref());
}

#[cfg(not(target_os = "macos"))]
fn show_badge(_label: Option<&str>) {}

/// Whether a change in open owner questions should bounce the Dock icon.
/// `seen` is the count before the change, or `None` until the first load.
pub fn should_bounce(seen: Option<usize>, count: usize, app_active: bool) -> bool {
    !app_active && seen.is_some_and(|seen| count > seen)
}

/// Bounces the Dock icon once. An informational request does not keep
/// bouncing, because agents can ask many questions in a row.
#[cfg(target_os = "macos")]
pub fn bounce_dock() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSRequestUserAttentionType};

    // Tests run off the main thread, and AppKit needs it.
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    NSApplication::sharedApplication(mtm)
        .requestUserAttention(NSRequestUserAttentionType::InformationalRequest);
}

#[cfg(not(target_os = "macos"))]
pub fn bounce_dock() {}

#[cfg(test)]
mod tests {
    use super::{badge_label, should_bounce};

    #[test]
    fn badge_hides_at_zero_and_shows_the_count() {
        assert_eq!(badge_label(0), None);
        assert_eq!(badge_label(1).as_deref(), Some("1"));
        assert_eq!(badge_label(12).as_deref(), Some("12"));
    }

    #[test]
    fn a_new_question_bounces_only_in_the_background() {
        assert!(should_bounce(Some(2), 3, false));
        assert!(should_bounce(Some(0), 1, false));
        assert!(!should_bounce(Some(2), 3, true));
    }

    #[test]
    fn the_first_load_never_bounces() {
        assert!(!should_bounce(None, 5, false));
    }

    #[test]
    fn a_count_that_falls_or_stays_never_bounces() {
        assert!(!should_bounce(Some(3), 2, false));
        assert!(!should_bounce(Some(3), 3, false));
        assert!(!should_bounce(Some(1), 0, false));
    }
}
