// Waves: soft rolling lines in the accent color, which lean toward the
// pointer. A llmtodo background shader. Copy it out with
// `:shader copy waves`.
//
// Inputs, in `u`: time (s), resolution (px), origin (px in the window),
// pointer (px in this slot), background, foreground and accent (RGBA).

fn main_image(frag: vec2<f32>) -> vec4<f32> {
    let size = max(u.resolution, vec2<f32>(1.0, 1.0));
    let p = frag / size.y;
    let m = u.pointer / size.y;
    let lean = exp(-length(p - m) * 3.0) * 0.08;
    let y = p.y * 14.0 + sin(p.x * 4.0 + u.time * 0.6) * 1.2 + lean * 20.0;
    let line = smoothstep(0.92, 1.0, abs(sin(y)));
    let color = mix(u.background.rgb, u.accent.rgb, line * 0.5);
    return vec4<f32>(color, 1.0);
}
