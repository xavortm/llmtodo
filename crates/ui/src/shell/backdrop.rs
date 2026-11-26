//! Background images behind the app (LLM-566).
//!
//! `config.toml` holds one table per slot under `[app.background]`:
//!
//! ```toml
//! [app.background.window]   # behind the whole window
//! image = "~/Pictures/dunes.jpg"
//! opacity = 0.15
//!
//! [app.background.sidebar]  # also `list` and `task`
//! image = "tile.png"        # relative to the folder of config.toml
//! size = "auto"
//! repeat = "repeat"
//! position = "top left"
//! ```
//!
//! The keys follow CSS `background-*`:
//!
//! - `opacity`: 0 to 1, 0.2 by default, so text stays readable.
//! - `size`: `cover` (the default), `contain`, `fill`, `auto` (the image's
//!   own pixels), a scale such as `0.5`, a width such as `40%` of the
//!   panel or `320px`. A width keeps the image's aspect.
//! - `position`: one or two of `left`, `center`, `right`, `top`, `bottom`,
//!   a percent or a length in px, as in CSS. `center` by default.
//! - `repeat`: `no-repeat` (the default), `repeat`, `repeat-x`, `repeat-y`.
//!
//! The window image lines up with the window and shows behind every
//! panel. A panel image lines up with its panel and draws over the window
//! image. Each image draws over the panel's own color and under its
//! content. The app reads these keys and never writes them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::{
    AnyElement, App, Bounds, Hsla, IntoElement, ParentElement as _, Pixels, RenderImage,
    Styled as _, Window, canvas, div, point, px, size,
};
use serde::{Deserialize, Serialize};

pub mod load;

use load::{BackdropLoader, Effects, Fit, ImageKey};

/// Where an image sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Window,
    Sidebar,
    List,
    Task,
}

impl Slot {
    pub const ALL: [Slot; 4] = [Slot::Window, Slot::Sidebar, Slot::List, Slot::Task];

    /// The table name under `[app.background]`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Sidebar => "sidebar",
            Self::List => "list",
            Self::Task => "task",
        }
    }
}

// Config, as written in `config.toml`.

/// `[app.background]`: one optional table per slot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackgroundsConfig {
    pub window: Option<BackgroundConfig>,
    pub sidebar: Option<BackgroundConfig>,
    pub list: Option<BackgroundConfig>,
    pub task: Option<BackgroundConfig>,
}

impl BackgroundsConfig {
    pub fn get(&self, slot: Slot) -> Option<&BackgroundConfig> {
        match slot {
            Slot::Window => self.window.as_ref(),
            Slot::Sidebar => self.sidebar.as_ref(),
            Slot::List => self.list.as_ref(),
            Slot::Task => self.task.as_ref(),
        }
    }
}

/// One slot's table. Every key but `image` is optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackgroundConfig {
    pub image: String,
    pub opacity: Option<f32>,
    pub size: Option<SizeValue>,
    pub position: Option<String>,
    pub repeat: Option<String>,
    /// LLM-572: a blur radius in px.
    pub blur: Option<f32>,
    pub grayscale: Option<bool>,
    /// A factor: below 1 darkens.
    pub brightness: Option<f32>,
    /// A color such as `"#1e1e2e"`, washed over the image.
    pub tint: Option<String>,
    pub tint_opacity: Option<f32>,
}

/// `size` takes a word or a bare number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SizeValue {
    Scale(f32),
    Text(String),
}

// Resolved values.

pub const DEFAULT_OPACITY: f32 = 0.2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BgSize {
    Cover,
    Contain,
    Fill,
    /// A factor on the image's own size. `auto` is 1.
    Scale(f32),
    /// The width as a share of the panel's width, 0 to 1.
    WidthShare(f32),
    /// The width in px.
    Width(f32),
}

/// One axis of `position`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Offset {
    /// 0 puts the image's start at the panel's start, 1 its end at the end.
    Share(f32),
    Px(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repeat {
    pub x: bool,
    pub y: bool,
}

/// How one image lays out, apart from the file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub size: BgSize,
    pub position: (Offset, Offset),
    pub repeat: Repeat,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            size: BgSize::Cover,
            position: (Offset::Share(0.5), Offset::Share(0.5)),
            repeat: Repeat { x: false, y: false },
        }
    }
}

/// A slot's image, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Backdrop {
    pub image: PathBuf,
    /// The file's modification time when the app last looked, `None`
    /// for a file that is missing (LLM-568).
    pub stamp: Option<SystemTime>,
    pub opacity: f32,
    pub layout: Layout,
    /// Blur and brightness, baked in as the image loads (LLM-572).
    pub effects: Effects,
    pub grayscale: bool,
    /// A color washed over the image, with its own opacity.
    pub tint: Option<Hsla>,
}

