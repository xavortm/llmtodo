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

/// A frame's BGRA bytes, read back.
#[cfg(target_os = "macos")]
fn pixels(program: &shader::Program, w: u32, h: u32) -> Vec<u8> {
    let frame = program
        .render(
            &mut shader::Target::default(),
            w,
            h,
            &inputs(w as f32, h as f32),
            true,
            &[],
        )
        .unwrap();
    assert!(frame.surfaces.is_empty());
    frame.pixels.unwrap()
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
    let bytes = pixels(&program, 64, 8);
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
        let bytes = pixels(&program, 32, 32);
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
    let bytes = pixels(&program, 4, 16);
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

/// LLM-579: the Y, Cb and Cr of a surface's pixel at `(x, y)`.
#[cfg(target_os = "macos")]
fn ycbcr(surface: &shader::Surface, x: usize, y: usize) -> (u8, u8, u8) {
    use objc2_core_video::{
        CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane,
        CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    };
    let buffer = &surface.0;
    // SAFETY: the buffer is locked for reading while the planes are read,
    // and each offset lies inside its plane.
    unsafe {
        CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly);
        let luma = CVPixelBufferGetBaseAddressOfPlane(buffer, 0).cast::<u8>();
        let chroma = CVPixelBufferGetBaseAddressOfPlane(buffer, 1).cast::<u8>();
        let luma_row = CVPixelBufferGetBytesPerRowOfPlane(buffer, 0);
        let chroma_row = CVPixelBufferGetBytesPerRowOfPlane(buffer, 1);
        let y_ = *luma.add(y * luma_row + x);
        let c = chroma.add((y / 2) * chroma_row + (x / 2) * 2);
        let out = (y_, *c, *c.add(1));
        CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly);
        out
    }
}

/// What GPUI's `surface_fragment` turns Y, Cb and Cr back into, 0 to 255.
#[cfg(target_os = "macos")]
fn rgb_of((y, cb, cr): (u8, u8, u8)) -> [f32; 3] {
    let (y, cb, cr) = (
        y as f32 / 255.,
        cb as f32 / 255. - 0.5,
        cr as f32 / 255. - 0.5,
    );
    [
        (y + 1.402 * cr) * 255.,
        (y - 0.3441 * cb - 0.7141 * cr) * 255.,
        (y + 1.772 * cb) * 255.,
    ]
}

#[cfg(target_os = "macos")]
fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() <= 3.)
}

/// LLM-579: a frame goes out as a YCbCr surface that GPUI turns back into
/// the shader's color, with no read back.
#[cfg(target_os = "macos")]
#[test]
fn a_surface_holds_the_shaders_color() {
    use objc2_core_video::{
        CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
        kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    };
    let text = "fn main_image(frag: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(0.8, 0.4, 0.2, 1.0);
}";
    let program = shader::compile(text, Lang::Wgsl).unwrap();
    let opaque = shader::Blend {
        under: [0., 0., 1.],
        amount: 1.,
    };
    let half = shader::Blend {
        under: [0., 0., 1.],
        amount: 0.5,
    };
    let frame = program
        .render(
            &mut shader::Target::default(),
            30,
            17,
            &inputs(30., 17.),
            false,
            &[opaque, half],
        )
        .unwrap();
    assert!(frame.pixels.is_none(), "no read back was asked for");
    assert_eq!(frame.surfaces.len(), 2);
    let (blend, surface) = &frame.surfaces[0];
    assert_eq!(*blend, opaque);
    assert_eq!(CVPixelBufferGetWidth(&surface.0), 30);
    assert_eq!(CVPixelBufferGetHeight(&surface.0), 17);
    assert_eq!(
        CVPixelBufferGetPixelFormatType(&surface.0),
        kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
    );
    for (x, y) in [(0, 0), (29, 16), (15, 8)] {
        let rgb = rgb_of(ycbcr(surface, x, y));
        assert!(near(rgb, [204., 102., 51.]), "({x}, {y}): {rgb:?}");
    }
    // At half opacity, half the blue under it shows through.
    let rgb = rgb_of(ycbcr(&frame.surfaces[1].1, 4, 4));
    assert!(near(rgb, [102., 51., 153.]), "{rgb:?}");
}

/// LLM-579: one render gives the image and the surfaces from the same
/// frame, and a surface keeps the frame's top at the top.
#[cfg(target_os = "macos")]
#[test]
fn a_surface_keeps_the_frame_upright() {
    let text = "fn main_image(frag: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(select(0.0, 1.0, frag.y < u.resolution.y * 0.5), 0.0, 0.0, 1.0);
}";
    let program = shader::compile(text, Lang::Wgsl).unwrap();
    let blend = shader::Blend {
        under: [0., 0., 0.],
        amount: 1.,
    };
    let frame = program
        .render(
            &mut shader::Target::default(),
            8,
            16,
            &inputs(8., 16.),
            true,
            &[blend],
        )
        .unwrap();
    let bytes = frame.pixels.unwrap();
    assert_eq!(bytes[2], 255, "the image's top row is red");
    let surface = &frame.surfaces[0].1;
    let top = rgb_of(ycbcr(surface, 2, 1));
    let bottom = rgb_of(ycbcr(surface, 2, 14));
    assert!(near(top, [255., 0., 0.]), "top {top:?}");
    assert!(near(bottom, [0., 0., 0.]), "bottom {bottom:?}");
}

