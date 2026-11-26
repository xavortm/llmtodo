//! LLM-578: shader backgrounds translate, compile and render.

use super::shader::{self, Inputs, Lang, translate};

fn inputs(w: f32, h: f32) -> Inputs {
    Inputs {
        time: 1.5,
        resolution: [w, h],
        background: [0.1, 0.1, 0.12, 1.],
        foreground: [0.9, 0.9, 0.9, 1.],
        accent: [0.4, 0.5, 1., 1.],
        ..Default::default()
    }
}

#[test]
fn every_builtin_translates() {
    for (name, text) in shader::BUILTINS {
        translate::translate(text, Lang::Wgsl).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn a_shadertoy_shader_translates_from_glsl() {
    let text = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    fragColor = vec4(uv, 0.5 + 0.5 * sin(iTime), 1.0);
}";
    let t = translate::translate(text, Lang::Glsl).unwrap();
    assert!(t.msl.contains(&t.fragment));
}

#[test]
fn an_error_names_the_line_in_the_file() {
    let text = "fn main_image(frag: vec2<f32>) -> vec4<f32> {\n    let x = 1.0;\n    return vec4<f32>(x, nope, 0.0, 1.0);\n}";
    let e = translate::translate(text, Lang::Wgsl).unwrap_err();
    assert_eq!(e.line, Some(3), "{e}");
}

#[test]
fn the_language_follows_the_extension() {
    assert_eq!(Lang::of("a.glsl"), Lang::Glsl);
    assert_eq!(Lang::of("a.FRAG"), Lang::Glsl);
    assert_eq!(Lang::of("a.wgsl"), Lang::Wgsl);
}

/// The spike: one WGSL shader through naga, Metal and a read back.
#[cfg(target_os = "macos")]
#[test]
fn a_shader_renders_on_the_gpu() {
    let text = "fn main_image(frag: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(frag.x / u.resolution.x, 0.0, 1.0, 1.0);
}";
    let program = shader::compile(text, Lang::Wgsl).unwrap();
    let bytes = program.render(64, 8, &inputs(64., 8.)).unwrap();
    assert_eq!(bytes.len(), 64 * 8 * 4);
    // BGRA: blue full, red rises from left to right.
    let px = |x: usize| &bytes[x * 4..x * 4 + 4];
    assert_eq!(px(0)[0], 255);
    assert!(px(0)[2] < 10, "{:?}", px(0));
    assert!(px(63)[2] > 240, "{:?}", px(63));
    assert_eq!(px(0)[3], 255);
}

#[cfg(target_os = "macos")]
#[test]
fn every_builtin_renders() {
    for (name, text) in shader::BUILTINS {
        let program = shader::compile(text, Lang::Wgsl).unwrap_or_else(|e| panic!("{name}: {e}"));
        let bytes = program.render(32, 32, &inputs(32., 32.)).unwrap();
        assert!(bytes.chunks(4).all(|p| p[3] == 255), "{name} is opaque");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn a_glsl_shader_renders_with_shadertoy_coordinates() {
    // ShaderToy's fragCoord starts at the bottom: the top row is dark.
    let text =
        "void mainImage(out vec4 c, in vec2 p) { c = vec4(p.y / iResolution.y, 0.0, 0.0, 1.0); }";
    let program = shader::compile(text, Lang::Glsl).unwrap();
    let bytes = program.render(4, 16, &inputs(4., 16.)).unwrap();
    let top_red = bytes[2];
    let bottom_red = bytes[(15 * 4) * 4 + 2];
    assert!(top_red > bottom_red, "top {top_red}, bottom {bottom_red}");
}

#[test]
fn a_broken_change_keeps_the_last_good_program() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.wgsl");
    std::fs::write(&path, shader::BUILTINS[1].1).unwrap();
    let source = shader::Source::File(path.clone());
    let mut programs = shader::Programs::default();
    let t0 = std::time::SystemTime::UNIX_EPOCH;
    assert_eq!(programs.prepare(&source, Some(t0)), None);
    let good = programs.get(&source);
    assert!(good.is_some());
    std::fs::write(&path, "fn main_image(").unwrap();
    let later = t0 + std::time::Duration::from_secs(1);
    assert!(programs.prepare(&source, Some(later)).is_some());
    assert!(std::sync::Arc::ptr_eq(
        &programs.get(&source).unwrap(),
        &good.unwrap()
    ));
    // The same broken file is not compiled or named again.
    assert_eq!(programs.prepare(&source, Some(later)), None);
}
