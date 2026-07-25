/**
 * Synthetic MP3 generator for Playwright audiobook fixtures.
 *
 * Produces minimal valid MPEG-1 Layer III silent MP3 files with an ID3v2.4
 * tag block carrying TIT2 / TPE1 / TALB / TRCK plus a 1×1 transparent PNG
 * cover image (APIC frame) so the omnibus indexer's cover extraction +
 * thumbnail pipeline is exercised end-to-end.
 *
 * Output is deterministic — given the same inputs, the resulting bytes are
 * identical run-to-run. Generated files are committed under
 * `test_data/audiobooks/generated/` so CI does not need to run this tool.
 *
 * Usage (from the pnpm project dir):
 *   cd ui_tests/playwright && pnpm exec tsx tools/make_audiobook.ts
 *
 * To add a new fixture, edit FIXTURES below and re-run.
 *
 * Why a hand-rolled generator rather than ffmpeg: ffmpeg is only present
 * in the `e2e` nix shell, so a script that shells out would gate fixture
 * regen on a heavy dev shell. A pure-Node generator works from any shell
 * (and from CI without extra setup). The MP3 produced is genuine silent
 * audio — `lofty` reads it the same way it would read a real audiobook,
 * so the indexer code path is exercised faithfully.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

interface AudiobookInput {
  /** Relative path under the output directory, including subdirectories. */
  filename: string;
  /** ID3 TIT2 (track / chapter title). */
  title: string;
  /** ID3 TPE1 (primary artist). */
  artist: string;
  /** ID3 TALB (album / book title). */
  album: string;
  /** ID3 TRCK (track number). */
  track: number;
  /** Number of silent MP3 frames to emit (~38 frames ≈ 1 second). */
  frames: number;
}

const FIXTURES: AudiobookInput[] = [
  // Single-file audiobook in a unique subdirectory — the indexer treats
  // it as a one-part AudiobookGroup with parts.len() == 1.
  {
    filename: "ada_lovelace_solo/the_analytical_audiobook.mp3",
    title: "The Analytical Audiobook",
    artist: "Ada Lovelace",
    album: "The Analytical Audiobook",
    track: 1,
    frames: 80,
  },

  // Multi-file audiobook — two MP3s in the same directory form one
  // AudiobookGroup with two parts ordered by TRCK then filename.
  {
    filename: "grace_hopper_series/the_compiled_tales/chapter01.mp3",
    title: "Chapter 1: The First Compiler",
    artist: "Grace Hopper",
    album: "The Compiled Tales",
    track: 1,
    frames: 80,
  },
  {
    filename: "grace_hopper_series/the_compiled_tales/chapter02.mp3",
    title: "Chapter 2: COBOL Dawns",
    artist: "Grace Hopper",
    album: "The Compiled Tales",
    track: 2,
    frames: 80,
  },

  // Dual-format book: a single-file audiobook whose (title, artist) is
  // byte-identical to the "Immersive Voyage" EPUB in `make_epub.ts`. The
  // indexer normalizes (title, author) the same way for both libraries and
  // auto-attaches this MP3 to that EPUB, yielding one book with both formats —
  // the precondition for the Immersive Read CTA. TIT2 == TALB so the
  // single-file indexer surfaces the book title (not a chapter title). See
  // `tests/fixtures/dual_format.ts`.
  {
    filename: "alan_turing_solo/immersive_voyage.mp3",
    title: "Immersive Voyage",
    artist: "Alan Turing",
    album: "Immersive Voyage",
    track: 1,
    frames: 80,
  },

  // Merge-only pair. Specs that MUTATE a book via /api/rpc/merge-books own
  // these two and nothing else reads them, so a merge in flight can never
  // change a book another spec is asserting against. Both authors are absent
  // from `make_epub.ts` so no auto-attach fires and no author-scoped count
  // assertion elsewhere shifts. See `tests/fixtures/audiobooks.ts`.
  {
    filename: "barbara_liskov_solo/the_mergeable_manuscript.mp3",
    title: "The Mergeable Manuscript",
    artist: "Barbara Liskov",
    album: "The Mergeable Manuscript",
    track: 1,
    frames: 80,
  },
  {
    filename: "frances_allen_solo/the_severable_sequel.mp3",
    title: "The Severable Sequel",
    artist: "Frances Allen",
    album: "The Severable Sequel",
    track: 1,
    frames: 80,
  },

  // Reserved for the seek-scrubber spec (listen.spec.ts). Scrubbing seeks the
  // audio and PERSISTS a per-(user,book) position, which is globally visible on
  // the shared server — so nothing else may read this book (same isolation
  // rationale as the merge-only pair). Longer than the other fixtures (~30s) so
  // a mid-book seek is meaningful, and the author is absent from make_epub.ts +
  // every other audiobook fixture → no auto-attach and no author-scoped count
  // shift. See `tests/fixtures/audiobooks.ts` (SCRUB_BOOK).
  {
    filename: "shafi_goldwasser_solo/the_scrubbable_saga.mp3",
    title: "The Scrubbable Saga",
    artist: "Shafi Goldwasser",
    album: "The Scrubbable Saga",
    track: 1,
    frames: 1150,
  },
];

// ---------------------------------------------------------------------------
// MPEG-1 Layer III silent-frame primitives
// ---------------------------------------------------------------------------

const MP3_FRAME_HEADER = Buffer.from([0xff, 0xfb, 0x90, 0xc0]);
const MP3_FRAME_SIZE = 417;

