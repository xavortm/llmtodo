//! The mascot's art, read once from `assets/mascot/mascot.json`.
//! `design/mascot/export.mjs --app` writes that file from
//! `design/mascot/sprites.js`, so the art has one source.
//!
//! Each frame becomes runs: one run per stretch of the same colour in a row.
//! The view paints one quad per run, so the cat stays sharp at any scale.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;

/// The animations the app reads from the data file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnimId {
    Watch,
    Laptop,
    Sleep,
    WalkRight,
    WalkLeft,
    /// The sleeping cat's head snaps up: one frame, held 1 s.
    Jolt,
    /// From the jolt, it jumps up and lands sitting.
    Jump,
    /// The jolt and the jump in one go.
    Startle,
    /// From sitting into sleep, once.
    LieDown,
    /// At work, after a click: it stops typing, with a "?" over its head.
    Puzzled,
    /// At work, after more clicks: it glares, with a pulsing anger mark.
    Angry,
    /// At work, after the anger: it jumps up, once.
    WorkStartle,
}

impl AnimId {
    pub const ALL: [AnimId; 12] = [
        AnimId::Watch,
        AnimId::Laptop,
        AnimId::Sleep,
        AnimId::WalkRight,
        AnimId::WalkLeft,
        AnimId::Jolt,
        AnimId::Jump,
        AnimId::Startle,
        AnimId::LieDown,
        AnimId::Puzzled,
        AnimId::Angry,
        AnimId::WorkStartle,
    ];

    fn file_name(self) -> &'static str {
        match self {
            AnimId::Watch => "watch",
            AnimId::Laptop => "laptop",
            AnimId::Sleep => "sleep",
            AnimId::WalkRight => "walkRight",
            AnimId::WalkLeft => "walkLeft",
            AnimId::Jolt => "jolt",
            AnimId::Jump => "jump",
            AnimId::Startle => "startle",
            AnimId::LieDown => "lieDown",
            AnimId::Puzzled => "puzzled",
            AnimId::Angry => "angry",
            AnimId::WorkStartle => "workStartle",
        }
    }
}

/// One stretch of pixels in one row, all in one colour. `key` indexes the
/// palettes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub x: u8,
    pub y: u8,
    pub len: u8,
    pub key: u8,
}

/// One step of a timeline: show `frame` for `ms` milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Step {
    #[serde(rename = "i")]
    pub frame: usize,
    pub ms: u64,
}

#[derive(Debug)]
pub struct Anim {
    pub frames: Vec<Vec<Run>>,
    pub timeline: Vec<Step>,
}

/// The art: every animation, the two chosen palettes, and the columns the
/// cat can cover in any frame.
#[derive(Debug)]
pub struct Sprites {
    /// The width and the height of a frame, in sprite pixels.
    pub size: u8,
    anims: HashMap<AnimId, Anim>,
    /// Colours as `0xRRGGBB`, indexed by `Run::key`.
    light: Vec<u32>,
    dark: Vec<u32>,
    /// The watch step where the happy eyes start. A click jumps there.
    pub watch_happy: usize,
    /// The click timings, from `RULES` in `design/mascot/sprites.js`.
    pub rules: Rules,
    /// The first and the last column any frame paints.
    pub cols: (u8, u8),
}

/// How long the cat's click reactions last, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rules {
    /// How long a jolt lasts. A click during it makes the cat jump.
    pub jolt_ms: u64,
    /// Clicks in this first part of a jolt do nothing.
    pub jolt_lock_ms: u64,
    /// After a jolt with no jump, a click within this time startles the
    /// cat at once.
    pub drowsy_ms: u64,
    /// At work: clicks in this first part of the puzzled look do nothing.
    /// A later click makes the cat angry.
    pub puzzled_lock_ms: u64,
    /// Clicks in this first part of the anger do nothing. A later click
    /// startles the cat, and it moves to the other side.
    pub angry_lock_ms: u64,
    /// With no click for this long, a puzzled or angry cat types again.
    pub calm_ms: u64,
}

#[derive(Deserialize)]
struct File {
    size: u8,
    palettes: HashMap<String, HashMap<char, String>>,
    cues: Cues,
    rules: Rules,
    animations: HashMap<String, FileAnim>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cues {
    watch_happy: usize,
}

#[derive(Deserialize)]
struct FileAnim {
    frames: Vec<Vec<String>>,
    timeline: Vec<Step>,
}

static DATA: &str = include_str!("../../../assets/mascot/mascot.json");

static SPRITES: LazyLock<Sprites> =
    LazyLock::new(|| Sprites::parse(DATA).expect("assets/mascot/mascot.json is valid"));

/// The art, parsed on first use.
pub fn sprites() -> &'static Sprites {
    &SPRITES
}

