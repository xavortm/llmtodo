//! The app's color themes. A theme is a named set of color tokens with a
//! light and a dark mode, and the dark mode switch picks between them.
//!
//! `Default` is monochrome. Owner rule: no accent color except for tags and
//! the green check on done issues. GPUI Kit's dark and light themes use blue
//! for selection, focus rings, links and primary buttons, so `Default` maps
//! those tokens onto shades of the foreground. The other themes keep their
//! own accents. Every theme gets the same contrast rules.

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Theme, ThemeConfig, ThemeMode, ThemeRegistry, ThemeSet,
};
use gpui_kit::{App, FontWeight, Hsla, Rgba, StyleRefinement, Styled as _};

pub const UI_FONT: &str = "IBM Plex Sans";
pub const CODE_FONT: &str = "Lilex";

/// Loads the bundled fonts. See `assets/fonts/NOTICE.md`.
pub fn load_fonts(cx: &mut App) {
    let fonts: Vec<std::borrow::Cow<'static, [u8]>> = vec![
        include_bytes!("../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../assets/fonts/ibm-plex-sans/IBMPlexSans-Italic.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBold.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBoldItalic.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../assets/fonts/lilex/Lilex-Regular.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../assets/fonts/lilex/Lilex-Italic.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../assets/fonts/lilex/Lilex-Bold.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../assets/fonts/lilex/Lilex-BoldItalic.ttf")
            .as_slice()
            .into(),
    ];
    if let Err(err) = cx.text_system().add_fonts(fonts) {
        eprintln!("llmtodo: could not load bundled fonts: {err}");
    }
}

/// UI font size at 100% zoom. Zed's default: `.ZedSans` is IBM Plex Sans
/// at 16px.
const UI_FONT_SIZE: f32 = 16.;
/// Code font size at 100% zoom. Zed's `.ZedMono` is Lilex at 15px.
const CODE_FONT_SIZE: f32 = 15.;

/// The zoom levels Cmd-= and Cmd-- step through, as browsers do.
pub const ZOOM_STEPS: &[f32] = &[0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// The UI scale. GPUI Kit sets the window's rem size from the theme's font
/// size, and the app sizes almost everything in rems, so zoom scales the
/// font sizes.
pub struct Zoom(f32);

impl gpui_kit::Global for Zoom {}

/// The current zoom level, 1.0 for actual size.
pub fn zoom(cx: &App) -> f32 {
    cx.try_global::<Zoom>().map_or(1.0, |z| z.0)
}

/// The next zoom step above or below `level`. A level between steps, from
/// a hand-edited settings file, moves to the nearest step in that
/// direction.
pub fn zoom_step(level: f32, up: bool) -> f32 {
    const EPS: f32 = 0.001;
    if up {
        ZOOM_STEPS
            .iter()
            .copied()
            .find(|s| *s > level + EPS)
            .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1])
    } else {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|s| *s < level - EPS)
            .unwrap_or(ZOOM_STEPS[0])
    }
}

/// Sets the zoom level, clamped to the steps' range, and redraws.
pub fn set_zoom(level: f32, cx: &mut App) {
    let level = if level.is_finite() { level } else { 1.0 };
    let level = level.clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
    cx.set_global(Zoom(level));
    apply_font_sizes(Theme::global_mut(cx), level);
    Theme::sync_base(cx);
    cx.refresh_windows();
}

fn apply_font_sizes(theme: &mut Theme, level: f32) {
    theme.font_size = gpui_kit::px(UI_FONT_SIZE * level);
    theme.mono_font_size = gpui_kit::px(CODE_FONT_SIZE * level);
}

/// A built-in color theme. Each one has a light and a dark mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorTheme {
    /// The monochrome theme, built on GPUI Kit's own colors.
    #[default]
    Default,
    /// A warm theme. See `assets/themes/NOTICE.md`.
    Gruvbox,
    /// Apple's system colors, Xcode's code colors and the system font.
    MacOs,
}

impl ColorTheme {
    /// Every theme, in the order the Settings picker shows them.
    pub const ALL: [Self; 3] = [Self::Default, Self::Gruvbox, Self::MacOs];

