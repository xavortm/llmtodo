// Silk: soft satin folds, lit by a light that drifts slowly across them.
// Made for the sidebar. A llmtodo background shader. Copy it out with
// `:shader copy silk`.
//
// Inputs, in `u`: time (s), resolution (px), origin (px in the window),
// pointer (px in this slot), background, foreground and accent (RGBA).

// The height of the cloth: two warps of a slow wave.
fn fold(p: vec2<f32>, t: f32) -> f32 {
    var q = p;
    q = q + 0.45 * vec2<f32>(sin(q.y * 1.3 + t * 0.9), sin(q.x * 1.1 - t * 0.7));
    q = q + 0.25 * vec2<f32>(sin(q.y * 2.3 - t * 0.6 + 1.3), sin(q.x * 2.1 + t * 0.8 + 0.7));
    return sin(q.x * 2.4 + q.y * 1.1 + t * 0.4);
}

// A tiny offset per pixel, so smooth gradients do not band.
fn dither(frag: vec2<f32>) -> f32 {
    return fract(sin(dot(frag, vec2<f32>(12.9898, 78.233))) * 43758.547) - 0.5;
}

fn main_image(frag: vec2<f32>) -> vec4<f32> {
    let p = frag / max(u.resolution.y, 1.0) * 5.0;
    let t = u.time * 0.06;
    let e = 0.02;
    let h = fold(p, t);
    let dx = (fold(p + vec2<f32>(e, 0.0), t) - h) / e;
    let dy = (fold(p + vec2<f32>(0.0, e), t) - h) / e;
    let n = normalize(vec3<f32>(-dx * 0.5, -dy * 0.5, 1.0));

    let light = normalize(vec3<f32>(cos(t * 1.7) * 0.7, sin(t * 1.3) * 0.7 - 0.2, 0.7));
    let half_way = normalize(light + vec3<f32>(0.0, 0.0, 1.0));
    let diffuse = dot(n, light) * 0.5 + 0.5;
    let shine = pow(max(dot(n, half_way), 0.0), 10.0);

    let bg = u.background.rgb;
    let lit = mix(u.foreground.rgb, u.accent.rgb, 0.35);
    var color = mix(bg, u.foreground.rgb, 0.01 + diffuse * 0.08);
    color = color + (lit - bg) * shine * 0.12;
    color = color + dither(frag) / 255.0;
    return vec4<f32>(color, 1.0);
}
