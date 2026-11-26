// Aurora: slow bands of the theme's accent over its background.
// A llmtodo background shader. Copy it out with `:shader copy aurora`.
//
// Inputs, in `u`: time (s), resolution (px), origin (px in the window),
// pointer (px in this slot), background, foreground and accent (RGBA).
// `frag` is in px from the top left of the slot.

fn band(y: f32, center: f32, width: f32) -> f32 {
    let d = (y - center) / width;
    return exp(-d * d);
}

fn main_image(frag: vec2<f32>) -> vec4<f32> {
    // Window coordinates, so the window slot lines up across panels.
    let p = (frag + u.origin) / max(u.resolution.y, 1.0);
    let t = u.time * 0.05;
    var glow = 0.0;
    for (var i = 0; i < 3; i = i + 1) {
        let f = f32(i);
        let wave = sin(p.x * (1.6 + f * 0.7) + t * (1.0 + f * 0.4) + f * 2.1) * 0.12;
        glow = glow + band(p.y, 0.3 + f * 0.18 + wave, 0.06 + f * 0.02) * (0.7 - f * 0.15);
    }
    let tint = mix(u.accent.rgb, u.foreground.rgb, 0.25);
    let color = mix(u.background.rgb, tint, clamp(glow, 0.0, 1.0));
    return vec4<f32>(color, 1.0);
}