impl Backdrop {
    /// Whether `key` loads this image, at any size.
    fn loads(&self, key: &ImageKey) -> bool {
        key.path == self.image && key.stamp == self.stamp && key.effects == self.effects
    }
}

/// The modification time of `path`, `None` when it is missing.
pub fn stamp_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The images in use, as a global. Empty slots are `None`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Backdrops {
    pub window: Option<Backdrop>,
    pub sidebar: Option<Backdrop>,
    pub list: Option<Backdrop>,
    pub task: Option<Backdrop>,
}

impl gpui_kit::Global for Backdrops {}

impl Backdrops {
    pub fn get(&self, slot: Slot) -> Option<&Backdrop> {
        match slot {
            Slot::Window => self.window.as_ref(),
            Slot::Sidebar => self.sidebar.as_ref(),
            Slot::List => self.list.as_ref(),
            Slot::Task => self.task.as_ref(),
        }
    }

    fn get_mut(&mut self, slot: Slot) -> &mut Option<Backdrop> {
        match slot {
            Slot::Window => &mut self.window,
            Slot::Sidebar => &mut self.sidebar,
            Slot::List => &mut self.list,
            Slot::Task => &mut self.task,
        }
    }

    /// Resolves the config. A relative `image` is read from `base`, the
    /// folder of `config.toml`. Each value the app does not understand
    /// falls back to its default and adds a line to the returned problems.
    pub fn resolve(config: &BackgroundsConfig, base: Option<&Path>) -> (Self, Vec<String>) {
        let mut out = Self::default();
        let mut problems = Vec::new();
        for slot in Slot::ALL {
            let Some(c) = config.get(slot) else { continue };
            *out.get_mut(slot) = resolve_slot(slot.key(), c, base, &mut problems);
        }
        (out, problems)
    }

    /// The image files that are not there, by slot.
    pub fn missing(&self) -> Vec<(Slot, PathBuf)> {
        Slot::ALL
            .into_iter()
            .filter_map(|slot| {
                let b = self.get(slot)?;
                b.stamp.is_none().then(|| (slot, b.image.clone()))
            })
            .collect()
    }
}

/// The default `tint_opacity`.
pub const DEFAULT_TINT_OPACITY: f32 = 0.3;

/// One slot's table, resolved. `name` names it in problems.
fn resolve_slot(
    name: &str,
    c: &BackgroundConfig,
    base: Option<&Path>,
    problems: &mut Vec<String>,
) -> Option<Backdrop> {
    let image = c.image.trim();
    if image.is_empty() {
        return None;
    }
    let mut problem = |what: &str, value: &str| {
        problems.push(format!("background.{name}: unknown {what} \"{value}\""));
    };
    let mut share = |what: &str, value: Option<f32>, default: f32| match value {
        Some(o) if o.is_finite() => o.clamp(0., 1.),
        Some(o) => {
            problem(what, &o.to_string());
            default
        }
        None => default,
    };
    let opacity = share("opacity", c.opacity, DEFAULT_OPACITY);
    let tint_opacity = share("tint_opacity", c.tint_opacity, DEFAULT_TINT_OPACITY);
    let mut problem = |what: &str, value: &str| {
        problems.push(format!("background.{name}: unknown {what} \"{value}\""));
    };
    let mut layout = Layout::default();
    if let Some(s) = &c.size {
        match parse_size(s) {
            Some(s) => layout.size = s,
            None => problem("size", &size_text(s)),
        }
    }
    if let Some(p) = &c.position {
        match parse_position(p) {
            Some(p) => layout.position = p,
            None => problem("position", p),
        }
    }
    if let Some(r) = &c.repeat {
        match parse_repeat(r) {
            Some(r) => layout.repeat = r,
            None => problem("repeat", r),
        }
    }
    let mut effects = Effects::default();
    match c.blur {
        Some(b) if b.is_finite() && b >= 0. => effects.blur = b.min(MAX_BLUR),
        Some(b) => problem("blur", &b.to_string()),
        None => {}
    }
    match c.brightness {
        Some(b) if b.is_finite() && b >= 0. => effects.brightness = b.min(MAX_BRIGHTNESS),
        Some(b) => problem("brightness", &b.to_string()),
        None => {}
    }
    let tint = match c.tint.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(t) => match gpui_kit::Rgba::try_from(t) {
            Ok(rgba) => {
                let color: Hsla = rgba.into();
                Some(color.opacity(tint_opacity))
            }
            Err(_) => {
                problem("tint", t);
                None
            }
        },
    };
    let image = resolve_path(image, base);
    Some(Backdrop {
        stamp: stamp_of(&image),
        image,
        opacity,
        layout,
        effects,
        grayscale: c.grayscale.unwrap_or(false),
        tint,
    })
}

