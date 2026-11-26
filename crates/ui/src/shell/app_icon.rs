//! The HardHat app icon, and a preview build's gray one (LLM-592).
//!
//! A `.app` bundle gets its icon from `HardHat.icns` through `Info.plist`
//! (see `scripts/bundle-macos`). A bare binary, as from `cargo run`, has no
//! bundle, so the Dock would show a generic icon. GPUI has no icon API, so
//! this sets it through AppKit at startup. Inside a bundle it does nothing:
//! decoding the 1024 px PNG would only slow the start (LLM-344).

use std::path::Path;

/// The bundle icon `scripts/bundle-macos` copies next to `Info.plist`.
const BUNDLE_ICON: &str = "HardHat.icns";

/// Whether `exe` runs from a `.app` bundle that has its own icon, so the
/// Dock already shows it.
pub fn bundle_has_icon(exe: &Path) -> bool {
    let Some(contents) = exe
        .parent()
        .filter(|dir| dir.file_name().is_some_and(|n| n == "MacOS"))
        .and_then(Path::parent)
    else {
        return false;
    };
    contents.file_name().is_some_and(|n| n == "Contents")
        && contents
            .parent()
            .and_then(Path::extension)
            .is_some_and(|ext| ext == "app")
        && contents.join("Resources").join(BUNDLE_ICON).is_file()
}

#[cfg(target_os = "macos")]
pub fn set_dock_icon() {
    use objc2::{AllocAnyThread as _, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    const PNG: &[u8] = include_bytes!("../../assets/icon/HardHat-1024.png");

    if std::env::current_exe().is_ok_and(|exe| bundle_has_icon(&exe)) {
        return;
    }

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

/// The app icon in gray, as PNG bytes, for a preview build's Dock icon
/// (LLM-592). Transparency stays, so the shape still reads as the app.
pub fn gray_icon_png(png: &[u8]) -> Option<Vec<u8>> {
    let mut img = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    for px in img.pixels_mut() {
        let [r, g, b, a] = px.0;
        let luma = (0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)) as u8;
        // A little darker than plain luma, so the yellow key stands out.
        let gray = (f32::from(luma) * 0.8) as u8;
        px.0 = [gray, gray, gray, a];
    }
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Sets a preview build's Dock icon: the app icon in gray, with `key` in
/// yellow across its lower part. It always replaces the bundle icon, since
/// a preview must never pass for the installed app.
// `lockFocus` is deprecated, but it still draws, and its block-based
// replacement would need the block2 crate for this one image.
#[cfg(target_os = "macos")]
#[allow(deprecated)]
pub fn set_preview_dock_icon(key: &str) {
    use objc2::runtime::AnyObject;
    use objc2::{AllocAnyThread as _, MainThreadMarker};
    use objc2_app_kit::{
        NSApplication, NSBezierPath, NSColor, NSFont, NSFontAttributeName,
        NSForegroundColorAttributeName, NSImage, NSStringDrawing as _,
    };
    use objc2_foundation::{NSData, NSDictionary, NSPoint, NSRect, NSSize, NSString};

    const PNG: &[u8] = include_bytes!("../../assets/icon/HardHat-1024.png");

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(gray) = gray_icon_png(PNG) else {
        return;
    };
    let data = NSData::with_bytes(&gray);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };
    // Draw in points of a 1024 square, whatever size the PNG says.
    let side = 1024.0;
    image.setSize(NSSize::new(side, side));
    image.lockFocus();
    // A dark band behind the key keeps the yellow readable on any gray.
    let band = NSRect::new(NSPoint::new(48.0, 96.0), NSSize::new(side - 96.0, 260.0));
    NSColor::colorWithSRGBRed_green_blue_alpha(0.1, 0.1, 0.1, 0.85).setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(band, 56.0, 56.0).fill();
    let yellow = NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.82, 0.1, 1.0);
    let text = NSString::from_str(key);
    let keys = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
    let attrs_at = |points: f64| {
        let font = NSFont::boldSystemFontOfSize(points);
        let values: [&AnyObject; 2] = [&font, &yellow];
        NSDictionary::from_slices(&keys, &values)
    };
    // Start large, then shrink until the key fits the band.
    let room = band.size.width - 64.0;
    let mut attrs = attrs_at(190.0);
    let mut size = unsafe { text.sizeWithAttributes(Some(&attrs)) };
    if size.width > room {
        attrs = attrs_at(190.0 * room / size.width);
        size = unsafe { text.sizeWithAttributes(Some(&attrs)) };
    }
    let at = NSPoint::new(
        (side - size.width) / 2.0,
        band.origin.y + (band.size.height - size.height) / 2.0,
    );
    unsafe { text.drawAtPoint_withAttributes(at, Some(&attrs)) };
    image.unlockFocus();
    let app = NSApplication::sharedApplication(mtm);
    unsafe { app.setApplicationIconImage(Some(&image)) };
}

#[cfg(not(target_os = "macos"))]
pub fn set_preview_dock_icon(_key: &str) {}

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
    use super::{BUNDLE_ICON, badge_label, bundle_has_icon, gray_icon_png, should_bounce};

    #[test]
    fn the_preview_icon_is_gray_and_keeps_its_shape() {
        let png = include_bytes!("../../assets/icon/HardHat-1024.png");
        let before = image::load_from_memory(png).unwrap().into_rgba8();
        let after = image::load_from_memory(&gray_icon_png(png).unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(before.dimensions(), after.dimensions());
        for (a, b) in before.pixels().zip(after.pixels()) {
            let [r, g, bl, alpha] = b.0;
            assert!(r == g && g == bl, "gray");
            assert_eq!(alpha, a.0[3], "the same shape");
        }
    }

    #[test]
    fn only_a_bundle_with_its_icon_skips_the_dock_icon() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("llmtodo.app/Contents");
        let exe = app.join("MacOS/llmtodo-app");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::create_dir_all(app.join("Resources")).unwrap();
        assert!(!bundle_has_icon(&exe), "no icon in the bundle yet");
        std::fs::write(app.join("Resources").join(BUNDLE_ICON), "").unwrap();
        assert!(bundle_has_icon(&exe));
        let cargo = dir.path().join("target/debug/llmtodo-app");
        assert!(!bundle_has_icon(&cargo), "cargo run has no bundle");
    }

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