    /// The name in `ui.json` and in the `:theme` command.
    pub fn id(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Gruvbox => "gruvbox",
            Self::MacOs => "macos",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Gruvbox => "Gruvbox",
            Self::MacOs => "macOS",
        }
    }

    /// The theme after this one, wrapping at the end.
    pub fn next(self, forward: bool) -> Self {
        let n = Self::ALL.len();
        let ix = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[if forward {
            (ix + 1) % n
        } else {
            (ix + n - 1) % n
        }]
    }

    fn config(self, mode: ThemeMode, cx: &App) -> Rc<ThemeConfig> {
        match self {
            Self::Default => {
                let registry = ThemeRegistry::global(cx);
                if mode.is_dark() {
                    registry.default_dark_theme().clone()
                } else {
                    registry.default_light_theme().clone()
                }
            }
            Self::Gruvbox => GRUVBOX.with(|set| pick(set, mode)),
            Self::MacOs => MACOS.with(|set| pick(set, mode)),
        }
    }

    /// The UI font. The macOS theme uses the system font, SF Pro. Code
    /// keeps `CODE_FONT` in every theme.
    pub fn ui_font(self) -> &'static str {
        match self {
            Self::MacOs => ".SystemUIFont",
            Self::Default | Self::Gruvbox => UI_FONT,
        }
    }
}

thread_local! {
    static GRUVBOX: Vec<Rc<ThemeConfig>> =
        load_set(include_str!("../../assets/themes/gruvbox.json"));
    static MACOS: Vec<Rc<ThemeConfig>> =
        load_set(include_str!("../../assets/themes/macos.json"));
}

fn load_set(json: &str) -> Vec<Rc<ThemeConfig>> {
    serde_json::from_str::<ThemeSet>(json)
        .expect("a bundled theme file parses")
        .themes
        .into_iter()
        .map(Rc::new)
        .collect()
}

fn pick(set: &[Rc<ThemeConfig>], mode: ThemeMode) -> Rc<ThemeConfig> {
    set.iter()
        .find(|c| c.mode.is_dark() == mode.is_dark())
        .expect("a bundled theme has both modes")
        .clone()
}

/// The theme the app shows now.
struct CurrentTheme(ColorTheme);

impl gpui_kit::Global for CurrentTheme {}

pub fn color_theme(cx: &App) -> ColorTheme {
    cx.try_global::<CurrentTheme>().map_or_default(|t| t.0)
}

/// Switches between light and dark and keeps the current theme.
pub fn set_mode(mode: ThemeMode, cx: &mut App) {
    set_theme(color_theme(cx), mode, cx);
}

/// Switches the theme and keeps the current mode.
pub fn set_color_theme(name: ColorTheme, cx: &mut App) {
    let mode = cx.theme().mode;
    set_theme(name, mode, cx);
}

pub fn set_theme(name: ColorTheme, mode: ThemeMode, cx: &mut App) {
    let level = zoom(cx);
    cx.set_global(CurrentTheme(name));
    if !cx.has_global::<Theme>() {
        Theme::change(mode, None, cx);
    }
    let (light, dark) = (
        name.config(ThemeMode::Light, cx),
        name.config(ThemeMode::Dark, cx),
    );
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    // `change` loads the mode's theme even when the mode stays the same.
    Theme::change(mode, None, cx);
    let theme = Theme::global_mut(cx);
    // `main::init` loads the fonts.
    theme.font_family = name.ui_font().into();
    theme.mono_font_family = CODE_FONT.into();
    apply_font_sizes(theme, level);
    style_colors(theme, name);

    // Edits made through `global_mut` reach GPUI Kit's base layer only
    // after this call.
    Theme::sync_base(cx);
    install_text_style(cx);
}

/// `name` in `mode`, with the app's rules applied, without switching to
/// it. The Settings picker draws its previews from this.
pub fn preview(name: ColorTheme, mode: ThemeMode, cx: &App) -> Theme {
    let mut theme = Theme::global(cx).clone();
    theme.apply_config(&name.config(mode, cx));
    theme.mode = mode;
    style_colors(&mut theme, name);
    theme
}