/// The largest `blur`, in px. More shows as a flat wash anyway.
pub const MAX_BLUR: f32 = 200.;
/// The largest `brightness`.
pub const MAX_BRIGHTNESS: f32 = 4.;

fn size_text(s: &SizeValue) -> String {
    match s {
        SizeValue::Scale(f) => f.to_string(),
        SizeValue::Text(t) => t.clone(),
    }
}

/// `~` is the home folder. A relative path is read from `base`.
pub fn resolve_path(image: &str, base: Option<&Path>) -> PathBuf {
    if let Some(rest) = image.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    let path = PathBuf::from(image);
    match base {
        Some(base) if path.is_relative() => base.join(path),
        _ => path,
    }
}

pub fn parse_size(value: &SizeValue) -> Option<BgSize> {
    let text = match value {
        SizeValue::Scale(f) => return (f.is_finite() && *f > 0.).then_some(BgSize::Scale(*f)),
        SizeValue::Text(t) => t.trim().to_ascii_lowercase(),
    };
    match text.as_str() {
        "cover" => Some(BgSize::Cover),
        "contain" => Some(BgSize::Contain),
        "fill" | "100% 100%" => Some(BgSize::Fill),
        "auto" => Some(BgSize::Scale(1.)),
        _ => {
            if let Some(p) = text.strip_suffix('%') {
                let p: f32 = p.trim().parse().ok()?;
                (p.is_finite() && p > 0.).then_some(BgSize::WidthShare(p / 100.))
            } else if let Some(n) = text.strip_suffix("px") {
                let n: f32 = n.trim().parse().ok()?;
                (n.is_finite() && n > 0.).then_some(BgSize::Width(n))
            } else {
                let f: f32 = text.parse().ok()?;
                (f.is_finite() && f > 0.).then_some(BgSize::Scale(f))
            }
        }
    }
}

/// One token of `position`: a keyword, and which axis it forces.
enum Token {
    X(f32),
    Y(f32),
    /// `center`, or a number: fits either axis.
    Either(Offset),
}

fn token(t: &str) -> Option<Token> {
    Some(match t {
        "left" => Token::X(0.),
        "right" => Token::X(1.),
        "top" => Token::Y(0.),
        "bottom" => Token::Y(1.),
        "center" => Token::Either(Offset::Share(0.5)),
        _ => {
            if let Some(p) = t.strip_suffix('%') {
                Token::Either(Offset::Share(
                    p.parse::<f32>().ok()?.clamp(-1000., 1000.) / 100.,
                ))
            } else {
                let n = t.strip_suffix("px").unwrap_or(t);
                let n: f32 = n.parse().ok()?;
                if !n.is_finite() {
                    return None;
                }
                Token::Either(Offset::Px(n))
            }
        }
    })
}

/// CSS `background-position` with one or two values. One value sets
/// the axis it names, or x, and centers the other. Keywords may come in
/// either order: `top left` is `left top`.
pub fn parse_position(text: &str) -> Option<(Offset, Offset)> {
    let lower = text.trim().to_ascii_lowercase();
    let tokens: Vec<Token> = lower.split_whitespace().map(token).collect::<Option<_>>()?;
    let center = Offset::Share(0.5);
    match tokens.as_slice() {
        [Token::X(x)] => Some((Offset::Share(*x), center)),
        [Token::Y(y)] => Some((center, Offset::Share(*y))),
        [Token::Either(o)] => Some((*o, center)),
        [a, b] => {
            let (x, y) = match (a, b) {
                (Token::X(_), Token::X(_)) | (Token::Y(_), Token::Y(_)) => return None,
                (Token::Y(_), _) | (_, Token::X(_)) => (b, a),
                _ => (a, b),
            };
            let offset = |t: &Token| match t {
                Token::X(v) | Token::Y(v) => Offset::Share(*v),
                Token::Either(o) => *o,
            };
            Some((offset(x), offset(y)))
        }
        _ => None,
    }
}

pub fn parse_repeat(text: &str) -> Option<Repeat> {
    let (x, y) = match text.trim().to_ascii_lowercase().as_str() {
        "no-repeat" | "none" => (false, false),
        "repeat" => (true, true),
        "repeat-x" => (true, false),
        "repeat-y" => (false, true),
        _ => return None,
    };
    Some(Repeat { x, y })
}

