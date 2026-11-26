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
//! content. A settings save never writes these keys; the Settings
//! Background section writes them one at a time (LLM-567).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::{
    AnyElement, App, Bounds, Hsla, IntoElement, ParentElement as _, Pixels, RenderImage,
    Styled as _, Window, canvas, div, point, px, size,
};
use serde::{Deserialize, Serialize};

pub mod load;

use load::{BackdropLoader, Effects, Fit, ImageKey};

/// Where an image sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// `[app.background]`: one optional table per slot, and the same tables
/// per project and per team (LLM-574).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackgroundsConfig {
    pub window: Option<BackgroundConfig>,
    pub sidebar: Option<BackgroundConfig>,
    pub list: Option<BackgroundConfig>,
    pub task: Option<BackgroundConfig>,
    /// `[app.background.project.<slug>.<slot>]`.
    pub project: BTreeMap<String, SlotTables>,
    /// `[app.background.team.<KEY>.<slot>]`.
    pub team: BTreeMap<String, SlotTables>,
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

    /// The table that applies to `slot` for `pick`: the project's, then
    /// the first of its teams', then the global one. The choice is per
    /// slot, so a project can set only its list.
    pub fn pick(&self, slot: Slot, pick: &Pick) -> Option<&BackgroundConfig> {
        fn find<'a>(
            map: &'a BTreeMap<String, SlotTables>,
            name: &str,
            slot: Slot,
        ) -> Option<&'a BackgroundConfig> {
            map.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .and_then(|(_, t)| t.get(slot))
        }
        pick.project
            .as_deref()
            .and_then(|p| find(&self.project, p, slot))
            .or_else(|| pick.teams.iter().find_map(|t| find(&self.team, t, slot)))
            .or_else(|| self.get(slot))
    }
}

/// The four slot tables of one project or team.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SlotTables {
    pub window: Option<BackgroundConfig>,
    pub sidebar: Option<BackgroundConfig>,
    pub list: Option<BackgroundConfig>,
    pub task: Option<BackgroundConfig>,
}

impl SlotTables {
    pub fn get(&self, slot: Slot) -> Option<&BackgroundConfig> {
        match slot {
            Slot::Window => self.window.as_ref(),
            Slot::Sidebar => self.sidebar.as_ref(),
            Slot::List => self.list.as_ref(),
            Slot::Task => self.task.as_ref(),
        }
    }
}

/// What decides which table and image apply.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pick {
    /// The sidebar's project, by slug.
    pub project: Option<String>,
    /// The sidebar's team, or the project's teams, by key.
    pub teams: Vec<String>,
}

/// One slot's table. Every key but `image` is optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackgroundConfig {
    pub image: String,
    /// LLM-571: the image and opacity in dark and in light mode. Each
    /// falls back to `image` or `opacity`.
    pub image_dark: Option<String>,
    pub image_light: Option<String>,
    pub opacity: Option<f32>,
    pub opacity_dark: Option<f32>,
    pub opacity_light: Option<f32>,
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
    /// LLM-576: `fixed` (the default) or `scroll`, as CSS
    /// `background-attachment`. Only `list` and `task` scroll.
    pub attachment: Option<String>,
    /// LLM-578: a shader file, or `builtin:<name>`, in place of `image`.
    pub shader: Option<String>,
    pub shader_dark: Option<String>,
    pub shader_light: Option<String>,
    /// Frames per second, 1 to 120.
    pub fps: Option<f32>,
    /// The share of the slot's pixels the shader renders, 0.25 to 1.
    pub scale: Option<f32>,
}

/// LLM-578: how a slot runs its shader.
#[derive(Debug, Clone, PartialEq)]
pub struct ShaderSpec {
    pub source: crate::shell::shader::Source,
    pub fps: f32,
    pub scale: f32,
}

pub const DEFAULT_FPS: f32 = 30.;
pub const DEFAULT_SHADER_SCALE: f32 = 0.5;

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
    /// LLM-576: the image moves with its panel's content.
    pub scroll: bool,
    /// LLM-578: the slot runs this shader; `image` then names its file,
    /// or `builtin:<name>`.
    pub shader: Option<ShaderSpec>,
}

impl Backdrop {
    /// Looks at the file again. A built-in shader is always there.
    fn restamp(&mut self) {
        if !matches!(
            self.shader.as_ref().map(|s| &s.source),
            Some(crate::shell::shader::Source::Builtin(_))
        ) {
            self.stamp = stamp_of(&self.image);
        }
    }
}

