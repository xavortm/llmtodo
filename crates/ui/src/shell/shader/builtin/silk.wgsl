// Silk: soft satin folds, lit by a light that drifts slowly across them,
// and by a faint second light that follows the pointer. Made for the
// sidebar. A llmtodo background shader. Copy it out with
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
    let fg = u.foreground.rgb;
    let lit = mix(fg, u.accent.rgb, 0.35);
    // On a dark theme the folds' shadows sink below the background,
    // toward black; a light theme keeps its paper.
    let dark = 1.0 - dot(bg, vec3<f32>(0.299, 0.587, 0.114));
    var color = bg * (1.0 - 0.5 * dark * (1.0 - diffuse));
    color = mix(color, fg, diffuse * diffuse * 0.035);
    color = color + (lit - bg) * shine * 0.05;

    // The pointer: a small light just above the cloth. It fades with
    // distance, so it is gone once the pointer leaves the slot.
    let m = u.pointer / max(u.resolution.y, 1.0) * 5.0;
    let to_m = vec3<f32>(m - p, 0.35);
    let near = exp(-dot(m - p, m - p) / 1.2);
    let m_half = normalize(normalize(to_m) + vec3<f32>(0.0, 0.0, 1.0));
    let m_shine = pow(max(dot(n, m_half), 0.0), 8.0);
    color = color + (lit - bg) * (m_shine * 0.16 + 0.03) * near;
    color = color + dither(frag) / 255.0;
    return vec4<f32>(color, 1.0);
}
