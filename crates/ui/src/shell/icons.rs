//! The icon for each domain value. The icons live with the views, so the
//! model stays free of GPUI.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon};
use gpui_kit::{App, Hsla, Styled as _};

use crate::model::data::{Agent, Assignee, ProjectStyle, Status, StatusType};
use crate::model::scope::Scope;

pub trait HasIcon {
    fn icon(&self) -> IconName;
}

impl HasIcon for Status {
    fn icon(&self) -> IconName {
        match self.kind {
            StatusType::Backlog => IconName::CircleDashed,
            StatusType::Unstarted => IconName::Circle,
            StatusType::Started => IconName::CircleDot,
            StatusType::Completed => IconName::CircleCheck,
            StatusType::Canceled => IconName::CircleX,
        }
    }
}

impl HasIcon for Agent {
    /// Lucide has no brand marks, so these are stand-ins. Claude gets the
    /// sparkles icon and every other agent gets the chip icon.
    fn icon(&self) -> IconName {
        let harness = self.harness.as_deref().unwrap_or(&self.name);
        if harness.to_lowercase().contains("claude") {
            IconName::Sparkles
        } else {
            IconName::Cpu
        }
    }
}

impl HasIcon for Assignee {
    fn icon(&self) -> IconName {
        match self {
            Self::Person(_) => IconName::User,
            Self::Agent(agent) => agent.icon(),
        }
    }
}

impl HasIcon for Scope {
    fn icon(&self) -> IconName {
        match self {
            Self::All => IconName::Inbox,
            Self::Team(_) => IconName::Users,
            Self::Project(_) => IconName::Folder,
            Self::View(_) => IconName::Funnel,
            Self::Trash => IconName::Trash,
            Self::Document(_) => IconName::FileText,
            Self::Inbox => IconName::MessageCircleQuestionMark,
            Self::Settings => IconName::Settings,
        }
    }
}

/// The colors the owner can give a project, by the name the palette uses.
/// Each one is a mid tone, so it reads on dark and light surfaces. The app
/// still darkens or lightens it where a theme needs more contrast.
pub const PROJECT_COLORS: &[(&str, &str)] = &[
    ("red", "#e5484d"),
    ("orange", "#f76b15"),
    ("amber", "#e2a336"),
    ("green", "#30a46c"),
    ("teal", "#12a594"),
    ("blue", "#0090ff"),
    ("violet", "#8e4ec6"),
    ("pink", "#d6409f"),
];

/// The preset name of a stored color, such as `teal`.
pub fn project_color_name(hex: &str) -> Option<&'static str> {
    let hex = llmtodo_core::project_style::normalize_color(hex)?;
    PROJECT_COLORS
        .iter()
        .find(|(_, h)| *h == hex)
        .map(|(name, _)| *name)
}

/// A project color as the app draws it: the stored color, with at least
/// 3:1 contrast on the sidebar, the page and menus. `None` for no color or
/// one that does not parse.
pub fn project_color(hex: Option<&str>, cx: &App) -> Option<Hsla> {
    let rgb = llmtodo_core::project_style::color_rgb(hex?)?;
    let theme = cx.theme();
    let color: Hsla = gpui_kit::rgb(rgb).into();
    Some(crate::shell::theme::ensure_contrast(
        color,
        &[theme.sidebar, theme.background, theme.popover],
        3.0,
    ))
}

/// A project's icon in its color: its own Lucide icon, or the folder that
/// every project had before. Without a color it takes `plain`, so it stays
/// monochrome.
pub fn project_icon(style: &ProjectStyle, plain: Hsla, cx: &App) -> Icon {
    let icon = match &style.icon {
        Some(name) if llmtodo_core::project_style::is_icon(name) => {
            Icon::default().path(llmtodo_core::project_style::icon_path(name))
        }
        _ => Icon::new(IconName::Folder),
    };
    icon.text_color(project_color(style.color.as_deref(), cx).unwrap_or(plain))
}

/// A swatch for the color menu: a filled circle in the color, or an empty
/// circle for no color.
pub fn swatch_icon(hex: Option<&str>, cx: &App) -> Icon {
    match project_color(hex, cx) {
        Some(color) => Icon::new(IconName::Circle).text_color(color),
        None => Icon::new(IconName::CircleDashed).text_color(cx.theme().muted_foreground),
    }
}

#[cfg(test)]
mod project_tests {
    use super::*;

    /// The core crate's icon list, which the server checks, names exactly
    /// the icons GPUI Kit bundles. A GPUI Kit upgrade that adds or drops
    /// icons fails here until the list is written again.
    #[test]
    fn project_icons_match_gpui_kit() {
        let kit: std::collections::BTreeSet<String> =
            IconName::ALL.iter().map(|i| i.path().to_string()).collect();
        let ours: std::collections::BTreeSet<String> = llmtodo_core::project_style::icon_names()
            .map(llmtodo_core::project_style::icon_path)
            .collect();
        assert_eq!(kit, ours);
    }

    #[test]
    fn preset_names_round_trip() {
        for (name, hex) in PROJECT_COLORS {
            assert_eq!(project_color_name(hex), Some(*name));
            assert_eq!(
                llmtodo_core::project_style::normalize_color(hex).as_deref(),
                Some(*hex)
            );
        }
        assert_eq!(project_color_name("#123456"), None);
    }

    /// Every preset keeps 3:1 on the sidebar in every theme and mode, and
    /// the eight stay apart from each other.
    #[gpui_kit::test]
    fn presets_read_in_every_theme(cx: &mut gpui_kit::TestAppContext) {
        use crate::shell::theme::{self, ColorTheme};
        use gpui_kit::component::ThemeMode;
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            for name in ColorTheme::ALL {
                for mode in [ThemeMode::Dark, ThemeMode::Light] {
                    theme::set_theme(name, mode, cx);
                    let sidebar = cx.theme().sidebar;
                    let drawn: Vec<Hsla> = PROJECT_COLORS
                        .iter()
                        .map(|(_, hex)| project_color(Some(hex), cx).unwrap())
                        .collect();
                    for (color, (preset, _)) in drawn.iter().zip(PROJECT_COLORS) {
                        let ratio = theme::contrast(*color, sidebar);
                        assert!(ratio >= 3.0, "{preset} in {name:?} {mode:?}: {ratio}");
                    }
                    for (i, a) in drawn.iter().enumerate() {
                        for b in &drawn[i + 1..] {
                            assert_ne!(a, b, "{name:?} {mode:?}");
                        }
                    }
                }
            }
        });
    }
}