fn hex(colour: &str) -> Result<u32, String> {
    colour
        .strip_prefix('#')
        .and_then(|h| u32::from_str_radix(h, 16).ok())
        .ok_or_else(|| format!("bad colour {colour}"))
}

impl Sprites {
    pub fn parse(json: &str) -> Result<Self, String> {
        let file: File = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let palette = |theme: &str| {
            file.palettes
                .get(theme)
                .ok_or_else(|| format!("no {theme} palette"))
        };
        let (light_file, dark_file) = (palette("light")?, palette("dark")?);
        // One key list for both palettes, in a fixed order.
        let mut keys: Vec<char> = light_file.keys().copied().collect();
        keys.sort_unstable();
        let colours = |file: &HashMap<char, String>| {
            keys.iter()
                .map(|k| hex(file.get(k).ok_or_else(|| format!("no colour for {k}"))?))
                .collect::<Result<Vec<u32>, String>>()
        };
        let (light, dark) = (colours(light_file)?, colours(dark_file)?);
        let size = file.size;
        let mut cols = (u8::MAX, 0u8);
        let mut anims = HashMap::new();
        for id in AnimId::ALL {
            let source = file
                .animations
                .get(id.file_name())
                .ok_or_else(|| format!("no {} animation", id.file_name()))?;
            let mut frames = Vec::with_capacity(source.frames.len());
            for rows in &source.frames {
                if rows.len() != usize::from(size) {
                    return Err(format!(
                        "{} has a frame of the wrong height",
                        id.file_name()
                    ));
                }
                let mut runs: Vec<Run> = Vec::new();
                for (y, row) in rows.iter().enumerate() {
                    let row: Vec<char> = row.chars().collect();
                    if row.len() != usize::from(size) {
                        return Err(format!("{} has a row of the wrong width", id.file_name()));
                    }
                    let mut x = 0;
                    while x < row.len() {
                        let c = row[x];
                        let start = x;
                        while x < row.len() && row[x] == c {
                            x += 1;
                        }
                        if c == '.' {
                            continue;
                        }
                        let key = keys
                            .iter()
                            .position(|k| *k == c)
                            .ok_or_else(|| format!("no colour for {c}"))?;
                        // The size is 32, so every value fits in a byte.
                        let run = Run {
                            x: start as u8,
                            y: y as u8,
                            len: (x - start) as u8,
                            key: key as u8,
                        };
                        cols = (cols.0.min(run.x), cols.1.max(run.x + run.len - 1));
                        runs.push(run);
                    }
                }
                frames.push(runs);
            }
            if source.timeline.is_empty()
                || source
                    .timeline
                    .iter()
                    .any(|s| s.frame >= frames.len() || s.ms == 0)
            {
                return Err(format!("{} has a bad timeline", id.file_name()));
            }
            anims.insert(
                id,
                Anim {
                    frames,
                    timeline: source.timeline.clone(),
                },
            );
        }
        if file.cues.watch_happy >= anims[&AnimId::Watch].timeline.len() {
            return Err("the watch cue is past the timeline".into());
        }
        let rules = file.rules;
        if rules.jolt_lock_ms == 0 || rules.jolt_lock_ms >= rules.jolt_ms {
            return Err("the jolt lock must be inside the jolt".into());
        }
        if rules.calm_ms == 0 {
            return Err("the calm-down time must not be zero".into());
        }
        Ok(Self {
            size,
            anims,
            light,
            dark,
            watch_happy: file.cues.watch_happy,
            rules,
            cols,
        })
    }

    pub fn anim(&self, id: AnimId) -> &Anim {
        &self.anims[&id]
    }

    /// The runs to paint for one step of one animation.
    pub fn frame(&self, id: AnimId, step: usize) -> &[Run] {
        let anim = self.anim(id);
        &anim.frames[anim.timeline[step % anim.timeline.len()].frame]
    }

    /// The colours, as `0xRRGGBB` by `Run::key`: the black cat for a light
    /// theme, the white cat for a dark one.
    pub fn palette(&self, dark: bool) -> &[u32] {
        if dark { &self.dark } else { &self.light }
    }
}
