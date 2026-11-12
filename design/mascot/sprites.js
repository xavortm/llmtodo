// llmtodo mascot: a white pixel cat.
// Every frame is a 32x32 grid of palette keys. "." is transparent.
// The frames are built from parts (head, body, tail, legs, laptop), so all
// poses share one head and stay consistent.
(function (root, factory) {
  if (typeof module === "object" && module.exports) module.exports = factory();
  else root.Mascot = factory();
})(typeof self !== "undefined" ? self : this, function () {
  const SIZE = 32;

  const PALETTE = {
    K: "#1f2030", // outline
    E: "#1f2030", // eyes
    W: "#ffffff", // fur
    S: "#c9cfe0", // fur shade
    P: "#f2a7bd", // inner ear
    B: "#ffd6e1", // blush
    N: "#ee7f9c", // nose
    G: "#dff3ff", // screen glow on fur
    L: "#6b7189", // laptop lid
    D: "#3b3f52", // laptop base
    C: "#bfe8ff", // laptop logo
    Z: "#9aa6c7", // sleep z
  };

  const PARTS = {
    head: [
      ".K.........K.",
      ".KK.......KK.",
      ".KPK.....KPK.",
      ".KPWKKKKKWPK.",
      "KWWWWWWWWWWWK",
      "KWWWWWWWWWWWK",
      "KWWWWWWWWWWWK",
      "KWWWWWWWWWWWK",
      "KWBWWWNWWWBWK",
      "KSWWWKWKWWWSK",
      ".KSSWWWWWSSK.",
      "..KKKKKKKKK..",
    ],
    // Front view, sitting.
    bodySit: [
      "...KWWWWWWWK...",
      "..KWWWWWWWWWK..",
      "..KWWWWWWWWWK..",
      ".KWWWWWWWWWWWK.",
      ".KWWWWWWWWWWWK.",
      "KSWWWWWWWWWWWSK",
      "KSWWWWWWWWWWWSK",
      "KSWWWWWWWWWWWSK",
      "KSSWWWWKWWWWSSK",
      "KSSWKWWKWWKWSSK",
      "KSSKWWWKWWWKSSK",
      ".KKKKKKKKKKKKK.",
    ],
    tailSitA: [
      "....K..",
      "...KWK.",
      "...KWK.",
      "...KWK.",
      "KKKKWK.",
      "WWWWSK.",
      "KKKKK..",
    ],
    tailSitB: [
      ".......",
      ".....K.",
      "...KKWK",
      "...KWK.",
      "KKKKWK.",
      "WWWWSK.",
      "KKKKK..",
    ],
    // Side view, facing right.
    bodySide: [
      "..KKKKKKKKKKKK..",
      ".KWWWWWWWWWWWWK.",
      "KWWWWWWWWWWWWWWK",
      "KWWWWWWWWWWWWWWK",
      "KWWWWWWWWWWWWWWK",
      ".KSSSSSSSSSSSSK.",
      "..KKKKKKKKKKKK..",
    ],
    tailSideA: [
      ".KK...",
      "KWWK..",
      "KWK...",
      "KWK...",
      "KWK...",
      "KWK...",
      "KWWK..",
      ".KWWK.",
      "..KWWK",
      "...KWW",
    ],
    tailSideB: [
      "......",
      ".KK...",
      "KWWK..",
      "KWK...",
      "KWK...",
      "KWK...",
      "KWWK..",
      ".KWWK.",
      "..KWWK",
      "...KWW",
    ],
    legNear: ["KWK", "KWK", "KWK", "KWK", "KKK"],
    legFar: ["KSK", "KSK", "KSK", "KSK", "KKK"],
    // Curled up, side view.
    bodySleepA: [
      ".....KKKKKKKK.....",
      "...KKWWWWWWWWKK...",
      "..KWWWWWWWWWWWWK..",
      ".KWWWWWWWWWWWWWWK.",
      "KWWWWWWWWWWWWWWWWK",
      "KSWWWWWWWWWWWWWWSK",
      "KSSWWWWWWWWWWWWSSK",
      ".KKKKKKKKKKKKKKKK.",
    ],
    bodySleepB: [
      ".....KKKKKKKK.....",
      "...KKWWWWWWWWKK...",
      "..KWWWWWWWWWWWWK..",
      ".KWWWWWWWWWWWWWWK.",
      ".KWWWWWWWWWWWWWWK.",
      "KWWWWWWWWWWWWWWWWK",
      "KSWWWWWWWWWWWWWWSK",
      "KSSWWWWWWWWWWWWSSK",
      ".KKKKKKKKKKKKKKKK.",
    ],
    tailSleep: [
      ".KKKKKKKKKKKKKK",
      "KWWWWWWWWWWWWWW",
      ".KKKKKKKKKKKKKK",
    ],
    zBig: ["ZZZZ", "..Z.", ".Z..", "ZZZZ"],
    zSmall: ["ZZZ", "..Z", ".Z.", "ZZZ"],
    paw: [".KK.", "KWWK", "KWWK"],
    // Laptop seen from behind the lid.
    laptop: [
      "KKKKKKKKKKKKKKK",
      "KLLLLLLLLLLLLLK",
      "KLLLLLLLLLLLLLK",
      "KLLLLLCCCLLLLLK",
      "KLLLLLLLLLLLLLK",
      "KKKKKKKKKKKKKKK",
      "KDDDDDDDDDDDDDK",
      ".KKKKKKKKKKKKK.",
    ],
  };

  const EYES = {
    open: [[3, 6], [3, 7], [9, 6], [9, 7]],
    left: [[2, 6], [2, 7], [8, 6], [8, 7]],
    right: [[4, 6], [4, 7], [10, 6], [10, 7]],
    down: [[3, 7], [3, 8], [9, 7], [9, 8]],
    blink: [[2, 7], [3, 7], [4, 7], [8, 7], [9, 7], [10, 7]],
    sleep: [[2, 6], [3, 7], [4, 6], [8, 6], [9, 7], [10, 6]],
    happy: [[3, 6], [2, 7], [4, 7], [9, 6], [8, 7], [10, 7]],
  };

  const toGrid = (rows) => rows.map((r) => r.split(""));

  function head({ eyes = "open", glow = false } = {}) {
    const g = toGrid(PARTS.head);
    for (const [x, y] of EYES[eyes]) g[y][x] = "E";
    if (glow) {
      for (let y = 10; y <= 10; y++)
        for (let x = 0; x < g[y].length; x++) if (g[y][x] === "W") g[y][x] = "G";
    }
    return g;
  }

  function compose(layers) {
    const out = Array.from({ length: SIZE }, () => Array(SIZE).fill("."));
    for (const { g, x, y } of layers) {
      const grid = typeof g === "string" ? toGrid(PARTS[g]) : g;
      grid.forEach((row, dy) =>
        row.forEach((c, dx) => {
          const px = x + dx;
          const py = y + dy;
          if (c !== "." && px >= 0 && px < SIZE && py >= 0 && py < SIZE) out[py][px] = c;
        })
      );
    }
    return out;
  }

  const flip = (grid) => grid.map((row) => [...row].reverse());

  // Poses -----------------------------------------------------------------

  function sit({ eyes = "open", tail = "A" } = {}) {
    return compose([
      { g: "tailSit" + tail, x: 21, y: 23 },
      { g: "bodySit", x: 8, y: 18 },
      { g: head({ eyes }), x: 9, y: 9 },
    ]);
  }

  function laptop({ left = 0, right = 0, eyes = "down", glow = true } = {}) {
    return compose([
      { g: "bodySit", x: 8, y: 18 },
      { g: head({ eyes, glow }), x: 9, y: 9 },
      { g: "paw", x: 10, y: 20 - left },
      { g: "paw", x: 17, y: 20 - right },
      { g: "laptop", x: 8, y: 22 },
    ]);
  }

  // Leg steps per walk frame: [dx, lift] for near-back, near-front, far-back, far-front.
  const STEPS = [
    [[-1, 0], [1, 0], [1, 0], [-1, 0]],
    [[0, 1], [0, 0], [0, 0], [0, 1]],
    [[1, 0], [-1, 0], [-1, 0], [1, 0]],
    [[0, 0], [0, 1], [0, 1], [0, 0]],
  ];

  function walk(f) {
    const [nb, nf, fb, ff] = STEPS[f];
    const bob = f % 2 === 1 ? -1 : 0;
    return compose([
      { g: f % 2 ? "tailSideB" : "tailSideA", x: 2, y: 11 + bob },
      { g: "legFar", x: 9 + fb[0], y: 25 - fb[1] },
      { g: "legFar", x: 19 + ff[0], y: 25 - ff[1] },
      { g: "legNear", x: 7 + nb[0], y: 25 - nb[1] },
      { g: "legNear", x: 17 + nf[0], y: 25 - nf[1] },
      { g: "bodySide", x: 6, y: 19 + bob },
      { g: head({ eyes: "right" }), x: 15, y: 12 + bob },
    ]);
  }

  function sleep(f) {
    const breathIn = f >= 2 && f < 6;
    const z2 = (f + 4) % 8;
    return compose([
      breathIn ? { g: "bodySleepB", x: 3, y: 21 } : { g: "bodySleepA", x: 3, y: 22 },
      { g: "tailSleep", x: 1, y: 27 },
      { g: head({ eyes: "sleep" }), x: 16, y: 18 },
      { g: "zBig", x: 23 + (f % 2), y: 13 - f },
      { g: "zSmall", x: 27 - (z2 % 2), y: 15 - z2 },
    ]);
  }

  // Animations: lists of { frame, ms } ------------------------------------

  const ANIMS = {
    watch: [
      [sit(), 1400],
      [sit({ eyes: "blink" }), 120],
      [sit(), 700],
      [sit({ tail: "B" }), 260],
      [sit(), 260],
      [sit({ tail: "B" }), 260],
      [sit(), 700],
      [sit({ eyes: "left" }), 1100],
      [sit(), 300],
      [sit({ eyes: "right" }), 1100],
      [sit(), 900],
      [sit({ eyes: "blink" }), 120],
      [sit(), 600],
      [sit({ eyes: "happy" }), 900],
    ],
    laptop: [
      [laptop({ left: 1 }), 140],
      [laptop({ glow: false }), 140],
      [laptop({ right: 1 }), 140],
      [laptop(), 140],
      [laptop({ left: 1, glow: false }), 140],
      [laptop(), 140],
      [laptop({ right: 1 }), 140],
      [laptop({ eyes: "blink" }), 120],
    ],
    sleep: Array.from({ length: 8 }, (_, f) => [sleep(f), 320]),
    walkRight: [0, 1, 2, 3].map((f) => [walk(f), 150]),
    walkLeft: [0, 1, 2, 3].map((f) => [flip(walk(f)), 150]),
  };

  for (const k of Object.keys(ANIMS)) ANIMS[k] = ANIMS[k].map(([frame, ms]) => ({ frame, ms }));

  function hexToRgba(hex) {
    const n = parseInt(hex.slice(1), 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255, 255];
  }

  // RGBA bytes for one frame, for canvas ImageData or PNG export.
  function rgba(frame) {
    const out = new Uint8ClampedArray(SIZE * SIZE * 4);
    frame.forEach((row, y) =>
      row.forEach((c, x) => {
        if (c === ".") return;
        out.set(hexToRgba(PALETTE[c]), (y * SIZE + x) * 4);
      })
    );
    return out;
  }

  return { SIZE, PALETTE, PARTS, ANIMS, rgba };
});
