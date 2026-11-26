// Metal: brushed metal in the theme's colors, with a soft sheen that
// sweeps slowly across it. Made for the sidebar. A llmtodo background
// shader. Copy it out with `:shader copy metal`.
//
// Inputs, in `u`: time (s), resolution (px), origin (px in the window),
// pointer (px in this slot), background, foreground and accent (RGBA).

fn hash(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(123.34, 456.21));
    let r = q + dot(q, q + 45.32);
    return fract(r.x * r.y);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let s = f * f * (3.0 - 2.0 * f);
    let a = hash(i);
    let b = hash(i + vec2<f32>(1.0, 0.0));
    let c = hash(i + vec2<f32>(0.0, 1.0));
    let d = hash(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, s.x), mix(c, d, s.x), s.y);
}

fn dither(frag: vec2<f32>) -> f32 {
    return fract(sin(dot(frag, vec2<f32>(12.9898, 78.233))) * 43758.547) - 0.5;
}

fn main_image(frag: vec2<f32>) -> vec4<f32> {
    let p = frag / max(u.resolution.y, 1.0);
    let t = u.time * 0.04;

    // Fine horizontal grain, stretched along x as a brush leaves it.
    let grain = noise(vec2<f32>(p.x * 4.0, p.y * 160.0)) * 0.6
        + noise(vec2<f32>(p.x * 9.0, p.y * 420.0)) * 0.4;

    // Two broad diagonal bands of sheen that sway up and down.
    let slope = p.y + p.x * 0.5;
    let a = (slope - (0.45 + 0.55 * sin(t * 1.3))) / 0.28;
    let b = (slope - (0.55 + 0.6 * sin(t * 0.8 + 2.4))) / 0.16;
    let sheen = exp(-a * a) * 0.8 + exp(-b * b) * 0.45;

    let bg = u.background.rgb;
    let fg = u.foreground.rgb;
    let lit = mix(fg, u.accent.rgb, 0.3);
    // Lighter at the top, as metal under a ceiling light.
    var color = mix(bg, fg, 0.015 + (1.0 - p.y) * 0.04);
    color = color + (fg - bg) * (grain - 0.5) * 0.025;
    color = color + (lit - bg) * sheen * 0.1;
    color = color + dither(frag) / 255.0;
    return vec4<f32>(color, 1.0);
}
