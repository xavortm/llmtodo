//! Turns a background shader into Metal Shading Language (LLM-578).
//!
//! This is the one place that knows the shader languages, so a change of
//! language later touches only this file. A shader file holds one
//! function; this file wraps it with the inputs and an entry point, then
//! naga validates it and writes MSL.
//!
//! WGSL: `fn main_image(frag: vec2<f32>) -> vec4<f32>`, with the inputs
//! in `u`: `u.time`, `u.resolution`, `u.origin`, `u.pointer`,
//! `u.background`, `u.foreground` and `u.accent`. `frag` is in pixels from
//! the top left.
//!
//! GLSL, as on ShaderToy: `void mainImage(out vec4 fragColor, in vec2
//! fragCoord)` with `iTime`, `iResolution`, `iMouse`, and `iOrigin`,
//! `iBackground`, `iForeground` and `iAccent`. `fragCoord` starts at the
//! bottom left, as on ShaderToy.

use naga::back::msl;
use naga::valid::{Capabilities, ValidationFlags, Validator};

/// The language of a shader file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Wgsl,
    Glsl,
}

impl Lang {
    /// By file extension: `.glsl`, `.frag` and `.fs` are GLSL, the rest
    /// WGSL.
    pub fn of(path: &str) -> Self {
        let lower = path.to_ascii_lowercase();
        if [".glsl", ".frag", ".fs"].iter().any(|e| lower.ends_with(e)) {
            Self::Glsl
        } else {
            Self::Wgsl
        }
    }
}

/// A shader as MSL, with the name of its fragment function.
#[derive(Debug, Clone, PartialEq)]
pub struct Translated {
    pub msl: String,
    pub fragment: String,
}

/// Why a shader does not compile, with the line in the person's file.
#[derive(Debug, Clone, PartialEq)]
pub struct CompileError {
    pub line: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// The name of the vertex function `VERTEX_MSL` holds.
pub const VERTEX: &str = "llmtodo_bg_vertex";

/// One triangle that covers the target, appended to every shader.
const VERTEX_MSL: &str = r#"
struct LlmtodoBgVertexOut { float4 position [[position]]; };
vertex LlmtodoBgVertexOut llmtodo_bg_vertex(uint vid [[vertex_id]]) {
    float2 p = float2(float((vid << 1) & 2), float(vid & 2));
    LlmtodoBgVertexOut out;
    out.position = float4(p * 2.0 - 1.0, 0.0, 1.0);
    return out;
}
"#;

const WGSL_PRELUDE: &str = "struct Inputs {
    time: f32,
    _pad: f32,
    resolution: vec2<f32>,
    origin: vec2<f32>,
    pointer: vec2<f32>,
    background: vec4<f32>,
    foreground: vec4<f32>,
    accent: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Inputs;
";

const WGSL_ENTRY: &str = "
@fragment
fn llmtodo_bg_fragment(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return main_image(p.xy);
}
";

const GLSL_PRELUDE: &str = "#version 450
layout(set = 0, binding = 0) uniform Inputs {
    float u_time;
    float u_pad;
    vec2 u_resolution;
    vec2 u_origin;
    vec2 u_pointer;
    vec4 u_background;
    vec4 u_foreground;
    vec4 u_accent;
};
#define iTime u_time
#define iResolution vec3(u_resolution, 1.0)
#define iMouse vec4(u_pointer.x, u_resolution.y - u_pointer.y, 0.0, 0.0)
#define iOrigin u_origin
#define iBackground u_background
#define iForeground u_foreground
#define iAccent u_accent
layout(location = 0) out vec4 llmtodo_out;
";

const GLSL_ENTRY: &str = "
void main() {
    vec2 frag = vec2(gl_FragCoord.x, u_resolution.y - gl_FragCoord.y);
    mainImage(llmtodo_out, frag);
}
";

/// The inputs as the shader reads them, 80 bytes, laid out as the
/// preludes declare them.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Inputs {
    pub time: f32,
    pub pad: f32,
    pub resolution: [f32; 2],
    pub origin: [f32; 2],
    pub pointer: [f32; 2],
    pub background: [f32; 4],
    pub foreground: [f32; 4],
    pub accent: [f32; 4],
}

/// Wraps, validates and translates `source`.
pub fn translate(source: &str, lang: Lang) -> Result<Translated, CompileError> {
    let (prelude, entry) = match lang {
        Lang::Wgsl => (WGSL_PRELUDE, WGSL_ENTRY),
        Lang::Glsl => (GLSL_PRELUDE, GLSL_ENTRY),
    };
    let full = format!("{prelude}{source}\n{entry}");
    let offset = prelude.lines().count();
    let user_lines = source.lines().count();
    // A line in the prelude or the entry point is not the person's line.
    let line_of = |byte: usize| -> Option<usize> {
        let line = full[..byte.min(full.len())].matches('\n').count() + 1;
        (line > offset && line <= offset + user_lines).then(|| line - offset)
    };
    let module = match lang {
        Lang::Wgsl => naga::front::wgsl::parse_str(&full).map_err(|e| CompileError {
            line: e
                .labels()
                .next()
                .and_then(|(span, _)| span.to_range())
                .and_then(|r| line_of(r.start)),
            message: e.message().to_string(),
        })?,
        Lang::Glsl => {
            let mut front = naga::front::glsl::Frontend::default();
            front
                .parse(
                    &naga::front::glsl::Options::from(naga::ShaderStage::Fragment),
                    &full,
                )
                .map_err(|e| {
                    let first = e.errors.first();
                    CompileError {
                        line: first
                            .and_then(|e| e.meta.to_range())
                            .and_then(|r| line_of(r.start)),
                        message: first
                            .map(|e| e.kind.to_string())
                            .unwrap_or_else(|| "does not parse".into()),
                    }
                })?
        }
    };
    let info = Validator::new(ValidationFlags::all(), Capabilities::empty())
        .validate(&module)
        .map_err(|e| CompileError {
            line: e
                .spans()
                .next()
                .and_then(|(span, _)| span.to_range())
                .and_then(|r| line_of(r.start)),
            message: e.as_inner().to_string(),
        })?;
    let entry_name = match lang {
        Lang::Wgsl => "llmtodo_bg_fragment",
        Lang::Glsl => "main",
    };
    let mut resources = msl::EntryPointResources::default();
    resources.resources.insert(
        naga::ResourceBinding {
            group: 0,
            binding: 0,
        },
        msl::BindTarget {
            buffer: Some(0),
            ..Default::default()
        },
    );
    let options = msl::Options {
        lang_version: (2, 4),
        per_entry_point_map: [(entry_name.to_string(), resources)].into_iter().collect(),
        fake_missing_bindings: false,
        ..Default::default()
    };
    let pipeline = msl::PipelineOptions {
        entry_point: Some((naga::ShaderStage::Fragment, entry_name.to_string())),
        ..Default::default()
    };
    let (msl, info) =
        msl::write_string(&module, &info, &options, &pipeline).map_err(|e| CompileError {
            line: None,
            message: e.to_string(),
        })?;
    let fragment = info
        .entry_point_names
        .into_iter()
        .next()
        .and_then(Result::ok)
        .ok_or_else(|| CompileError {
            line: None,
            message: "no fragment entry point".into(),
        })?;
    Ok(Translated {
        msl: format!("{msl}\n{VERTEX_MSL}"),
        fragment,
    })
}
