// llmtodo mascot: a pixel cat. It is black on light themes and white on dark.
// Every frame is a 32x32 grid of palette keys. "." is transparent.
// The frames are built from parts (head, body, tail, legs, laptop), so all
// poses share one head and stay consistent.
//
// "K" marks any line. When a frame is coloured, a K next to transparency
// becomes the outer outline "O", and every other K becomes an inner line "I".
// Each theme colours the two kinds on their own.
(function (root, factory) {
  if (typeof module === "object" && module.exports) module.exports = factory();
  else root.Mascot = factory();
})(typeof self !== "undefined" ? self : this, function () {
  const SIZE = 32;

  const PALETTES = {
    // Black cat on a light background.
    light: {
      bg: "#f4f5f8",
      O: "#15161d", // outer outline
      I: "#15161d", // inner lines
      W: "#2f3142", // fur
      S: "#222330", // fur shade
      P: "#d4869e", // inner ear
      B: "#46364b", // blush
      N: "#f08fab", // nose
      E: "#f5c542", // open eyes
      Q: "#9aa0b8", // closed eyes
      G: "#3b4a68", // screen glow on fur
      L: "#9aa0b5", // laptop lid
      D: "#6b7189", // laptop base
      C: "#ffffff", // laptop logo
      Z: "#7d84a0", // sleep z
    },
    // White cat on a dark background. A soft outline keeps the edge visible
    // without a black ring that melts into the background.
    dark: {
      bg: "#16171f",
      O: "#8a93ad",
      I: "#5f6682",
      W: "#ffffff",
      S: "#c9cfe0",
      P: "#f2a7bd",
      B: "#ffd6e1",
      N: "#ee7f9c",
      E: "#1f2030",
      Q: "#4a5068",
      G: "#dff3ff",
      L: "#4b5068",
      D: "#343749",
      C: "#bfe8ff",
      Z: "#9aa6c7",
    },
  };

  const BASE_SIT_TAIL = ["KKKKWK..", "WWWWSK..", "KKKKK..."];

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
    // Sitting tail, one grid per wiggle position. They share the base rows.
    tailUp: ["....K...", "...KWK..", "...KWK..", "...KWK..", ...BASE_SIT_TAIL],
    tailHalfR: [".....K..", "....KWK.", "...KWK..", "...KWK..", ...BASE_SIT_TAIL],
    tailRight: ["......K.", ".....KWK", "....KWK.", "...KWK..", ...BASE_SIT_TAIL],
    tailHalfL: ["...K....", "..KWK...", "...KWK..", "...KWK..", ...BASE_SIT_TAIL],
    tailLeft: ["..K.....", ".KWK....", "..KWK...", "...KWK..", ...BASE_SIT_TAIL],
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
    // Curled up, side view. B is the breath in.
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
    tailSleep: [".KKKKKKKKKKKKKK", "KWWWWWWWWWWWWWW", ".KKKKKKKKKKKKKK"],
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

  // Open eyes use "E". Closed eyes use "Q", so a black cat's closed eyes do
  // not glow like open ones.
  const EYES = {
    open: ["E", [3, 6], [3, 7], [9, 6], [9, 7]],
    left: ["E", [2, 6], [2, 7], [8, 6], [8, 7]],
    right: ["E", [4, 6], [4, 7], [10, 6], [10, 7]],
    down: ["E", [3, 7], [3, 8], [9, 7], [9, 8]],
    blink: ["Q", [2, 7], [3, 7], [4, 7], [8, 7], [9, 7], [10, 7]],
    sleep: ["Q", [2, 6], [3, 7], [4, 6], [8, 6], [9, 7], [10, 6]],
    happy: ["Q", [3, 6], [2, 7], [4, 7], [9, 6], [8, 7], [10, 7]],
  };

  const toGrid = (rows) => rows.map((r) => r.split(""));

  function head({ eyes = "open", glow = false, twitch = false } = {}) {
    const g = toGrid(PARTS.head);
    const [key, ...cells] = EYES[eyes];
    for (const [x, y] of cells) g[y][x] = key;
    if (glow) for (let x = 0; x < g[10].length; x++) if (g[10][x] === "W") g[10][x] = "G";
    if (twitch) g[0][11] = "."; // right ear tip folds down for a moment
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

  const WIGGLE = ["tailUp", "tailHalfR", "tailRight", "tailHalfR", "tailUp", "tailHalfL", "tailLeft", "tailHalfL"];

  function sit({ eyes = "open", tail = 0, twitch = false } = {}) {
    return compose([
      { g: WIGGLE[tail % 8], x: 21, y: 23 },
      { g: "bodySit", x: 8, y: 18 },
      { g: head({ eyes, twitch }), x: 9, y: 9 },
    ]);
  }

  function laptop({ left = 0, right = 0, eyes = "down", glow = true, tail = 0 } = {}) {
    return compose([
      { g: WIGGLE[tail % 8], x: 21, y: 23 },
      { g: "bodySit", x: 8, y: 18 },
      { g: head({ eyes, glow }), x: 9, y: 9 },
      { g: "paw", x: 10, y: 20 - left },
      { g: "paw", x: 17, y: 20 - right },
      { g: "laptop", x: 8, y: 22 },
    ]);
  }

  // Walk ------------------------------------------------------------------
  // Legs and the tail are drawn as fill shapes; outline() then wraps them
  // in a 1px line. Each leg pose lists one [start column, width] fill span
  // per row, top to bottom. Row 0 joins the body. A null row is empty, so
  // the paw is lifted.
  //
  // Each leg runs an 8-frame cycle: 4 frames on the ground, then 4 in the
  // air. On the ground the paw moves back 1px per frame. The cat moves
  // forward 1px per frame, so a planted paw stays still on the floor.
  const FRONT_LEG = [
    [[2, 2], [2, 2], [2, 2], [3, 2], [3, 2], [4, 1], [4, 2]], // 0 paw lands ahead
    [[2, 2], [2, 2], [2, 2], [3, 2], [3, 1], [3, 1], [3, 2]], // 1
    [[2, 2], [2, 2], [2, 2], [2, 2], [2, 1], [2, 1], [2, 2]], // 2 under the shoulder
    [[2, 2], [2, 2], [2, 2], [1, 2], [1, 1], [1, 1], [1, 2]], // 3 pushes off
    [[2, 2], [2, 2], [2, 2], [3, 1], [2, 1], [1, 2], null], //   4 wrist curls, paw lifts
    [[2, 2], [2, 2], [2, 2], [3, 1], [3, 1], [2, 2], null], //   5 swings under
    [[2, 2], [2, 2], [2, 2], [3, 2], [4, 1], [3, 2], null], //   6
    [[2, 2], [3, 2], [3, 2], [4, 1], [4, 2], [5, 2], null], //   7 reaches forward
  ];
  const HIND_LEG = [
    [[2, 3], [2, 3], [2, 3], [3, 2], [3, 1], [4, 1], [4, 2]], // 0 paw lands ahead
    [[2, 3], [2, 3], [2, 3], [2, 2], [2, 1], [3, 1], [3, 2]], // 1
    [[2, 3], [2, 3], [2, 3], [2, 2], [1, 1], [2, 1], [2, 2]], // 2 hock bends back
    [[2, 3], [2, 3], [2, 3], [1, 2], [0, 1], [0, 1], [0, 2]], // 3 pushes off
    [[2, 3], [2, 3], [2, 3], [2, 2], [1, 1], [0, 2], null], //   4 paw flicks back
    [[2, 3], [2, 3], [2, 3], [2, 2], [2, 1], [2, 2], null], //   5 tucked
    [[2, 3], [2, 3], [2, 3], [3, 2], [3, 1], [3, 2], null], //   6
    [[2, 3], [2, 3], [2, 3], [3, 2], [4, 1], [4, 2], null], //   7 reaches forward
  ];

  // Wraps fill cells in a 1px outline and returns a full-frame layer.
  // With capTop false, the top row gets no line above it, so a leg can
  // join the body without a seam.
  function outline(cells, fill, { capTop = true } = {}) {
    const g = Array.from({ length: SIZE }, () => Array(SIZE).fill("."));
    let top = SIZE;
    for (const [x, y] of cells) {
      if (x >= 0 && x < SIZE && y >= 0 && y < SIZE) g[y][x] = fill;
      top = Math.min(top, y);
    }
    const out = g.map((r) => [...r]);
    for (let y = 0; y < SIZE; y++)
      for (let x = 0; x < SIZE; x++) {
        if (g[y][x] !== "." || (!capTop && y < top)) continue;
        const near = [[1, 0], [-1, 0], [0, 1], [0, -1]].some(([dx, dy]) => {
          const c = g[y + dy]?.[x + dx];
          return c !== undefined && c !== ".";
        });
        if (near) out[y][x] = "K";
      }
    return out;
  }

  function leg(poses, phase, x, fill) {
    const cells = [];
    poses[((phase % 8) + 8) % 8].forEach((span, r) => {
      if (!span) return;
      for (let c = 0; c < span[1]; c++) cells.push([x + span[0] + c, 22 + r]);
    });
    return { g: outline(cells, fill, { capTop: false }), x: 0, y: 0 };
  }

  // The tail rises from the back. A wave runs from the base to the tip,
  // so the tip swings most. The base stays thick, the tip curls forward.
  const TAIL_PATH = [[7, 18], [6, 17], [5, 16], [4, 15], [4, 14], [4, 13], [4, 12], [4, 11], [4, 10], [5, 9], [6, 9]];
  function walkTail(f) {
    const cells = [];
    let prevX = null;
    TAIL_PATH.forEach(([x, y], i) => {
      const amp = Math.max(0, i - 3) * 0.3;
      let nx = x + Math.round(amp * Math.sin((2 * Math.PI * f) / 8 - i * 0.35));
      if (prevX !== null) nx = Math.max(prevX - 1, Math.min(prevX + 1, nx));
      if (i === TAIL_PATH.length - 1) nx = prevX + 1; // the tip always hooks forward
      prevX = nx;
      cells.push([nx, y]);
      if (i >= 1 && i <= 3) cells.push([nx + 1, y]);
    });
    return { g: outline(cells, "W"), x: 0, y: 0 };
  }

  // Lateral gait: near hind, near front, far hind, far front, 2 frames apart.
  function walk(f) {
    return compose([
      walkTail(f),
      leg(HIND_LEG, f - 4, 8, "S"),
      leg(FRONT_LEG, f - 6, 16, "S"),
      { g: "bodySide", x: 6, y: 16 },
      leg(HIND_LEG, f, 5, "W"),
      leg(FRONT_LEG, f - 2, 13, "W"),
      { g: head({ eyes: "right" }), x: 15, y: 9 },
    ]);
  }

  // A z starts small above the head, then grows and drifts up and right.
  function z(phase) {
    return { g: phase < 6 ? "zSmall" : "zBig", x: 22 + (phase >> 2), y: 15 - phase };
  }

  function sleep(f) {
    const breathIn = f >= 4 && f < 12;
    return compose([
      breathIn ? { g: "bodySleepB", x: 3, y: 21 } : { g: "bodySleepA", x: 3, y: 22 },
      { g: "tailSleep", x: 1, y: 27 },
      { g: head({ eyes: "sleep", twitch: f === 13 }), x: 16, y: 18 },
      z(f),
      z((f + 8) % 16),
    ]);
  }

  // Animations ------------------------------------------------------------
  // Each animation keeps its unique frames once, plus a timeline of
  // { i: frame index, ms: duration }. The timeline is what a player loops.

  function anim(seq) {
    const frames = [];
    const index = new Map();
    const timeline = [];
    for (const [grid, ms] of seq) {
      const key = grid.map((r) => r.join("")).join("\n");
      if (!index.has(key)) {
        index.set(key, frames.length);
        frames.push(grid);
      }
      const i = index.get(key);
      const prev = timeline[timeline.length - 1];
      if (prev && prev.i === i) prev.ms += ms;
      else timeline.push({ i, ms });
    }
    return { frames, timeline };
  }

  // Watch: 80 ticks of 120ms. The tail wiggles all the time; the eyes follow
  // this schedule of [first tick, eyes].
  const WATCH_EYES = [
    [0, "open"], [12, "blink"], [13, "open"], [24, "left"], [34, "open"],
    [37, "right"], [47, "open"], [56, "blink"], [57, "open"], [58, "blink"],
    [59, "open"], [68, "happy"], [76, "open"],
  ];
  const eyesAt = (t) => WATCH_EYES.filter(([start]) => start <= t).pop()[1];

  // Laptop: 16 ticks of 90ms. "L" and "R" lift a paw, "." rests.
  const TYPING = "L.R.LRL..R.LR...";
  const GLOW = "1101110110111101";

  const walkFrames = Array.from({ length: 8 }, (_, f) => walk(f));

  const ANIMS = {
    watch: anim(
      Array.from({ length: 80 }, (_, t) => [sit({ eyes: eyesAt(t), tail: t, twitch: t === 20 || t === 21 }), 120])
    ),
    laptop: anim(
      Array.from({ length: 16 }, (_, t) => [
        laptop({
          left: TYPING[t] === "L" ? 1 : 0,
          right: TYPING[t] === "R" ? 1 : 0,
          glow: GLOW[t] === "1",
          eyes: t === 15 ? "blink" : t === 8 ? "open" : "down",
          tail: t >> 1,
        }),
        90,
      ])
    ),
    sleep: anim(Array.from({ length: 16 }, (_, f) => [sleep(f), 160])),
    walkRight: anim(walkFrames.map((g) => [g, 90])),
    walkLeft: anim(walkFrames.map((g) => [flip(g), 90])),
  };

  function hexToRgba(hex) {
    const n = parseInt(hex.slice(1), 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255, 255];
  }

  // Outer outline if a line pixel touches transparency or the frame edge.
  function lineKind(frame, x, y) {
    for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) {
      const c = frame[y + dy]?.[x + dx];
      if (c === undefined || c === ".") return "O";
    }
    return "I";
  }

  // RGBA bytes for one frame in one theme, for canvas ImageData or PNG export.
  function rgba(frame, theme = "dark") {
    const pal = PALETTES[theme];
    const out = new Uint8ClampedArray(SIZE * SIZE * 4);
    frame.forEach((row, y) =>
      row.forEach((c, x) => {
        if (c === ".") return;
        const key = c === "K" ? lineKind(frame, x, y) : c;
        out.set(hexToRgba(pal[key]), (y * SIZE + x) * 4);
      })
    );
    return out;
  }

  return { SIZE, PALETTES, PARTS, ANIMS, rgba };
});
