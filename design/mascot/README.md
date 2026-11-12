# Mascot

The pixel cat in the app's sidebar. `sprites.js` holds all the art. Open `preview.html` to see the motion.

## The app's data file

The app reads `crates/ui/assets/mascot/mascot.json`. It holds the two chosen palettes and, for each animation, the frames and the timeline. Every "K" in it is already split into the outline "O" and the inner line "I".

Write it again after any change to `sprites.js`. Run this from the repo root, and commit the result:

```sh
node design/mascot/export.mjs --app
```

## PNG sheets

`node design/mascot/export.mjs <dir> [scale] [--bg]` writes one PNG sheet per animation and theme, plus `timeline.json`. The app does not use them.
