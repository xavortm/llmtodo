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
      F: "#4a4e66", // far legs: lighter, so they separate from the near legs
      P: "#d4869e", // inner ear
      B: "#2f3142", // blush: same as fur, it barely showed on black
      N: "#f08fab", // nose
      E: "#f5c542", // open eyes
      Q: "#9aa0b8", // closed eyes
      G: "#3b4a68", // screen glow on fur
      L: "#9aa0b5", // laptop lid
      D: "#6b7189", // laptop base
      C: "#ffffff", // laptop logo
      Z: "#7d84a0", // sleep z
      H: "#d3d7e2", // ground shadow under a jump
    },
    // White cat on a dark background. The outline is white too, so the cat
    // is a clean white shape. Grey inner lines keep the face and legs clear.
    dark: {
      bg: "#16171f",
      O: "#ffffff",
      I: "#8a8f9e",
      W: "#ffffff",
      S: "#d9dce4",
      F: "#c5c9d6",
      P: "#ff9bb8",
      B: "#ffd6e1",
      N: "#ff6f9a",
      E: "#000000",
      Q: "#3a3d4a",
      G: "#dff3ff",
      L: "#4b5068",
      D: "#343749",
      C: "#bfe8ff",
      Z: "#9aa6c7",
      H: "#2c2e3b",
    },
  };

  // Colour variants to compare for clarity. "soft" is the current palette.
  // The other three raise contrast in different ways. Each one only lists
  // the colours it changes.
  const VARIANT_CHANGES = {
    light: {
      soft: { name: "Soft (chosen)" },
      pure: {
        name: "Pure black",
        O: "#000000", I: "#000000", W: "#000000", B: "#000000", S: "#2a2a2a",
        F: "#2a2a2a", E: "#ffd400", Q: "#9a9a9a", P: "#ff8fb0", N: "#ff6f9a", G: "#1f2a44",
      },
      ink: {
        name: "Ink",
        O: "#000000", I: "#000000", W: "#1c1d26", B: "#1c1d26", S: "#0b0b10",
        E: "#ffd23f", Q: "#a3a8bf", P: "#e88aa8", N: "#ff7fa3",
      },
      detail: {
        name: "Ink, light inner lines",
        O: "#000000", I: "#6a6f8e", W: "#24252f", B: "#24252f", S: "#121219",
        E: "#ffcf33", Q: "#b8bdd1", P: "#ec8fac", N: "#ff7fa3",
      },
    },
    dark: {
      pure: { name: "Pure white (chosen)" },
      soft: {
        name: "Soft",
        O: "#8a93ad", I: "#8a93ad", S: "#c9cfe0", F: "#c9cfe0",
        E: "#1f2030", Q: "#4a5068", P: "#f2a7bd", N: "#ee7f9c",
      },
      bright: {
        name: "Bright outline",
        O: "#bcc3d8", I: "#bcc3d8", S: "#d6dbe8", F: "#d6dbe8", P: "#f2a7bd", N: "#ee7f9c",
      },
      crisp: {
        name: "Bright outline, dark inner lines",
        O: "#e6e9f2", I: "#5c6584", S: "#b4bcd2", F: "#b4bcd2", P: "#f2a7bd", N: "#ee7f9c",
      },
    },
  };
  const VARIANTS = {};
  for (const [theme, list] of Object.entries(VARIANT_CHANGES)) {
    VARIANTS[theme] = {};
    for (const [id, changes] of Object.entries(list)) VARIANTS[theme][id] = { ...PALETTES[theme], ...changes };
  }

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
    // Startled: the tail puffs up and stands straight.
    tailPuff: ["...KK...", "..KWWK..", ".KWWWWK.", "..KWWK..", ...BASE_SIT_TAIL],
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
    // Startled: tall, wide-open eyes.
    wide: ["E", [3, 5], [3, 6], [3, 7], [9, 5], [9, 6], [9, 7]],
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
    [[2, 2], [3, 2], [4, 2]], // 0 paw lands ahead
    [[2, 2], [2, 2], [3, 2]], // 1
    [[2, 2], [2, 2], [2, 2]], // 2 under the shoulder
    [[2, 2], [1, 2], [1, 2]], // 3 pushes off
    [[2, 2], [1, 2], null], //   4 paw lifts behind
    [[2, 2], [2, 2], null], //   5 swings under
    [[2, 2], [3, 2], null], //   6
    [[2, 3], [4, 2], null], //   7 reaches forward
  ];
  const HIND_LEG = [
    [[2, 3], [3, 2], [4, 2]], // 0 paw lands ahead
    [[2, 3], [2, 2], [3, 2]], // 1
    [[2, 3], [2, 2], [2, 2]], // 2
    [[2, 3], [1, 2], [1, 2]], // 3 pushes off
    [[2, 3], [0, 2], null], //   4 paw flicks back
    [[2, 3], [2, 2], null], //   5 tucked
    [[2, 3], [3, 2], null], //   6
    [[2, 3], [4, 2], null], //   7 reaches forward
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

  // A near leg is fur with an outline. A far leg is one solid "F" shape
  // of the same size, with no dark outline of its own. The near legs' lines
  // then separate the two sides instead of merging into a dark bundle.
  function leg(poses, phase, x, { far = false } = {}) {
    const cells = [];
    poses[((phase % 8) + 8) % 8].forEach((span, r) => {
      if (!span) return;
      for (let c = 0; c < span[1]; c++) cells.push([x + span[0] + c, 26 + r]);
    });
    let g = outline(cells, "W", { capTop: false });
    if (far) g = g.map((row) => row.map((c) => (c === "." ? c : "F")));
    return { g, x: 0, y: 0 };
  }

  // The tail rises from the back. A wave runs from the base to the tip,
  // so the tip swings most. The base stays thick, the tip curls forward.
  const TAIL_PATH = [[7, 22], [6, 21], [5, 20], [4, 19], [4, 18], [4, 17], [4, 16], [4, 15], [4, 14], [5, 13], [6, 13]];
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
      leg(HIND_LEG, f - 4, 8, { far: true }),
      leg(FRONT_LEG, f - 6, 16, { far: true }),
      { g: "bodySide", x: 6, y: 20 },
      leg(HIND_LEG, f, 5),
      leg(FRONT_LEG, f - 2, 13),
      { g: head({ eyes: "right" }), x: 15, y: 13 },
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

  // Startle ----------------------------------------------------------------
  // The sleeping cat wakes with a jolt, jumps straight up, and lands sitting.
  // It then hands over to the watch animation.

  // Short lines that flash around the head. "top" is the head's top row,
  // "cx" its centre column.
  function shock(top, cx) {
    const g = Array.from({ length: SIZE }, () => Array(SIZE).fill("."));
    const put = (x, y) => { if (x >= 0 && x < SIZE && y >= 0 && y < SIZE) g[y][x] = "Z"; };
    put(cx, top - 3); put(cx, top - 2);
    put(cx - 8, top); put(cx - 9, top - 1);
    put(cx + 8, top); put(cx + 9, top - 1);
    return { g, x: 0, y: 0 };
  }

  // A flat shadow on the ground, centred under the cat.
  function shadow(width) {
    const g = Array.from({ length: SIZE }, () => Array(SIZE).fill("."));
    const from = 15 - (width >> 1);
    for (let x = from; x < from + width; x++) g[30][x] = "H";
    return { g, x: 0, y: 0 };
  }

  // Still curled up, but the head snaps up with wide eyes.
  function jolt() {
    return compose([
      { g: "bodySleepA", x: 3, y: 22 },
      { g: "tailSleep", x: 1, y: 27 },
      { g: head({ eyes: "wide" }), x: 16, y: 16 },
      shock(16, 22),
    ]);
  }

  // The sitting cat, "dy" pixels above the ground and "dx" right of its
  // sitting spot. The sleeping head lies further right, so the jump starts
  // there and drifts back. In the air the paws hang below the body.
  // "squash" drops the head 1px as it lands.
  function hop({ dy = 0, dx = 0, eyes = "wide", puff = true, lines = false, squash = false, shade = 0 } = {}) {
    const layers = [];
    if (shade) layers.push({ ...shadow(shade), x: dx });
    layers.push({ g: puff ? "tailPuff" : WIGGLE[0], x: 21 + dx, y: 23 - dy });
    if (dy >= 3) {
      layers.push({ g: "paw", x: 10 + dx, y: 29 - dy });
      layers.push({ g: "paw", x: 17 + dx, y: 29 - dy });
    }
    layers.push({ g: "bodySit", x: 8 + dx, y: 18 - dy });
    layers.push({ g: head({ eyes }), x: 9 + dx, y: 9 - dy + (squash ? 1 : 0) });
    if (lines) layers.push({ ...shock(9 - dy, 15), x: dx });
    return compose(layers);
  }

  const STARTLE = [
    [jolt(), 260],
    [hop({ dy: 4, dx: 4, lines: true, shade: 9 }), 70],
    [hop({ dy: 7, dx: 2, lines: true, shade: 7 }), 150],
    [hop({ dy: 4, dx: 1, shade: 9 }), 70],
    [hop({ squash: true }), 90],
    [hop(), 240],
    [hop({ puff: false }), 220],
    [hop({ puff: false, eyes: "blink" }), 100],
    [hop({ puff: false, eyes: "open" }), 200],
  ];

  // Lie down ---------------------------------------------------------------
  // The sitting cat gets sleepy, sinks down, curls up, and lowers its head
  // into the sleep pose. It plays once, from watch into sleep.
  const LIE_DOWN = [
    // Sleepy eyes while still sitting.
    [compose([
      { g: WIGGLE[0], x: 21, y: 23 },
      { g: "bodySit", x: 8, y: 18 },
      { g: head({ eyes: "blink" }), x: 9, y: 9 },
    ]), 260],
    // Hunches down: the head sinks into the body, the tail lies flat.
    [compose([
      { g: "tailSleep", x: 13, y: 27 },
      { g: "bodySit", x: 8, y: 18 },
      { g: head({ eyes: "blink" }), x: 10, y: 12 },
    ]), 180],
    // Curls up, head still raised.
    [compose([
      { g: "bodySleepA", x: 3, y: 22 },
      { g: "tailSleep", x: 1, y: 27 },
      { g: head({ eyes: "blink" }), x: 14, y: 15 },
    ]), 200],
    // The head settles, eyes close.
    [compose([
      { g: "bodySleepA", x: 3, y: 22 },
      { g: "tailSleep", x: 1, y: 27 },
      { g: head({ eyes: "sleep" }), x: 16, y: 17 },
    ]), 240],
  ];

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
    // The startle plays once, from sleep into watch. It does not loop.
    // A first click on the sleeping cat shows "jolt" for 1 s. A second
    // click in that time plays "jump". A click within 3 s after the cat
    // dozed off again plays the whole "startle" at once.
    startle: anim(STARTLE),
    jolt: anim([[STARTLE[0][0], 1000]]),
    jump: anim(STARTLE.slice(1)),
    // Plays once, from watch into sleep.
    lieDown: anim(LIE_DOWN),
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

  // RGBA bytes for one frame in one palette, for canvas ImageData or PNG export.
  // "palette" is a theme name or a palette object, such as a variant.
  function rgba(frame, palette = "dark") {
    const pal = typeof palette === "string" ? PALETTES[palette] : palette;
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

  return { SIZE, PALETTES, VARIANTS, PARTS, ANIMS, rgba };
});