/// The app's color rules on top of a theme file's colors.
fn style_colors(theme: &mut Theme, name: ColorTheme) {
    if name == ColorTheme::Default {
        monochrome(theme);
    }

    // Owner rule: text keeps at least 4.5:1 contrast. Muted text sits on the
    // page, the sidebar, popovers and group headers, so it must pass on the
    // lightest of them. Aim for 5:1, because hover and selection fills sit
    // between the text and the page.
    let surfaces = [theme.background, theme.sidebar, theme.popover, theme.muted];
    theme.foreground = ensure_contrast(theme.foreground, &surfaces, 4.5);
    theme.muted_foreground = ensure_contrast(theme.muted_foreground, &surfaces, 5.0);
    theme.danger = ensure_contrast(theme.danger, &surfaces, 4.5);
    theme.primary_foreground = text_on(theme.primary, theme.primary_foreground, theme);
    theme.button_primary_foreground =
        text_on(theme.button_primary, theme.button_primary_foreground, theme);
}

fn monochrome(theme: &mut Theme) {
    let fg = theme.foreground;
    let bg = theme.background;

    theme.primary = fg;
    theme.primary_hover = fg.opacity(0.9);
    theme.primary_active = fg.opacity(0.8);
    theme.primary_foreground = bg;
    theme.button_primary = fg;
    theme.button_primary_hover = fg.opacity(0.9);
    theme.button_primary_active = fg.opacity(0.8);
    theme.button_primary_foreground = bg;

    theme.ring = fg.opacity(0.45);
    theme.caret = fg;
    theme.selection = fg.opacity(0.2);
    theme.link = fg;
    theme.link_hover = fg;
    theme.link_active = fg;

    theme.accent = fg.opacity(0.06);
    theme.accent_foreground = fg;
    theme.list_hover = fg.opacity(0.04);
    theme.list_active = fg.opacity(0.11);
    theme.list_active_border = fg.opacity(0.6);
    theme.table_hover = fg.opacity(0.04);
    theme.table_active = fg.opacity(0.08);
    theme.table_active_border = fg.opacity(0.6);
    theme.sidebar_accent = fg.opacity(0.08);
    theme.sidebar_accent_foreground = fg;
    theme.sidebar_primary = fg;
    theme.sidebar_primary_foreground = bg;
    theme.tab_active_foreground = fg;
    theme.progress_bar = fg;
    theme.slider_bar = fg;
    theme.slider_thumb = fg;
    theme.switch = fg;
}

/// Text for a solid fill: the theme's own choice when it is readable, else
/// the page or text color, else black or white.
fn text_on(fill: Hsla, own: Hsla, theme: &Theme) -> Hsla {
    let fill = blend(fill, theme.background);
    [own, theme.background, theme.foreground]
        .into_iter()
        .map(|c| blend(c, fill))
        .find(|c| contrast(*c, fill) >= 4.5)
        .unwrap_or_else(|| black_or_white(fill))
}

fn black_or_white(fill: Hsla) -> Hsla {
    if luminance(fill) > 0.18 {
        gpui_kit::black()
    } else {
        gpui_kit::white()
    }
}