/** Per-frame duration: 1152 / 44100 ≈ 0.02612 s. */
export const MP3_FRAME_DURATION_SEC = 1152 / 44100;

function buildSilentMp3Body(nFrames: number): Buffer {
  const body = Buffer.alloc(nFrames * MP3_FRAME_SIZE);
  for (let i = 0; i < nFrames; i++) {
    MP3_FRAME_HEADER.copy(body, i * MP3_FRAME_SIZE);
  }
  return body;
}

// ---------------------------------------------------------------------------
// 1×1 transparent PNG — embedded as the APIC cover image
// ---------------------------------------------------------------------------

/**
 * Minimal valid 1×1 transparent PNG (68 bytes). Same image the epub
 * generator uses — `omnibus_shared::detect_image_format` recognises the
 * 8-byte PNG signature and the thumbnail pipeline decodes it via `image`.
 */
const COVER_PNG = Buffer.from([
  0x89,
  0x50,
  0x4e,
  0x47,
  0x0d,
  0x0a,
  0x1a,
  0x0a, // PNG signature
  0x00,
  0x00,
  0x00,
  0x0d,
  0x49,
  0x48,
  0x44,
  0x52, // IHDR chunk
  0x00,
  0x00,
  0x00,
  0x01,
  0x00,
  0x00,
  0x00,
  0x01,
  0x08,
  0x06,
  0x00,
  0x00,
  0x00,
  0x1f,
  0x15,
  0xc4,
  0x89,
  0x00,
  0x00,
  0x00,
  0x0a,
  0x49,
  0x44,
  0x41, // IDAT chunk
  0x54,
  0x78,
  0x9c,
  0x62,
  0x00,
  0x00,
  0x00,
  0x02,
  0x00,
  0x01,
  0xe5,
  0x27,
  0xde,
  0xfc,
  0x00,
  0x00,
  0x00,
  0x00,
  0x49,
  0x45,
  0x4e,
  0x44,
  0xae,
  0x42, // IEND chunk
  0x60,
  0x82,
]);

// ---------------------------------------------------------------------------
// ID3v2.4 tag primitives
// ---------------------------------------------------------------------------

function syncsafe(n: number): Buffer {
  return Buffer.from([
    (n >> 21) & 0x7f,
    (n >> 14) & 0x7f,
    (n >> 7) & 0x7f,
    n & 0x7f,
  ]);
}

function textFrame(id: string, text: string): Buffer {
  const enc = Buffer.from([0x03]); // UTF-8
  const body = Buffer.concat([
    enc,
    Buffer.from(text, "utf8"),
    Buffer.from([0x00]),
  ]);
  const flags = Buffer.from([0, 0]);
  return Buffer.concat([
    Buffer.from(id, "ascii"),
    syncsafe(body.length),
    flags,
    body,
  ]);
}

/**
 * Build an APIC (Attached Picture) frame for the ID3v2.4 tag.
 *
 * Layout: frame-id(4) + size(4, syncsafe) + flags(2) +
 *         encoding(1, 0x00=ISO-8859-1) + mime(NUL-terminated) +
 *         picture-type(1, 0x03=Cover front) + description(NUL) + data.
 */
function apicFrame(imageData: Buffer, mime: string): Buffer {
  const mimeBytes = Buffer.concat([
    Buffer.from(mime, "ascii"),
    Buffer.from([0x00]),
  ]);
  // encoding=0x00 (ISO-8859-1 for the description string, which is empty)
  // picture_type=0x03 (Cover front)
  // description="" (just a NUL terminator)
  const payload = Buffer.concat([
    Buffer.from([0x00]), // encoding
    mimeBytes, // mime + NUL
    Buffer.from([0x03]), // picture type: Cover (front)
    Buffer.from([0x00]), // description (empty, NUL-terminated)
    imageData,
  ]);
  const flags = Buffer.from([0, 0]);
  return Buffer.concat([
    Buffer.from("APIC", "ascii"),
    syncsafe(payload.length),
    flags,
    payload,
  ]);
}

function buildId3v24Tag(input: AudiobookInput): Buffer {
  const frames = Buffer.concat([
    textFrame("TIT2", input.title),
    textFrame("TPE1", input.artist),
    textFrame("TALB", input.album),
    textFrame("TRCK", String(input.track)),
    apicFrame(COVER_PNG, "image/png"),
  ]);
  const header = Buffer.concat([
    Buffer.from("ID3", "ascii"),
    Buffer.from([0x04, 0x00, 0x00]), // v2.4, no flags
    syncsafe(frames.length),
  ]);
  return Buffer.concat([header, frames]);
}

function buildAudiobook(input: AudiobookInput): Buffer {
  return Buffer.concat([
    buildId3v24Tag(input),
    buildSilentMp3Body(input.frames),
  ]);
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

async function main() {
  const here = dirname(fileURLToPath(import.meta.url));
  const repoRoot = resolve(here, "..", "..", "..");
  const outDir = resolve(repoRoot, "test_data", "audiobooks", "generated");
  mkdirSync(outDir, { recursive: true });

  for (const fx of FIXTURES) {
    const buf = buildAudiobook(fx);
    const path = resolve(outDir, fx.filename);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, buf);
    const seconds = (fx.frames * MP3_FRAME_DURATION_SEC).toFixed(3);
    console.log(`wrote ${path} (${buf.length} bytes, ${seconds}s)`);
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
