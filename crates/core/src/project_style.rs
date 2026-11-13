//! A project's icon and color. The server checks both, and the app draws
//! them, so the one list of icon names lives here.

const ICONS: &str = include_str!("project_icons.txt");

/// Every icon name a project can use, in sorted order. Each name is a
/// Lucide name, such as `rocket`, and the app draws it from
/// `icons/<name>.svg`.
pub fn icon_names() -> impl Iterator<Item = &'static str> {
    ICONS
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

/// Whether the app can draw this icon name.
pub fn is_icon(name: &str) -> bool {
    icon_names().any(|n| n == name)
}

/// The asset path of an icon name, as GPUI Kit's asset source names it.
pub fn icon_path(name: &str) -> String {
    format!("icons/{name}.svg")
}

/// Reads a color such as `#5D9AA9` or `5d9aa9`, and returns it as
/// `#5d9aa9`. Anything else is `None`.
pub fn normalize_color(color: &str) -> Option<String> {
    let hex = color.trim();
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    (hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex.to_ascii_lowercase()))
}

/// The RGB value of a color that `normalize_color` accepts.
pub fn color_rgb(color: &str) -> Option<u32> {
    let hex = normalize_color(color)?;
    u32::from_str_radix(&hex[1..], 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_list_is_sorted_and_known() {
        let names: Vec<_> = icon_names().collect();
        assert!(names.len() > 1800);
        assert!(names.windows(2).all(|w| w[0] < w[1]));
        assert!(is_icon("rocket"));
        assert!(is_icon("folder"));
        assert!(!is_icon("Rocket"));
        assert!(!is_icon("no-such-icon"));
        assert!(!is_icon(""));
    }

    #[test]
    fn colors_normalize() {
        assert_eq!(normalize_color("#5D9AA9").as_deref(), Some("#5d9aa9"));
        assert_eq!(normalize_color(" 5d9aa9 ").as_deref(), Some("#5d9aa9"));
        assert_eq!(normalize_color("#5d9aa"), None);
        assert_eq!(normalize_color("red"), None);
        assert_eq!(normalize_color("#gggggg"), None);
        assert_eq!(color_rgb("#ff0000"), Some(0xff0000));
    }
}
