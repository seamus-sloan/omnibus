/**
 * Synthetic CBZ generator for Playwright fixtures.
 *
 * Produces a minimal comic archive — solid-color PNG pages in natural-sort
 * order plus a `ComicInfo.xml` — so the comic-pager spec can assert against
 * known values. Output is deterministic for a given Node/zlib version and is
 * committed under `test_data/epubs/generated/`, so CI does not run this tool.
 *
 * Usage (from the pnpm project dir):
 *   cd ui_tests/playwright && pnpm exec tsx tools/make_cbz.ts
 *
 * To add a fixture, edit FIXTURES below, re-run, and update the matching
 * entry in `tests/fixtures/epubs.ts`.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";
import JSZip from "jszip";

interface CbzInput {
  /** Output filename (no path). */
  filename: string;
  title: string;
  series: string;
  number: string;
  writer: string;
  /** One page per entry; each is a solid-color 600×800 PNG. */
  pageColors: readonly [number, number, number][];
}

const FIXTURES: CbzInput[] = [
  {
    filename: "aurora-station-01.cbz",
    title: "Aurora Station, Vol. 1",
    series: "Aurora Station",
    number: "1",
    writer: "Sora Inkwell",
    // Eight distinct hues so page turns are visible to a human eye; the
    // first page doubles as the extracted cover.
    pageColors: [
      [188, 71, 60],
      [214, 129, 54],
      [219, 179, 63],
      [104, 159, 84],
      [64, 130, 152],
      [83, 92, 168],
      [128, 77, 156],
      [173, 74, 122],
    ],
  },
  {
    filename: "aurora-station-02.cbz",
    title: "Aurora Station, Vol. 2",
    series: "Aurora Station",
    number: "2",
    writer: "Sora Inkwell",
    pageColors: [
      [70, 130, 110],
      [120, 110, 190],
      [190, 120, 80],
      [90, 150, 200],
      [160, 90, 90],
    ],
  },
];

/** Fixed timestamp so regenerated archives are byte-stable. */
const FIXED_DATE = new Date("2020-01-01T00:00:00Z");

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(buf: Buffer): number {
  let c = 0xffffffff;
  for (const byte of buf) {
    c = (CRC_TABLE[(c ^ byte) & 0xff] as number) ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

function pngChunk(type: string, data: Buffer): Buffer {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

/** A solid-color 8-bit RGB PNG — real, decodable image data so the CBZ
 *  indexer's cover + accent extraction has something to chew on. */
function solidPng(
  width: number,
  height: number,
  [r, g, b]: readonly [number, number, number],
): Buffer {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // color type: truecolor
  // compression / filter / interlace all 0

  const row = Buffer.alloc(1 + width * 3); // filter byte 0 + RGB pixels
  for (let x = 0; x < width; x++) {
    row[1 + x * 3] = r;
    row[2 + x * 3] = g;
    row[3 + x * 3] = b;
  }
  const raw = Buffer.concat(Array.from({ length: height }, () => row));
  const idat = deflateSync(raw, { level: 9 });

  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk("IHDR", ihdr),
    pngChunk("IDAT", idat),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

function escapeXml(s: string): string {
  return s
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function comicInfoXml(input: CbzInput): string {
  return `<?xml version="1.0" encoding="utf-8"?>
<ComicInfo xmlns:xsd="http://www.w3.org/2001/XMLSchema">
  <Title>${escapeXml(input.title)}</Title>
  <Series>${escapeXml(input.series)}</Series>
  <Number>${escapeXml(input.number)}</Number>
  <Writer>${escapeXml(input.writer)}</Writer>
  <PageCount>${input.pageColors.length}</PageCount>
</ComicInfo>
`;
}

async function buildCbz(input: CbzInput): Promise<Buffer> {
  const zip = new JSZip();
  zip.file("ComicInfo.xml", comicInfoXml(input), { date: FIXED_DATE });
  input.pageColors.forEach((color, i) => {
    const name = `p${String(i + 1).padStart(3, "0")}.png`;
    zip.file(name, solidPng(600, 800, color), { date: FIXED_DATE });
  });
  return zip.generateAsync({
    type: "nodebuffer",
    compression: "DEFLATE",
    compressionOptions: { level: 9 },
  });
}

async function main() {
  const here = dirname(fileURLToPath(import.meta.url));
  const repoRoot = resolve(here, "..", "..", "..");
  const outDir = resolve(repoRoot, "test_data", "epubs", "generated");
  mkdirSync(outDir, { recursive: true });

  for (const fx of FIXTURES) {
    const buf = await buildCbz(fx);
    const path = resolve(outDir, fx.filename);
    writeFileSync(path, buf);
    console.log(`wrote ${path} (${buf.length} bytes)`);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
