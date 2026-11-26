// Lights: soft orbs of light in the theme's colors, drifting slowly.
// Made for the sidebar. A llmtodo background shader. Copy it out with
// `:shader copy lights`.
//
// Inputs, in `u`: time (s), resolution (px), origin (px in the window),
// pointer (px in this slot), background, foreground and accent (RGBA).

fn dither(frag: vec2<f32>) -> f32 {
    return fract(sin(dot(frag, vec2<f32>(12.9898, 78.233))) * 43758.547) - 0.5;
}

fn main_image(frag: vec2<f32>) -> vec4<f32> {
    let size = max(u.resolution, vec2<f32>(1.0, 1.0));
    let aspect = size.x / size.y;
    // y from 0 to 1 down the slot; x keeps the same scale.
    let p = frag / size.y;
    let t = u.time * 0.05;

    let bg = u.background.rgb;
    let fg = u.foreground.rgb;
    let accent = u.accent.rgb;
    // A faint wash from the top, so the slot never sits flat.
    var color = mix(bg, fg, (1.0 - p.y) * 0.025);
    for (var i = 0; i < 5; i = i + 1) {
        let f = f32(i);
        let center = vec2<f32>(
            aspect * (0.5 + 0.5 * sin(t * (0.9 + f * 0.23) + f * 1.9)),
            0.5 + 0.5 * sin(t * (0.6 + f * 0.17) + f * 2.7 + 0.8),
        );
        // Round in px whatever the slot's shape: sized by the width.
        let radius = max(aspect, 0.25) * (0.35 + 0.12 * fract(f * 0.61));
        let d = length(p - center) / radius;
        let glow = exp(-d * d * 1.6);
        let tint = mix(accent, fg, fract(f * 0.37) * 0.8);
        color = color + (tint - bg) * glow * 0.12;
    }
    color = color + dither(frag) / 255.0;
    return vec4<f32>(color, 1.0);
}
