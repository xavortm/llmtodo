//! Monochrome theme. Owner rule: no accent color except for tags and the
//! green check on done issues. GPUI Kit's dark and light themes use blue for
//! selection, focus rings, links and primary buttons. This maps those tokens
//! onto shades of the foreground.

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};
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

pub fn set_mode(mode: ThemeMode, cx: &mut App) {
    let level = zoom(cx);
    Theme::change(mode, None, cx);
    let theme = Theme::global_mut(cx);
    // `main::init` loads the fonts.
    theme.font_family = UI_FONT.into();
    theme.mono_font_family = CODE_FONT.into();
    apply_font_sizes(theme, level);
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

    // Owner rule: text keeps at least 4.5:1 contrast. Muted text sits on the
    // page, the sidebar, popovers and group headers, so it must pass on the
    // lightest of them. Aim for 5:1, because hover and selection fills sit
    // between the text and the page.
    let surfaces = [theme.background, theme.sidebar, theme.popover, theme.muted];
    theme.muted_foreground = ensure_contrast(theme.muted_foreground, &surfaces, 5.0);
    theme.danger = ensure_contrast(theme.danger, &surfaces, 4.5);

    // Edits made through `global_mut` reach GPUI Kit's base layer only
    // after this call.
    Theme::sync_base(cx);
    install_text_style(cx);
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
    let page = if luminance(fill) > 0.18 {
        gpui_kit::black()
    } else {
        gpui_kit::white()
    };
    (fill, ensure_contrast(page, &[fill], 4.5))
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

    #[gpui_kit::test]
    fn the_locked_badge_stays_readable(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            cx.update(|cx| {
                set_mode(mode, cx);
                let t = cx.theme();
                let (fill, text) = locked_badge(t);
                assert!(contrast(text, fill) >= 4.5, "{mode:?} badge text");
            });
        }
    }

    #[gpui_kit::test]
    fn text_tokens_keep_4_5_to_1_in_both_modes(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            cx.update(|cx| {
                set_mode(mode, cx);
                let t = cx.theme();
                for surface in [t.background, t.sidebar, t.popover] {
                    assert!(
                        contrast(t.foreground, surface) >= 4.5,
                        "{mode:?} foreground"
                    );
                    assert!(
                        contrast(t.muted_foreground, surface) >= 4.5,
                        "{mode:?} muted text: {}",
                        contrast(t.muted_foreground, surface)
                    );
                }
                for color in [t.red, t.green, t.blue, t.yellow, t.magenta, t.cyan] {
                    let label = ensure_contrast(color, &[t.background], 5.0);
                    assert!(contrast(label, t.background) >= 4.5, "{mode:?} label color");
                }
            });
        }
    }
}