/// The modification time of `path`, `None` when it is missing.
pub fn stamp_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The images of one mode. Empty slots are `None`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Backdrops {
    pub window: Option<Backdrop>,
    pub sidebar: Option<Backdrop>,
    pub list: Option<Backdrop>,
    pub task: Option<Backdrop>,
}

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

    /// Resolves the config for one mode. A relative `image` is read
    /// from `base`, the folder of `config.toml`. Each value the app does
    /// not understand falls back to its default and adds a line to the
    /// returned problems.
    pub fn resolve(
        config: &BackgroundsConfig,
        base: Option<&Path>,
        pick: &Pick,
        dark: bool,
    ) -> (Self, Vec<String>) {
        let mut out = Self::default();
        let mut problems = Vec::new();
        for slot in Slot::ALL {
            let Some(c) = config.pick(slot, pick) else {
                continue;
            };
            *out.get_mut(slot) = resolve_slot(slot, c, base, dark, &mut problems);
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

/// The images in use, as a global: one set per mode, so a switch
/// between light and dark needs no new resolve (LLM-571).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shown {
    pub light: Backdrops,
    pub dark: Backdrops,
}

impl gpui_kit::Global for Shown {}

impl Shown {
    /// Both modes. Problems are named once, though both modes read them.
    pub fn resolve(
        config: &BackgroundsConfig,
        base: Option<&Path>,
        pick: &Pick,
    ) -> (Self, Vec<String>) {
        let (light, mut problems) = Backdrops::resolve(config, base, pick, false);
        let (dark, more) = Backdrops::resolve(config, base, pick, true);
        for p in more {
            if !problems.contains(&p) {
                problems.push(p);
            }
        }
        (Self { light, dark }, problems)
    }

    pub fn mode(&self, dark: bool) -> &Backdrops {
        if dark { &self.dark } else { &self.light }
    }

    fn each_mut(&mut self) -> [&mut Backdrops; 2] {
        [&mut self.light, &mut self.dark]
    }

    fn each(&self) -> [&Backdrops; 2] {
        [&self.light, &self.dark]
    }

    /// The missing files of both modes, each once.
    pub fn missing(&self) -> Vec<(Slot, PathBuf)> {
        let mut out = self.light.missing();
        for m in self.dark.missing() {
            if !out.contains(&m) {
                out.push(m);
            }
        }
        out
    }
}

/// The default `tint_opacity`.
pub const DEFAULT_TINT_OPACITY: f32 = 0.3;

/// One slot's table, resolved. `name` names it in problems.
fn resolve_slot(
    slot: Slot,
    c: &BackgroundConfig,
    base: Option<&Path>,
    dark: bool,
    problems: &mut Vec<String>,
) -> Option<Backdrop> {
    let (image, opacity, shader) = if dark {
        (&c.image_dark, c.opacity_dark, &c.shader_dark)
    } else {
        (&c.image_light, c.opacity_light, &c.shader_light)
    };
    let name = slot.key();
    let image = image.as_deref().unwrap_or(&c.image).trim();
    let shader = shader
        .as_deref()
        .or(c.shader.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if image.is_empty() && shader.is_none() {
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
    let opacity = share("opacity", opacity.or(c.opacity), DEFAULT_OPACITY);
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
    let scroll = match c
        .attachment
        .as_deref()
        .map(|a| a.trim().to_ascii_lowercase())
    {
        None => false,
        Some(a) if a == "fixed" => false,
        Some(a) if a == "scroll" && matches!(slot, Slot::List | Slot::Task) => true,
        Some(a) => {
            problem("attachment", &a);
            false
        }
    };
    // LLM-578: a shader wins over an image in the same table.
    let shader = shader.map(|text| {
        let source = crate::shell::shader::Source::parse(text, base);
        let fps = match c.fps {
            Some(f) if f.is_finite() && f > 0. => f.clamp(1., 120.),
            Some(f) => {
                problem("fps", &f.to_string());
                DEFAULT_FPS
            }
            None => DEFAULT_FPS,
        };
        let scale = match c.scale {
            Some(f) if f.is_finite() && f > 0. => f.clamp(0.25, 1.),
            Some(f) => {
                problem("scale", &f.to_string());
                DEFAULT_SHADER_SCALE
            }
            None => DEFAULT_SHADER_SCALE,
        };
        ShaderSpec { source, fps, scale }
    });
    let (image, stamp) = match shader.as_ref().map(|s| &s.source) {
        Some(crate::shell::shader::Source::Builtin(name)) => (
            PathBuf::from(format!("builtin:{name}")),
            Some(SystemTime::UNIX_EPOCH),
        ),
        Some(crate::shell::shader::Source::File(path)) => (path.clone(), stamp_of(path)),
        None => {
            let image = resolve_path(image, base);
            (image.clone(), stamp_of(&image))
        }
    };
    Some(Backdrop {
        scroll,
        shader,
        stamp,
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
    tiles_over(area, area, natural, layout)
}

/// Like [`tiles`], but fills `cover` with the tiles laid out in `area`.
/// An image that scrolls lays out in its moved area, and its repeats
/// still fill the panel (LLM-576).
pub fn tiles_over(area: Rect, cover: Rect, natural: (f32, f32), layout: &Layout) -> Vec<Rect> {
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
    let xs = starts(x, cover.x, cover.w, w, layout.repeat.x);
    let ys = starts(y, cover.y, cover.h, h, layout.repeat.y);
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
    /// Where each panel's own layer last drew, so a layer over part of
    /// the panel lines its image up with the panel (LLM-570).
    areas: HashMap<Slot, Rect>,
    /// When each image first drew. A fade in and an animation count from
    /// here (LLM-577, LLM-569).
    ready: HashMap<Ident, Instant>,
    /// When a timer will next redraw the window for an animated image.
    wake: Option<Instant>,
}

impl gpui_kit::Global for Loads {}

/// Sets the images in use and redraws every window, since the bars and
/// panels are cached views.
pub fn set(backdrops: Shown, cx: &mut App) {
    let before = cx.try_global::<Shown>().cloned().unwrap_or_default();
    if before == backdrops {
        return;
    }
    // The images still in use: the new ones, and what each slot shows
    // now, which may fade out after this (LLM-577).
    let mut live: Vec<Ident> = backdrops
        .each()
        .iter()
        .flat_map(|mode| Slot::ALL.iter().filter_map(|s| mode.get(*s).map(ident)))
        .collect();
    if let Some(f) = cx.try_global::<Fades>() {
        for state in f.0.borrow().values() {
            live.extend(state.current.iter().chain(&state.previous).map(ident));
        }
    }
    let loads = cx.default_global::<Loads>();
    let stale: Vec<ImageKey> = loads
        .keys
        .keys()
        .filter(|key| !live.contains(&key_ident(key)))
        .cloned()
        .collect();
    for key in &stale {
        if let Some(Some(image)) = loads.keys.remove(key) {
            loads.retired.push(image);
        }
    }
    loads.ready.retain(|k, _| live.contains(k));
    for key in &stale {
        cx.remove_asset::<BackdropLoader>(key);
    }
    // A slot that no longer runs a shader lets its last frame go. One
    // that may still fade out keeps it until the next change.
    let shader_slots: Vec<Slot> = Slot::ALL
        .into_iter()
        .filter(|s| {
            backdrops
                .each()
                .iter()
                .any(|m| m.get(*s).is_some_and(|b| b.shader.is_some()))
        })
        .collect();
    let fading_shader = |slot: &Slot| {
        cx.try_global::<Fades>().is_some_and(|f| {
            f.0.borrow()
                .get(slot)
                .and_then(|s| s.current.as_ref())
                .is_some_and(|b| b.shader.is_some())
        })
    };
    let drop_runs: Vec<Slot> = cx
        .try_global::<Runs>()
        .map(|r| {
            r.0.keys()
                .filter(|s| !shader_slots.contains(s) && !fading_shader(s))
                .copied()
                .collect()
        })
        .unwrap_or_default();
    let mut gone = Vec::new();
    for slot in drop_runs {
        if let Some(run) = cx.default_global::<Runs>().0.remove(&slot) {
            gone.extend(run.frame.and_then(|f| f.image));
        }
    }
    cx.default_global::<Loads>().retired.extend(gone);
    cx.default_global::<Fades>();
    cx.set_global(backdrops);
    cx.refresh_windows();
}

// LLM-569: animated images.

/// The frame of `image` to show now, counted from when it first drew.
/// Asks for a redraw when the next frame is due. With animations off,
/// the first frame.
fn animation_frame(
    image: &RenderImage,
    ready: Instant,
    now: Instant,
    window: &Window,
    cx: &mut App,
) -> usize {
    let count = image.frame_count();
    if count < 2 || !crate::shell::motion::on(cx) {
        return 0;
    }
    let delays: Vec<u64> = (0..count).map(|i| load::frame_ms(image.delay(i))).collect();
    let (frame, until) = frame_at(&delays, now.saturating_duration_since(ready));
    wake_at(now + until, window, cx);
    frame
}

/// The frame showing `elapsed` into a loop of `delays` ms, and how long
/// until the next one.
pub fn frame_at(delays: &[u64], elapsed: Duration) -> (usize, Duration) {
    let total: u64 = delays.iter().sum();
    if total == 0 {
        return (0, Duration::from_millis(100));
    }
    let at = elapsed.as_millis() as u64 % total;
    let mut start = 0;
    for (i, d) in delays.iter().enumerate() {
        if at < start + d {
            return (i, Duration::from_millis(start + d - at));
        }
        start += d;
    }
    (0, Duration::from_millis(delays[0]))
}

/// Redraws the painting view at `at`. One timer serves every image: an
/// earlier wish moves it, a later one waits for it. Only the workspace
/// root draws again; the cached panels stay as they are.
fn wake_at(at: Instant, window: &Window, cx: &mut App) {
    let now = cx.background_executor().now();
    let loads = cx.default_global::<Loads>();
    if let Some(w) = loads.wake
        && w > now
        && w <= at
    {
        return;
    }
    loads.wake = Some(at);
    let view = window.current_view();
    let delay = at.saturating_duration_since(now);
    cx.spawn(async move |cx| {
        cx.background_executor().timer(delay).await;
        cx.update(|cx| {
            let loads = cx.default_global::<Loads>();
            if loads.wake == Some(at) {
                loads.wake = None;
            }
            cx.notify(view);
        });
    })
    .detach();
}

/// Looks at each image file again, and loads the ones that changed on
/// disk (LLM-568). Returns the files that are missing now.
pub fn check_files(cx: &mut App) -> Vec<(Slot, PathBuf)> {
    let Some(mut all) = cx.try_global::<Shown>().cloned() else {
        return Vec::new();
    };
    for mode in all.each_mut() {
        for slot in Slot::ALL {
            if let Some(b) = mode.get_mut(slot) {
                b.restamp();
            }
        }
    }
    let missing = all.missing();
    set(all, cx);
    missing
}

/// The images of the mode the theme is in now.
pub fn shown(cx: &App) -> Backdrops {
    use gpui_kit::component::ActiveTheme as _;
    cx.try_global::<Shown>()
        .map(|s| s.mode(cx.theme().is_dark()).clone())
        .unwrap_or_default()
}

fn current(slot: Slot, cx: &App) -> Option<Backdrop> {
    use gpui_kit::component::ActiveTheme as _;
    cx.try_global::<Shown>()
        .and_then(|s| s.mode(cx.theme().is_dark()).get(slot).cloned())
}

/// Whether `slot` draws anything now, or still fades out.
fn shows(slot: Slot, cx: &App) -> bool {
    current(slot, cx).is_some_and(|b| b.opacity > 0.)
        || cx.try_global::<Fades>().is_some_and(|f| {
            f.0.borrow()
                .get(&slot)
                .is_some_and(|s| s.current.is_some() || s.previous.is_some())
        })
}

/// Whether any of `slots` shows an image.
pub fn any(slots: &[Slot], cx: &App) -> bool {
    slots.iter().any(|s| current(*s, cx).is_some())
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
    layer_with(slots, None, cx)
}

/// Reads a panel's scroll offset at paint, so a scroll needs no new
/// render of the panel (LLM-576).
pub type ScrollOffset = std::rc::Rc<dyn Fn() -> gpui_kit::Point<Pixels>>;

/// [`layer`] for a panel whose images may scroll with `scroll`.
pub fn layer_with(slots: &[Slot], scroll: Option<ScrollOffset>, cx: &App) -> Option<AnyElement> {
    layer_inner(slots, scroll, false, None, cx)
}

/// The opacity a parent gives the layer, from 0 to 1, written while the
/// parent renders, such as a panel that fades in (LLM-579). A shader
/// draws as a surface only at 1: a surface ignores element opacity.
pub type ParentFade = std::rc::Rc<std::cell::Cell<f32>>;

/// [`layer_with`] inside a parent whose opacity `fade` reports.
pub fn layer_faded(
    slots: &[Slot],
    scroll: Option<ScrollOffset>,
    fade: ParentFade,
    cx: &App,
) -> Option<AnyElement> {
    layer_inner(slots, scroll, false, Some(fade), cx)
}

/// A layer for a surface that covers part of a panel and must show the
/// same picture as the panel under it, such as the list's open and close
/// animation (LLM-570). Each panel image lines up with where the panel's
/// own layer last drew, not with this layer. Put it over the page color.
pub fn layer_over(slots: &[Slot], scroll: Option<ScrollOffset>, cx: &App) -> Option<AnyElement> {
    layer_inner(slots, scroll, true, None, cx)
}

fn layer_inner(
    slots: &[Slot],
    scroll: Option<ScrollOffset>,
    over: bool,
    parent: Option<ParentFade>,
    cx: &App,
) -> Option<AnyElement> {
    use gpui_kit::component::ActiveTheme as _;
    let now = cx.background_executor().now();
    let animate = crate::shell::motion::on(cx);
    let mut draws: Vec<(Slot, Backdrop, f32, bool)> = Vec::new();
    let mut running = false;
    for &slot in slots {
        let b = current(slot, cx).filter(|b| b.opacity > 0. && b.stamp.is_some());
        if over {
            // A surface over the panel shows what the panel shows now.
            draws.extend(b.map(|b| (slot, b.clone(), b.opacity, false)));
            continue;
        }
        let ready = |b: &Backdrop| ready_at(b, cx);
        for (b, opacity, fading) in fade(slot, b, animate, now, &ready, cx) {
            running |= fading;
            draws.push((slot, b, opacity, fading));
        }
    }
    if draws.is_empty() {
        return None;
    }
    // LLM-579: a shader draws as an opaque surface where one flat color
    // lies under it: the panel's fill, with no other background drawn
    // before it in this layer, and no window background under a panel. It
    // blends with that color at its own opacity, so it must not be fading
    // either. Elsewhere it draws as an image.
    let theme = cx.theme();
    let base = if slots.contains(&Slot::Sidebar) {
        theme.sidebar
    } else {
        theme.background
    };
    let window_under = shows(Slot::Window, cx);
    let flat: Vec<Option<Hsla>> = draws
        .iter()
        .enumerate()
        .map(|(i, (slot, _, _, fading))| {
            (i == 0 && !fading && base.a >= 1. && (*slot == Slot::Window || !window_under))
                .then_some(base)
        })
        .collect();
    Some(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .children(
                draws
                    .into_iter()
                    .zip(flat)
                    .map(move |((slot, b, opacity, _), flat)| {
                        let scroll = scroll.clone().filter(|_| b.scroll);
                        let parent = parent.clone();
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .opacity(opacity)
                            .child(
                                canvas(
                                    |_, _, _| {},
                                    move |bounds, _, window, cx| {
                                        let offset = scroll.as_ref().map(|f| f());
                                        let blend = flat
                                            .filter(|_| {
                                                parent.as_ref().is_none_or(|p| p.get() >= 1.)
                                            })
                                            .map(|c| {
                                                let [r, g, b, _] = rgba_of(c);
                                                crate::shell::shader::Blend {
                                                    under: [r, g, b],
                                                    amount: opacity,
                                                }
                                            });
                                        paint(slot, &b, offset, over, blend, bounds, window, cx);
                                        if running {
                                            window.request_animation_frame();
                                        }
                                    },
                                )
                                .size_full(),
                            )
                    }),
            )
            .into_any_element(),
    )
}

// LLM-577: a changed image fades in.

/// How long the old image takes to fade out and the new one in.
pub const FADE: Duration = Duration::from_millis(200);

/// What each slot showed, so a change can fade.
#[derive(Default)]
pub(crate) struct Fades(std::cell::RefCell<HashMap<Slot, FadeState>>);

impl gpui_kit::Global for Fades {}

pub(crate) struct FadeState {
    /// What the slot shows now. `None` while it fades out to nothing.
    current: Option<Backdrop>,
    /// What it showed before, while that fades out.
    previous: Option<Backdrop>,
    changed: Instant,
}

/// The images to draw for `slot`, with their opacity and whether they
/// still fade. Changing what `b` is starts a fade when `animate` is on.
/// A new image that is still loading keeps the old one at full opacity,
/// then fades in from the moment it is ready, so it never pops in.
pub(crate) fn fade(
    slot: Slot,
    b: Option<Backdrop>,
    animate: bool,
    now: Instant,
    ready: &dyn Fn(&Backdrop) -> Option<Instant>,
    cx: &App,
) -> Vec<(Backdrop, f32, bool)> {
    let Some(fades) = cx.try_global::<Fades>() else {
        return b
            .map(|b| vec![(b.clone(), b.opacity, false)])
            .unwrap_or_default();
    };
    let mut map = fades.0.borrow_mut();
    let state = map.entry(slot).or_insert(FadeState {
        current: None,
        previous: None,
        changed: now,
    });
    if state.current != b {
        state.previous = if animate { state.current.take() } else { None };
        state.current = b;
        state.changed = now;
    }
    if !animate {
        state.previous = None;
    }
    let mut out = Vec::new();
    // The new image's fade starts when it is ready, not when it was set.
    let t = match &state.current {
        Some(c) if animate => match ready(c) {
            Some(at) => progress(now, at.max(state.changed)),
            None => 0.,
        },
        _ => 1.,
    };
    // With nothing new, the old image fades out over the same time.
    let t = if state.current.is_none() && state.previous.is_some() {
        progress(now, state.changed)
    } else {
        t
    };
    let fading = t < 1.;
    if let Some(p) = &state.previous
        && fading
    {
        out.push((p.clone(), p.opacity * (1. - t), true));
    }
    if !fading {
        state.previous = None;
    }
    if let Some(c) = &state.current {
        // Drawn even at 0, so it loads.
        out.push((c.clone(), c.opacity * t, fading));
    }
    out
}

/// From 0 to 1 over [`FADE`] since `start`.
fn progress(now: Instant, start: Instant) -> f32 {
    (now.saturating_duration_since(start).as_secs_f32() / FADE.as_secs_f32()).clamp(0., 1.)
}

/// What identifies a decoded image apart from its size.
type Ident = (PathBuf, Option<SystemTime>, u32, u32);

fn key_ident(k: &ImageKey) -> Ident {
    (
        k.path.clone(),
        k.stamp,
        k.effects.blur.to_bits(),
        k.effects.brightness.to_bits(),
    )
}

fn ident(b: &Backdrop) -> Ident {
    (
        b.image.clone(),
        b.stamp,
        b.effects.blur.to_bits(),
        b.effects.brightness.to_bits(),
    )
}

/// When `b` first drew, `None` while it loads.
fn ready_at(b: &Backdrop, cx: &App) -> Option<Instant> {
    cx.try_global::<Loads>()
        .and_then(|l| l.ready.get(&ident(b)).copied())
}

/// LLM-567: a small picture of `path` that fills its box, as `cover`. It
/// loads through the same loader, shrunk to the box, so a large photo
/// costs little.
pub fn thumbnail(path: PathBuf) -> AnyElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let r = rect_of(bounds);
            let key = ImageKey {
                stamp: stamp_of(&path),
                path: path.clone(),
                fit: Fit {
                    size: BgSize::Cover,
                    screen: (r.w.ceil(), r.h.ceil()),
                    scale: window.scale_factor(),
                },
                effects: Effects::default(),
            };
            let Some(Ok(loaded)) = window.use_asset::<BackdropLoader>(&key, cx) else {
                return;
            };
            for t in tiles(r, loaded.natural, &Layout::default()) {
                let image_bounds = Bounds::new(point(px(t.x), px(t.y)), size(px(t.w), px(t.h)));
                let _ = window.paint_image(
                    bounds,
                    image_bounds,
                    Default::default(),
                    loaded.image.clone(),
                    0,
                    false,
                );
            }
        },
    )
    .size_full()
    .into_any_element()
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

#[allow(clippy::too_many_arguments)]
fn paint(
    slot: Slot,
    b: &Backdrop,
    scroll: Option<gpui_kit::Point<Pixels>>,
    over: bool,
    blend: Option<crate::shell::shader::Blend>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let loads = cx.default_global::<Loads>();
    let own = if over {
        loads.areas.get(&slot).copied().unwrap_or(rect_of(bounds))
    } else {
        loads.areas.insert(slot, rect_of(bounds));
        rect_of(bounds)
    };
    let retired = std::mem::take(&mut cx.default_global::<Loads>().retired);
    for image in retired {
        let _ = window.drop_image(image);
    }
    if let Some(spec) = &b.shader {
        let area = if slot == Slot::Window {
            let view = window.viewport_size();
            Rect {
                x: 0.,
                y: 0.,
                w: f32::from(view.width),
                h: f32::from(view.height),
            }
        } else {
            own
        };
        paint_shader(slot, b, spec, area, over, blend, bounds, window, cx);
        return;
    }
    let key = key_for(b, window, cx);
    let loaded = match window.use_asset::<BackdropLoader>(&key, cx) {
        Some(Ok(loaded)) => loaded,
        // Loading, or a file that is not an image. The loader has
        // written why to the log.
        result => {
            let now = cx.background_executor().now();
            let loads = cx.default_global::<Loads>();
            loads.keys.entry(key).or_insert(None);
            // A file that fails counts as ready, so the old image still
            // fades out.
            if result.is_some() {
                loads.ready.entry(ident(b)).or_insert(now);
            }
            return;
        }
    };
    let loads = cx.default_global::<Loads>();
    loads.keys.insert(key, Some(loaded.image.clone()));
    let now = cx.background_executor().now();
    let loads = cx.default_global::<Loads>();
    let ready = *loads.ready.entry(ident(b)).or_insert(now);
    let frame = animation_frame(&loaded.image, ready, now, window, cx);
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
        let mut area = own;
        // The offset is negative as the content moves up.
        if let Some(offset) = scroll {
            area.x += f32::from(offset.x);
            area.y += f32::from(offset.y);
        }
        area
    };
    let clip = rect_of(bounds);
    for t in tiles_over(area, clip, loaded.natural, &b.layout) {
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
            frame,
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

// LLM-578: shader backgrounds.

/// Compiles the shaders the shown slots name, when new or changed on
/// disk. Returns a status bar line per shader that fails; the slot keeps
/// its last good shader.
pub fn prepare_shaders(cx: &mut App) -> Vec<String> {
    let Some(shown) = cx.try_global::<Shown>().cloned() else {
        return Vec::new();
    };
    let programs = cx.default_global::<crate::shell::shader::Programs>();
    let mut problems = Vec::new();
    for mode in shown.each() {
        for slot in Slot::ALL {
            let Some(b) = mode.get(slot) else { continue };
            let Some(spec) = &b.shader else { continue };
            if b.stamp.is_none() {
                continue;
            }
            if let Some(e) = programs.prepare(&spec.source, b.stamp) {
                let line = format!(
                    "background.{}: shader {}: {e}",
                    slot.key(),
                    spec.source.label()
                );
                if !problems.contains(&line) {
                    problems.push(line);
                }
            }
        }
    }
    problems
}

/// What one slot's shader drew last, and the frame on its way.
struct Run {
    program: Arc<crate::shell::shader::Program>,
    started: Instant,
    frame: Option<ShaderFrame>,
    /// When the frame in `frame`, or the one on its way, was asked for.
    asked: Option<Instant>,
    size: (u32, u32),
    pending: bool,
    /// A finished frame waiting for the next paint.
    done: Option<(ShaderFrame, (u32, u32))>,
    /// What the paints since the last request wanted, so the next frame
    /// renders each of them (LLM-579).
    wants: Wants,
}

/// One frame of a shader in each form a paint wanted: an image for the
/// paints that fade or lie over another background, and a surface for each
/// flat color it lies over (LLM-579).
#[derive(Clone, Default)]
struct ShaderFrame {
    image: Option<Arc<RenderImage>>,
    surfaces: Vec<(crate::shell::shader::Blend, crate::shell::shader::Surface)>,
}

impl ShaderFrame {
    fn has(&self, blend: Option<crate::shell::shader::Blend>) -> bool {
        match blend {
            Some(blend) => self.surfaces.iter().any(|(b, _)| *b == blend),
            None => self.image.is_some(),
        }
    }
}

#[derive(Default)]
struct Wants {
    image: bool,
    blends: Vec<crate::shell::shader::Blend>,
}

impl Wants {
    fn add(&mut self, blend: Option<crate::shell::shader::Blend>) {
        match blend {
            Some(blend) if !self.blends.contains(&blend) => self.blends.push(blend),
            Some(_) => {}
            None => self.image = true,
        }
    }
}

/// Every slot's shader, and one clock for all, so two shaders at the same
/// rate draw in the same window frame.
#[derive(Default)]
struct Runs(HashMap<Slot, Run>, Option<Instant>);

impl gpui_kit::Global for Runs {}

/// The largest side a shader renders, in px.
const MAX_SHADER_SIDE: f32 = 4096.;
/// The frame rate while another app is in front.
const BACKGROUND_FPS: f32 = 2.;

fn rgba_of(color: Hsla) -> [f32; 4] {
    let c = color.to_rgb();
    [c.r, c.g, c.b, c.a]
}

/// Paints a slot's shader. With `blend`, the slot lies over one flat
/// color and the frame draws as a surface; without, as an image.
#[allow(clippy::too_many_arguments)]
fn paint_shader(
    slot: Slot,
    b: &Backdrop,
    spec: &ShaderSpec,
    area: Rect,
    over: bool,
    blend: Option<crate::shell::shader::Blend>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui_kit::component::ActiveTheme as _;
    let Some(program) = cx
        .try_global::<crate::shell::shader::Programs>()
        .and_then(|p| p.get(&spec.source))
    else {
        return;
    };
    let now = cx.background_executor().now();
    let factor = window.scale_factor() * spec.scale;
    let target = (
        (area.w * factor).clamp(1., MAX_SHADER_SIDE).round() as u32,
        (area.h * factor).clamp(1., MAX_SHADER_SIDE).round() as u32,
    );
    let theme = cx.theme();
    // The panel's own fill, so the shader starts from what it covers. A
    // translucent token, such as the Default theme's accent, arrives as
    // it shows on that fill: the shader reads only `.rgb` (LLM-583).
    let fill = match slot {
        Slot::Sidebar => crate::shell::theme::blend(theme.sidebar, theme.background),
        _ => theme.background,
    };
    let colors = (
        rgba_of(fill),
        rgba_of(crate::shell::theme::blend(theme.foreground, fill)),
        rgba_of(crate::shell::theme::blend(theme.accent, fill)),
    );
    let mouse = window.mouse_position();
    let animate = crate::shell::motion::on(cx);
    let fps = if !animate {
        0.
    } else if window.is_window_active() {
        spec.fps
    } else {
        spec.fps.min(BACKGROUND_FPS)
    };

    let runs = cx.default_global::<Runs>();
    let epoch = *runs.1.get_or_insert(now);
    let run = runs.0.entry(slot).or_insert_with(|| Run {
        program: program.clone(),
        started: now,
        frame: None,
        asked: None,
        size: target,
        pending: false,
        done: None,
        wants: Wants::default(),
    });
    let mut retired = Vec::new();
    if !Arc::ptr_eq(&run.program, &program) {
        // A new shader, or a reload: start its clock again.
        run.program = program.clone();
        run.started = now;
        run.asked = None;
    }
    if let Some((frame, made)) = run.done.take() {
        let old = run.frame.replace(frame);
        retired.extend(old.and_then(|f| f.image));
        run.size = made;
    }
    run.wants.add(blend);
    let interval = (fps > 0.).then(|| Duration::from_secs_f32(1. / fps));
    // Frames fall on ticks of the shared clock.
    let tick_of = |at: Instant, i: Duration| {
        (at.saturating_duration_since(epoch).as_secs_f64() / i.as_secs_f64()).floor() as u64
    };
    let missing = run.frame.as_ref().is_none_or(|f| !f.has(blend));
    // A surface over the panel shows the panel's frame. It asks for one
    // only when that frame lacks the form it needs.
    let due = !run.pending
        && (missing
            || (!over
                && (run.size != target
                    || interval.is_some_and(|i| {
                        run.asked.is_none_or(|a| tick_of(now, i) > tick_of(a, i))
                    }))));
    let frame = run.frame.clone();
    let next = interval.map(|i| epoch + i.mul_f64((tick_of(now, i) + 1) as f64));
    if due {
        run.pending = true;
        run.asked = Some(now);
        let wants = std::mem::take(&mut run.wants);
        let inputs = crate::shell::shader::Inputs {
            time: now.saturating_duration_since(run.started).as_secs_f32(),
            pad: 0.,
            resolution: [target.0 as f32, target.1 as f32],
            origin: [area.x * factor, area.y * factor],
            pointer: [
                (f32::from(mouse.x) - area.x) * factor,
                (f32::from(mouse.y) - area.y) * factor,
            ],
            background: colors.0,
            foreground: colors.1,
            accent: colors.2,
        };
        let view = window.current_view();
        let program = program.clone();
        let render = cx.background_executor().spawn(async move {
            program.render(target.0, target.1, &inputs, wants.image, &wants.blends)
        });
        cx.spawn(async move |cx| {
            let result = render.await;
            cx.update(|cx| {
                let runs = cx.default_global::<Runs>();
                let Some(run) = runs.0.get_mut(&slot) else {
                    return;
                };
                run.pending = false;
                match result {
                    Ok(rendered) => {
                        let image = rendered.pixels.and_then(|bgra| {
                            let buffer = image::RgbaImage::from_raw(target.0, target.1, bgra)?;
                            Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
                        });
                        let made = ShaderFrame {
                            image,
                            surfaces: rendered.surfaces,
                        };
                        // A frame shows at the next tick, which draws
                        // anyway. The first frame, one for a new size, or
                        // one with a form a paint lacked draws at once.
                        let now_please = run.size != target
                            || run.frame.as_ref().is_none_or(|old| {
                                (made.image.is_some() && old.image.is_none())
                                    || made.surfaces.iter().any(|(b, _)| !old.has(Some(*b)))
                            });
                        // A frame that was not shown yet gives its image
                        // back to the atlas.
                        if let Some((stale, _)) = run.done.replace((made, target))
                            && let Some(image) = stale.image
                        {
                            cx.default_global::<Loads>().retired.push(image);
                        }
                        if now_please {
                            cx.notify(view);
                        }
                    }
                    Err(e) => eprintln!("llmtodo: background shader: {e}"),
                }
            });
        })
        .detach();
    }
    cx.default_global::<Loads>().retired.extend(retired);
    if let Some(at) = next.filter(|_| !over) {
        wake_at(at.max(now + Duration::from_millis(1)), window, cx);
    }
    let Some(frame) = frame else { return };
    let loads = cx.default_global::<Loads>();
    loads.ready.entry(ident(b)).or_insert(now);
    // The frame stretches over the slot: it renders at `scale`.
    let image_bounds = Bounds::new(point(px(area.x), px(area.y)), size(px(area.w), px(area.h)));
    // The surface for this paint's color; else the image, which honors
    // opacity; else any surface, for the one frame until the right one
    // arrives.
    let surface = blend
        .and_then(|blend| frame.surfaces.iter().find(|(b, _)| *b == blend))
        .or_else(|| {
            frame
                .image
                .is_none()
                .then(|| frame.surfaces.first())
                .flatten()
        })
        .map(|(_, s)| s.clone());
    match (surface, frame.image) {
        (Some(surface), _) => paint_surface(surface, bounds, image_bounds, window),
        (None, Some(image)) => {
            let _ = window.paint_image(bounds, image_bounds, Default::default(), image, 0, false);
        }
        (None, None) => {}
    }
}

/// Draws a shader frame as a GPUI surface, clipped to the slot.
#[cfg(target_os = "macos")]
fn paint_surface(
    surface: crate::shell::shader::Surface,
    clip: Bounds<Pixels>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    window.with_content_mask(Some(gpui_kit::ContentMask { bounds: clip }), |window| {
        window.paint_surface(bounds, surface.0)
    });
}

#[cfg(not(target_os = "macos"))]
fn paint_surface(
    surface: crate::shell::shader::Surface,
    _: Bounds<Pixels>,
    _: Bounds<Pixels>,
    _: &mut Window,
) {
    match surface {}
}

fn rect_of(bounds: Bounds<Pixels>) -> Rect {
    Rect {
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        w: f32::from(bounds.size.width),
        h: f32::from(bounds.size.height),
    }
}