// Layout math, in logical px. Kept apart from GPUI so tests can check it.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// More tiles than this means a tiny image on a big panel. The rest are
/// left out, so one setting cannot stall a frame.
pub const MAX_TILES: usize = 4096;

/// The image's size in the box.
fn image_size(area: Rect, natural: (f32, f32), size: BgSize) -> (f32, f32) {
    let (iw, ih) = natural;
    match size {
        BgSize::Cover => {
            let s = (area.w / iw).max(area.h / ih);
            (iw * s, ih * s)
        }
        BgSize::Contain => {
            let s = (area.w / iw).min(area.h / ih);
            (iw * s, ih * s)
        }
        BgSize::Fill => (area.w, area.h),
        BgSize::Scale(s) => (iw * s, ih * s),
        BgSize::WidthShare(p) => (area.w * p, area.w * p * ih / iw),
        BgSize::Width(w) => (w, w * ih / iw),
    }
}

/// Where the image goes in `area`: one rect, or one per tile. A tile that
/// lies outside `area` is left out.
pub fn tiles(area: Rect, natural: (f32, f32), layout: &Layout) -> Vec<Rect> {
    if natural.0 <= 0. || natural.1 <= 0. || area.w <= 0. || area.h <= 0. {
        return Vec::new();
    }
    let (w, h) = image_size(area, natural, layout.size);
    if !(w.is_finite() && h.is_finite()) || w < 0.5 || h < 0.5 {
        return Vec::new();
    }
    let place = |offset: Offset, start: f32, room: f32, len: f32| match offset {
        Offset::Share(s) => start + (room - len) * s,
        Offset::Px(n) => start + n,
    };
    let x = place(layout.position.0, area.x, area.w, w);
    let y = place(layout.position.1, area.y, area.h, h);
    // Repeating steps back to the first tile that reaches the box.
    let starts = |at: f32, start: f32, room: f32, len: f32, repeat: bool| -> Vec<f32> {
        if !repeat {
            return if at + len > start && at < start + room {
                vec![at]
            } else {
                Vec::new()
            };
        }
        let first = at - ((at - start) / len).ceil() * len;
        let count = ((start + room - first) / len).ceil().max(0.) as usize;
        (0..count.min(MAX_TILES))
            .map(|i| first + i as f32 * len)
            .collect()
    };
    let xs = starts(x, area.x, area.w, w, layout.repeat.x);
    let ys = starts(y, area.y, area.h, h, layout.repeat.y);
    let mut out = Vec::with_capacity((xs.len() * ys.len()).min(MAX_TILES));
    'rows: for &ty in &ys {
        for &tx in &xs {
            if out.len() == MAX_TILES {
                break 'rows;
            }
            out.push(Rect { x: tx, y: ty, w, h });
        }
    }
    out
}

// Global and drawing.

/// The images drawn so far, by what loaded them. `set` drops the ones
/// the new images no longer use, so a replaced photo frees its memory.
#[derive(Default)]
struct Loads {
    keys: HashMap<ImageKey, Option<Arc<RenderImage>>>,
    /// Images to take out of the sprite atlas at the next paint.
    retired: Vec<Arc<RenderImage>>,
}

impl gpui_kit::Global for Loads {}

/// Sets the images in use and redraws every window, since the bars and
/// panels are cached views.
pub fn set(backdrops: Backdrops, cx: &mut App) {
    let before = cx.try_global::<Backdrops>().cloned().unwrap_or_default();
    if before == backdrops {
        return;
    }
    let loads = cx.default_global::<Loads>();
    let stale: Vec<ImageKey> = loads
        .keys
        .keys()
        .filter(|key| {
            !Slot::ALL
                .iter()
                .any(|s| backdrops.get(*s).is_some_and(|b| b.loads(key)))
        })
        .cloned()
        .collect();
    for key in &stale {
        if let Some(Some(image)) = loads.keys.remove(key) {
            loads.retired.push(image);
        }
    }
    for key in &stale {
        cx.remove_asset::<BackdropLoader>(key);
    }
    cx.set_global(backdrops);
    cx.refresh_windows();
}

/// Looks at each image file again, and loads the ones that changed on
/// disk (LLM-568). Returns the files that are missing now.
pub fn check_files(cx: &mut App) -> Vec<(Slot, PathBuf)> {
    let Some(mut all) = cx.try_global::<Backdrops>().cloned() else {
        return Vec::new();
    };
    for slot in Slot::ALL {
        if let Some(b) = all.get_mut(slot) {
            b.stamp = stamp_of(&b.image);
        }
    }
    let missing = all.missing();
    set(all, cx);
    missing
}

