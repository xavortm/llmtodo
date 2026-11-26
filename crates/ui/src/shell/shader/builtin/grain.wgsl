// Grain: a fine, slowly changing film grain over the background color.
// A llmtodo background shader. Copy it out with `:shader copy grain`.
//
// Inputs, in `u`: time (s), resolution (px), origin (px in the window),
// pointer (px in this slot), background, foreground and accent (RGBA).

fn hash(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(123.34, 456.21));
    let r = q + dot(q, q + 45.32);
    return fract(r.x * r.y);
}

fn main_image(frag: vec2<f32>) -> vec4<f32> {
    let step = floor(u.time * 8.0);
    let n = hash(floor(frag + u.origin) + step * 17.0);
    let color = mix(u.background.rgb, u.foreground.rgb, n * 0.12);
    return vec4<f32>(color, 1.0);
}
