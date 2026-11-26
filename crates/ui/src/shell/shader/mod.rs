//! Shader backgrounds (LLM-578): a slot can run a fragment shader in
//! place of an image. `translate` turns WGSL or GLSL into Metal Shading
//! Language with naga, and `metal` runs it on our own pipeline. The
//! background layer in `backdrop.rs` paints each frame as a GPUI surface
//! where it can, and as an image where it must (LLM-579).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

pub mod translate;

#[cfg(target_os = "macos")]
mod metal;

pub use translate::{CompileError, Inputs, Lang};

/// The shaders that ship with the app: `shader = "builtin:<name>"`.
pub const BUILTINS: [(&str, &str); 6] = [
    ("aurora", include_str!("builtin/aurora.wgsl")),
    ("grain", include_str!("builtin/grain.wgsl")),
    ("waves", include_str!("builtin/waves.wgsl")),
    ("silk", include_str!("builtin/silk.wgsl")),
    ("lights", include_str!("builtin/lights.wgsl")),
    ("metal", include_str!("builtin/metal.wgsl")),
];

/// The source of a built-in shader by name.
pub fn builtin(name: &str) -> Option<&'static str> {
    BUILTINS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, s)| *s)
}

/// Where a shader comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    Builtin(String),
    File(PathBuf),
}

impl Source {
    /// `builtin:<name>`, or a path resolved as image paths are.
    pub fn parse(text: &str, base: Option<&Path>) -> Self {
        match text.trim().strip_prefix("builtin:") {
            Some(name) => Self::Builtin(name.trim().to_ascii_lowercase()),
            None => Self::File(crate::shell::backdrop::resolve_path(text.trim(), base)),
        }
    }

    /// The name the status bar uses.
    pub fn label(&self) -> String {
        match self {
            Self::Builtin(name) => format!("builtin:{name}"),
            Self::File(path) => path.display().to_string(),
        }
    }

    /// The text and language. A missing built-in or file is an error.
    pub fn read(&self) -> Result<(String, Lang), CompileError> {
        let fail = |message: String| CompileError {
            line: None,
            message,
        };
        match self {
            Self::Builtin(name) => builtin(name)
                .map(|s| (s.to_string(), Lang::Wgsl))
                .ok_or_else(|| fail(format!("no built-in shader \"{name}\""))),
            Self::File(path) => std::fs::read_to_string(path)
                .map(|s| (s, Lang::of(&path.to_string_lossy())))
                .map_err(|e| fail(format!("{}: {e}", path.display()))),
        }
    }
}

/// How a frame is laid over what lies under its slot, for a surface
/// (LLM-579). A surface is opaque, so the frame is blended with `under`
/// at `amount`, the slot's opacity, before GPUI draws it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blend {
    /// The flat color under the slot, as sRGB-encoded RGB.
    pub under: [f32; 3],
    pub amount: f32,
}

/// A frame as an IOSurface-backed YCbCr pixel buffer, for
/// `Window::paint_surface`.
#[cfg(target_os = "macos")]
#[derive(Clone)]
pub struct Surface(pub objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>);

// SAFETY: a CVPixelBuffer is reference counted with atomics, and nothing
// writes to it once the GPU has drawn the frame.
#[cfg(target_os = "macos")]
unsafe impl Send for Surface {}
#[cfg(target_os = "macos")]
unsafe impl Sync for Surface {}

/// Other platforms have no surfaces.
#[cfg(not(target_os = "macos"))]
#[derive(Clone)]
pub enum Surface {}

/// One rendered frame: the BGRA bytes, row by row, when asked for, and a
/// surface for each blend asked for.
pub struct Frame {
    pub pixels: Option<Vec<u8>>,
    pub surfaces: Vec<(Blend, Surface)>,
}

/// What one slot's frames draw into: the frame texture and the pool its
/// surfaces come from. Each slot keeps its own, so slots of different
/// sizes that share a [`Program`] keep their buffers (LLM-585).
#[derive(Default)]
pub struct Target {
    #[cfg(target_os = "macos")]
    inner: metal::Target,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Target")
    }
}

#[cfg(all(test, target_os = "macos"))]
impl Target {
    /// The frame texture's size, and the pool and the texture cache as
    /// addresses, so a test can tell which of them were made again.
    pub fn ids(&self) -> (Option<(u32, u32)>, Option<usize>, Option<usize>) {
        self.inner.ids()
    }
}

/// A compiled shader, ready to render frames.
#[derive(Debug)]
pub struct Program {
    #[cfg(target_os = "macos")]
    pipeline: metal::Pipeline,
}

impl Program {
    /// Renders a frame: as bytes with `pixels`, and as one surface per
    /// blend, into `target`. The shader runs once for all of them.
    /// Blocks on the GPU.
    pub fn render(
        &self,
        target: &mut Target,
        width: u32,
        height: u32,
        inputs: &Inputs,
        pixels: bool,
        blends: &[Blend],
    ) -> Result<Frame, String> {
        #[cfg(target_os = "macos")]
        {
            self.pipeline
                .render(&mut target.inner, width, height, inputs, pixels, blends)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (target, width, height, inputs, pixels, blends);
            Err("shader backgrounds run only on macOS".into())
        }
    }
}

/// Translates and compiles `text`.
pub fn compile(text: &str, lang: Lang) -> Result<Program, CompileError> {
    let translated = translate::translate(text, lang)?;
    #[cfg(target_os = "macos")]
    {
        let pipeline = metal::compile(&translated).map_err(|message| CompileError {
            line: None,
            message,
        })?;
        Ok(Program { pipeline })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = translated;
        Err(CompileError {
            line: None,
            message: "shader backgrounds run only on macOS".into(),
        })
    }
}

/// The compiled shaders, as a global. A shader that stops compiling keeps
/// its last good program, so the slot keeps drawing.
#[derive(Default)]
pub struct Programs {
    entries: HashMap<Source, Entry>,
}

impl gpui_kit::Global for Programs {}

struct Entry {
    stamp: Option<SystemTime>,
    program: Option<Arc<Program>>,
}

impl Programs {
    /// The program to run for `source`: the last one that compiled.
    pub fn get(&self, source: &Source) -> Option<Arc<Program>> {
        self.entries.get(source).and_then(|e| e.program.clone())
    }

    /// Compiles `source` when it is new or its file changed. Returns the
    /// error, if any, once per change.
    pub fn prepare(&mut self, source: &Source, stamp: Option<SystemTime>) -> Option<CompileError> {
        if let Some(e) = self.entries.get(source)
            && e.stamp == stamp
        {
            return None;
        }
        let result = source.read().and_then(|(text, lang)| compile(&text, lang));
        let entry = self.entries.entry(source.clone()).or_insert(Entry {
            stamp,
            program: None,
        });
        entry.stamp = stamp;
        match result {
            Ok(program) => {
                entry.program = Some(Arc::new(program));
                None
            }
            Err(e) => Some(e),
        }
    }
}