fn current(slot: Slot, cx: &App) -> Option<Backdrop> {
    cx.try_global::<Backdrops>()
        .and_then(|b| b.get(slot).cloned())
}

/// Whether any of `slots` shows an image.
pub fn any(slots: &[Slot], cx: &App) -> bool {
    cx.try_global::<Backdrops>()
        .is_some_and(|b| slots.iter().any(|s| b.get(*s).is_some()))
}

/// The fill for a surface in the page color that sits over the images of
/// `under`: clear while one of them shows, so the image shows through.
pub fn surface(color: Hsla, under: &[Slot], cx: &App) -> Hsla {
    if any(under, cx) {
        gpui_kit::transparent_black()
    } else {
        color
    }
}

/// A layer that fills its parent and draws the images of `slots`, in
/// order. Put it first in a `relative` parent, after the parent's own
/// fill. `None` when no slot has an image.
pub fn layer(slots: &[Slot], cx: &App) -> Option<AnyElement> {
    let images: Vec<(Slot, Backdrop)> = slots
        .iter()
        .filter_map(|s| current(*s, cx).map(|b| (*s, b)))
        .filter(|(_, b)| b.opacity > 0. && b.stamp.is_some())
        .collect();
    if images.is_empty() {
        return None;
    }
    Some(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .children(images.into_iter().map(|(slot, b)| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .opacity(b.opacity)
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, cx| paint(slot, &b, bounds, window, cx),
                        )
                        .size_full(),
                    )
            }))
            .into_any_element(),
    )
}

/// The largest screen attached, in logical px. Images shrink to it.
fn largest_screen(window: &Window, cx: &App) -> (f32, f32) {
    let mut w = f32::from(window.viewport_size().width);
    let mut h = f32::from(window.viewport_size().height);
    for display in cx.displays() {
        let size = display.bounds().size;
        w = w.max(f32::from(size.width));
        h = h.max(f32::from(size.height));
    }
    (w.ceil(), h.ceil())
}

/// The key that loads `b` in this window.
fn key_for(b: &Backdrop, window: &Window, cx: &App) -> ImageKey {
    ImageKey {
        path: b.image.clone(),
        stamp: b.stamp,
        fit: Fit {
            size: b.layout.size,
            screen: largest_screen(window, cx),
            scale: window.scale_factor(),
        },
        effects: b.effects,
    }
}

fn paint(slot: Slot, b: &Backdrop, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let retired = std::mem::take(&mut cx.default_global::<Loads>().retired);
    for image in retired {
        let _ = window.drop_image(image);
    }
    let key = key_for(b, window, cx);
    let loaded = match window.use_asset::<BackdropLoader>(&key, cx) {
        Some(Ok(loaded)) => loaded,
        // Loading, or a file that is not an image. The loader has
        // written why to the log.
        _ => {
            cx.default_global::<Loads>().keys.entry(key).or_insert(None);
            return;
        }
    };
    cx.default_global::<Loads>()
        .keys
        .insert(key, Some(loaded.image.clone()));
    // The window image lines up with the window, so it runs on across
    // panels. A panel image lines up with its panel.
    let area = if slot == Slot::Window {
        let view = window.viewport_size();
        Rect {
            x: 0.,
            y: 0.,
            w: f32::from(view.width),
            h: f32::from(view.height),
        }
    } else {
        rect_of(bounds)
    };
    let clip = rect_of(bounds);
    for t in tiles(area, loaded.natural, &b.layout) {
        if t.x >= clip.x + clip.w
            || t.y >= clip.y + clip.h
            || t.x + t.w <= clip.x
            || t.y + t.h <= clip.y
        {
            continue;
        }
        let image_bounds = Bounds::new(point(px(t.x), px(t.y)), size(px(t.w), px(t.h)));
        let _ = window.paint_image(
            bounds,
            image_bounds,
            Default::default(),
            loaded.image.clone(),
            0,
            b.grayscale,
        );
        // LLM-572: the tint washes over the image only, not the panel.
        if let Some(tint) = b.tint {
            window.with_content_mask(Some(gpui_kit::ContentMask { bounds }), |window| {
                window.paint_quad(gpui_kit::fill(image_bounds, tint))
            });
        }
    }
}

fn rect_of(bounds: Bounds<Pixels>) -> Rect {
    Rect {
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        w: f32::from(bounds.size.width),
        h: f32::from(bounds.size.height),
    }
}
