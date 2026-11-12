// Writes one PNG sprite sheet per animation: frames side by side, 32x32 each.
// Usage: node design/mascot/export.mjs <out-dir> [scale]
import { createRequire } from "node:module";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { deflateSync } from "node:zlib";

const Mascot = createRequire(import.meta.url)("./sprites.js");
const outDir = process.argv[2] ?? "design/mascot/out";
const scale = Number(process.argv[3] ?? 1);

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 255] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}
function png(width, height, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  const raw = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (width * 4 + 1)] = 0;
    rgba.copy(raw, y * (width * 4 + 1) + 1, y * width * 4, (y + 1) * width * 4);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

mkdirSync(outDir, { recursive: true });
const S = Mascot.SIZE;
for (const [name, frames] of Object.entries(Mascot.ANIMS)) {
  const w = S * frames.length * scale;
  const h = S * scale;
  const sheet = Buffer.alloc(w * h * 4);
  frames.forEach(({ frame }, i) => {
    const px = Mascot.rgba(frame);
    for (let y = 0; y < h; y++)
      for (let x = 0; x < S * scale; x++) {
        const src = (Math.floor(y / scale) * S + Math.floor(x / scale)) * 4;
        const dst = (y * w + i * S * scale + x) * 4;
        for (let c = 0; c < 4; c++) sheet[dst + c] = px[src + c];
      }
  });
  writeFileSync(join(outDir, `${name}.png`), png(w, h, sheet));
  console.log(`${name}.png  ${frames.length} frames`);
}