/// Markdown body text a little softer than headings, so headings stand out
/// and long text tires the eye less. Both stay well above 4.5:1.
fn install_text_style(cx: &mut App) {
    let theme = cx.theme();
    let heading = theme.foreground;
    let body = ensure_contrast(theme.foreground.opacity(0.82), &[theme.background], 7.0);
    let mut table_head = StyleRefinement::default();
    table_head = table_head
        .bg(theme.table_head)
        .text_color(theme.table_head_foreground);
    let style = gpui_kit::base::TextViewStyle::default()
        .with_foreground(body)
        .with_muted_foreground(theme.muted_foreground)
        .with_link(theme.link)
        .with_selection(theme.selection)
        .with_code_background(theme.muted)
        .with_border(theme.border)
        .with_table_head(table_head)
        // Code in the preview matches the body text size, which is
        // `text_sm`, so code and prose line up.
        .with_code_block(
            StyleRefinement::default()
                .text_size(gpui_kit::rems(0.875))
                .rounded(theme.radius),
        )
        .with_table(StyleRefinement::default().rounded(theme.radius))
        .with_table_row_hover(theme.table_hover)
        .with_inline_code(gpui_kit::HighlightStyle {
            background_color: Some(theme.accent),
            ..Default::default()
        })
        .with_dark(theme.is_dark())
        .with_paragraph_gap(gpui_kit::rems(0.75))
        .with_heading(move |level| {
            StyleRefinement::default()
                .text_color(heading)
                .font_weight(if level <= 2 {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
        });
    // Keep the installed code-block highlighter, replace only the style.
    // Text views take this color instead of their container's.
    gpui_kit::base::TextViewDefaults::global(cx)
        .with_style(style)
        .with_inherit_text_color(false)
        .install(cx);
}

/// The fill of the selected row in a list or the sidebar. The fill is the
/// only selection marker, because the app never draws a left border. A
/// list without focus shows a fainter fill.
pub fn selection_bg(theme: &Theme, focused: bool) -> Hsla {
    if focused {
        theme.list_active
    } else {
        theme.list_active.opacity(0.6)
    }
}

/// The fill of a selected row under the pointer. It sits between
/// `selection_bg` and `row_pressed`, so hover and press both show.
pub fn selection_hover(theme: &Theme) -> Hsla {
    theme.foreground.opacity(0.14)
}

/// The fill of a list or sidebar row while the mouse is held.
pub fn row_pressed(theme: &Theme) -> Hsla {
    theme.foreground.opacity(0.18)
}

/// The fill and text of the `Locked` badge on a task row. It is solid, so it
/// stands out on any row fill.
pub fn locked_badge(theme: &Theme) -> (Hsla, Hsla) {
    let fill = theme.warning;
    (fill, ensure_contrast(black_or_white(fill), &[fill], 4.5))
}

fn luminance(color: Hsla) -> f32 {
    let rgb: Rgba = color.into();
    let channel = |v: f32| {
        if v <= 0.039_28 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(rgb.r) + 0.7152 * channel(rgb.g) + 0.0722 * channel(rgb.b)
}

/// WCAG contrast ratio of two opaque colors, from 1 to 21.
pub fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Moves `color`'s lightness away from the surfaces until it reaches `min`
/// contrast on every one of them. A translucent color is made opaque over
/// the first surface first, because that is how it is seen.
pub fn ensure_contrast(color: Hsla, surfaces: &[Hsla], min: f32) -> Hsla {
    let base = surfaces[0];
    let mut c = if color.a < 1.0 {
        blend(color, base)
    } else {
        color
    };
    let dark = luminance(base) < 0.5;
    for _ in 0..100 {
        if surfaces.iter().all(|s| contrast(c, blend(*s, base)) >= min) {
            break;
        }
        c.l = (c.l + if dark { 0.01 } else { -0.01 }).clamp(0., 1.);
    }
    c
}

/// `top` drawn over the opaque `bottom`.
fn blend(top: Hsla, bottom: Hsla) -> Hsla {
    if top.a >= 1.0 {
        return top;
    }
    let (t, b): (Rgba, Rgba) = (top.into(), bottom.into());
    let mix = |x: f32, y: f32| x * top.a + y * (1. - top.a);
    Rgba {
        r: mix(t.r, b.r),
        g: mix(t.g, b.g),
        b: mix(t.b, b.b),
        a: 1.,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_theme() -> impl Iterator<Item = (ColorTheme, ThemeMode)> {
        ColorTheme::ALL.into_iter().flat_map(|name| {
            [ThemeMode::Dark, ThemeMode::Light]
                .into_iter()
                .map(move |mode| (name, mode))
        })
    }

    #[gpui_kit::test]
    fn the_locked_badge_stays_readable(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        for (name, mode) in every_theme() {
            cx.update(|cx| {
                set_theme(name, mode, cx);
                let t = cx.theme();
                let (fill, text) = locked_badge(t);
                assert!(contrast(text, fill) >= 4.5, "{name:?} {mode:?} badge text");
            });
        }
    }

    #[gpui_kit::test]
    fn text_tokens_keep_4_5_to_1_in_every_theme_and_mode(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        for (name, mode) in every_theme() {
            cx.update(|cx| {
                set_theme(name, mode, cx);
                let t = cx.theme();
                for surface in [t.background, t.sidebar, t.popover] {
                    assert!(
                        contrast(t.foreground, surface) >= 4.5,
                        "{name:?} {mode:?} foreground"
                    );
                    assert!(
                        contrast(t.muted_foreground, surface) >= 4.5,
                        "{name:?} {mode:?} muted text: {}",
                        contrast(t.muted_foreground, surface)
                    );
                }
                for color in [t.red, t.green, t.blue, t.yellow, t.magenta, t.cyan] {
                    let label = ensure_contrast(color, &[t.background], 5.0);
                    assert!(
                        contrast(label, t.background) >= 4.5,
                        "{name:?} {mode:?} label color"
                    );
                }
            });
        }
    }

    #[gpui_kit::test]
    fn primary_buttons_keep_readable_text(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        for (name, mode) in every_theme() {
            cx.update(|cx| {
                set_theme(name, mode, cx);
                let t = cx.theme();
                let fill = blend(t.primary, t.background);
                assert!(
                    contrast(t.primary_foreground, fill) >= 4.5,
                    "{name:?} {mode:?} primary text"
                );
                let fill = blend(t.button_primary, t.background);
                assert!(
                    contrast(t.button_primary_foreground, fill) >= 4.5,
                    "{name:?} {mode:?} button text"
                );
            });
        }
    }

    #[gpui_kit::test]
    fn macos_buttons_keep_white_text(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            cx.update(|cx| {
                set_theme(ColorTheme::MacOs, mode, cx);
                let t = cx.theme();
                assert_eq!(t.button_primary_foreground, gpui_kit::white(), "{mode:?}");
                assert_eq!(t.font_family.as_ref(), ".SystemUIFont");
            });
        }
    }

    #[gpui_kit::test]
    fn a_preview_shows_the_colors_the_theme_applies(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        for (name, mode) in every_theme() {
            cx.update(|cx| {
                set_theme(ColorTheme::Default, ThemeMode::Dark, cx);
                let preview = preview(name, mode, cx);
                set_theme(name, mode, cx);
                let t = cx.theme();
                assert_eq!(preview.background, t.background, "{name:?} {mode:?}");
                assert_eq!(preview.foreground, t.foreground, "{name:?} {mode:?}");
                assert_eq!(preview.primary, t.primary, "{name:?} {mode:?}");
                assert_eq!(color_theme(cx), name);
                assert_eq!(t.is_dark(), mode.is_dark());
            });
        }
    }

    #[gpui_kit::test]
    fn the_mode_switch_keeps_the_theme(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            set_theme(ColorTheme::Gruvbox, ThemeMode::Dark, cx);
            set_mode(ThemeMode::Light, cx);
            assert_eq!(color_theme(cx), ColorTheme::Gruvbox);
            assert_eq!(cx.theme().theme_name().as_ref(), "Gruvbox Light");
            set_color_theme(ColorTheme::Default, cx);
            assert!(!cx.theme().is_dark(), "the theme switch keeps the mode");
        });
    }

    #[test]
    fn theme_ids_round_trip() {
        for name in ColorTheme::ALL {
            assert_eq!(ColorTheme::from_id(name.id()), Some(name));
        }
        assert_eq!(ColorTheme::from_id("nope"), None);
        assert_eq!(ColorTheme::Default.next(true), ColorTheme::Gruvbox);
        assert_eq!(ColorTheme::Default.next(false), ColorTheme::MacOs);
        assert_eq!(ColorTheme::Gruvbox.next(true), ColorTheme::MacOs);
        assert_eq!(ColorTheme::MacOs.next(true), ColorTheme::Default);
    }
}