/// A shader of one flat color, for the LLM-585 tests.
#[cfg(target_os = "macos")]
fn flat(rgba: [f32; 4]) -> shader::Program {
    let [r, g, b, a] = rgba;
    let text = format!(
        "fn main_image(frag: vec2<f32>) -> vec4<f32> {{\n    return vec4<f32>({r:?}, {g:?}, {b:?}, {a:?});\n}}"
    );
    shader::compile(&text, Lang::Wgsl).unwrap()
}

/// LLM-585: a slot's target makes a new texture and pool when its size
/// changes, keeps them while the size stays, and keeps its texture cache
/// throughout.
#[cfg(target_os = "macos")]
#[test]
fn a_target_follows_its_size_and_keeps_its_cache() {
    use objc2_core_video::{CVPixelBufferGetHeight, CVPixelBufferGetWidth};
    let program = flat([0.8, 0.4, 0.2, 1.]);
    let blend = shader::Blend {
        under: [0., 0., 1.],
        amount: 1.,
    };
    let mut target = shader::Target::default();
    let render = |target: &mut shader::Target, w: u32, h: u32| {
        program
            .render(target, w, h, &inputs(w as f32, h as f32), false, &[blend])
            .unwrap()
    };
    let big = render(&mut target, 30, 17);
    let (size, pool, cache) = target.ids();
    assert_eq!(size, Some((30, 17)));
    assert!(pool.is_some() && cache.is_some());

    let small = render(&mut target, 8, 8);
    let surface = &small.surfaces[0].1;
    assert_eq!(
        (
            CVPixelBufferGetWidth(&surface.0),
            CVPixelBufferGetHeight(&surface.0)
        ),
        (8, 8),
        "a new size takes buffers from a new pool"
    );
    let (size, small_pool, small_cache) = target.ids();
    assert_eq!(size, Some((8, 8)));
    assert_eq!(small_cache, cache, "the texture cache stays");
    assert!(near(rgb_of(ycbcr(surface, 4, 4)), [204., 102., 51.]));

    // The same size again keeps the pool.
    let again = render(&mut target, 8, 8);
    assert_eq!(target.ids(), (Some((8, 8)), small_pool, small_cache));
    // The first frame's buffer still holds its picture.
    assert!(near(
        rgb_of(ycbcr(&big.surfaces[0].1, 29, 16)),
        [204., 102., 51.]
    ));
    drop(again);
}

/// LLM-585: a render that asks for nothing does no GPU work.
#[cfg(target_os = "macos")]
#[test]
fn a_render_that_asks_for_nothing_makes_nothing() {
    let mut target = shader::Target::default();
    let frame = flat([1., 0., 0., 1.])
        .render(&mut target, 16, 16, &inputs(16., 16.), false, &[])
        .unwrap();
    assert!(frame.pixels.is_none() && frame.surfaces.is_empty());
    assert_eq!(target.ids(), (None, None, None));
}

/// LLM-585: a 1 × 1 slot still renders an image and a surface.
#[cfg(target_os = "macos")]
#[test]
fn a_one_pixel_frame_renders() {
    use objc2_core_video::{CVPixelBufferGetHeight, CVPixelBufferGetWidth};
    let blend = shader::Blend {
        under: [0., 0., 0.],
        amount: 1.,
    };
    let frame = flat([0.8, 0.4, 0.2, 1.])
        .render(
            &mut shader::Target::default(),
            1,
            1,
            &inputs(1., 1.),
            true,
            &[blend],
        )
        .unwrap();
    let bytes = frame.pixels.unwrap();
    assert_eq!(bytes.len(), 4);
    assert_eq!(bytes[3], 255);
    let surface = &frame.surfaces[0].1;
    assert_eq!(
        (
            CVPixelBufferGetWidth(&surface.0),
            CVPixelBufferGetHeight(&surface.0)
        ),
        (1, 1)
    );
    let rgb = rgb_of(ycbcr(surface, 0, 0));
    assert!(near(rgb, [204., 102., 51.]), "{rgb:?}");
}

/// LLM-585: a shader's own alpha and the slot's opacity multiply in the
/// blend: half-clear red at half opacity over blue shows a quarter red.
#[cfg(target_os = "macos")]
#[test]
fn a_translucent_shader_blends_at_its_alpha_times_the_opacity() {
    let blend = shader::Blend {
        under: [0., 0., 1.],
        amount: 0.5,
    };
    let frame = flat([1., 0., 0., 0.5])
        .render(
            &mut shader::Target::default(),
            8,
            8,
            &inputs(8., 8.),
            false,
            &[blend],
        )
        .unwrap();
    let rgb = rgb_of(ycbcr(&frame.surfaces[0].1, 3, 3));
    assert!(near(rgb, [63.75, 0., 191.25]), "{rgb:?}");
}
