/**
 * Synthetic MP3 generator for Playwright audiobook fixtures.
 *
 * Produces minimal valid MPEG-1 Layer III silent MP3 files with an ID3v2.3
 * tag block carrying TIT2 / TPE1 / TALB / TRCK so the omnibus indexer
 * (`db::audiobook::parse_groups`) can lift title / artist / album / track —
 * the same per-part fields the listen-page spec asserts against.
 *
 * Output is deterministic — given the same inputs, the resulting bytes are
 * identical run-to-run. Generated files are committed under
 * `test_data/audiobooks/generated/` so CI does not need to run this tool.
 *
 * Usage:
 *   npx tsx ui_tests/playwright/tools/make_audiobook.ts
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
  /** Output filename (no path). */
  filename: string;
  /** ID3 TIT2 (track / chapter title). For single-file audiobooks this is
   *  the book title; for multi-file mp3 folders this is the chapter title. */
  title: string;
  /** ID3 TPE1 (primary artist). The indexer maps this to the book's first
   *  creator. */
  artist: string;
  /** ID3 TALB (album). The indexer prefers TALB over TIT2 as the *book*
   *  title for multi-file groups, so set this to the book title. */
  album: string;
  /** ID3 TRCK (track number). Drives sort order within an mp3 folder. */
  track: number;
  /** Number of silent MP3 frames to emit. Each frame is 26.122 ms at the
   *  hardcoded MPEG-1 Layer III / 128 kbps / 44.1 kHz / mono parameters.
   *  ~38 frames ≈ 1 second; 80 frames ≈ 2.09 seconds. */
  frames: number;
}

const FIXTURES: AudiobookInput[] = [
  // Single-file audiobook (`ada_lovelace_solo/the_analytical_audiobook.mp3`).
  // The omnibus indexer treats a top-level MP3 with no sibling MP3s in
  // the same directory as a one-part book. We place it in a unique
  // subdirectory so the grouping logic surfaces it as its own
  // AudiobookGroup with parts.len() == 1.
  {
    filename: "ada_lovelace_solo/the_analytical_audiobook.mp3",
    title: "The Analytical Audiobook",
    artist: "Ada Lovelace",
    album: "The Analytical Audiobook",
    track: 1,
    frames: 80,
  },

  // Multi-file audiobook (`grace_hopper_series/the_compiled_tales/chapter01.mp3`
  // and `…/chapter02.mp3`). Two MP3s in the same directory form one
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
];

// -----------------------------------------------------------------------------
// MPEG-1 Layer III silent-frame primitives
// -----------------------------------------------------------------------------

/**
 * Header bytes for an MPEG-1 Layer III frame at 128 kbps / 44.1 kHz / mono /
 * no padding / no CRC. Computed bit-by-bit:
 *   sync(11)=0x7FF version(2)=11=MPEG1 layer(2)=01=III protect(1)=1=noCRC
 *   bitrate(4)=1001=128kbps samplerate(2)=00=44.1kHz padding(1)=0 private(1)=0
 *   channel(2)=11=mono modeext(2)=00 copyright(1)=0 original(1)=0 emphasis(2)=00
 *   → FF FB 90 C0
 */
const MP3_FRAME_HEADER = Buffer.from([0xff, 0xfb, 0x90, 0xc0]);

/**
 * Frame size in bytes = `floor(144 * bitrate / samplerate) + padding`
 * = `floor(144 * 128000 / 44100) + 0` = 417 bytes.
 */
const MP3_FRAME_SIZE = 417;

/**
 * Frame duration in seconds = `1152 / samplerate` = `1152 / 44100` ≈
 * `0.02612244898`. Exported so the fixture table can derive the expected
 * per-file duration alongside the generator inputs.
 */
export const MP3_FRAME_DURATION_SEC = 1152 / 44100;

/**
 * Build a silent MP3 payload made of `nFrames` MPEG-1 Layer III frames.
 * Each frame is the canonical header followed by 413 zero bytes — a
 * valid (and totally silent) side-info block. `lofty` parses this as
 * standard MPEG audio with duration `nFrames * MP3_FRAME_DURATION_SEC`.
 */
function buildSilentMp3Body(nFrames: number): Buffer {
  const body = Buffer.alloc(nFrames * MP3_FRAME_SIZE);
  for (let i = 0; i < nFrames; i++) {
    MP3_FRAME_HEADER.copy(body, i * MP3_FRAME_SIZE);
    // remainder of each frame stays as zero bytes (silent payload).
  }
  return body;
}

// -----------------------------------------------------------------------------
// ID3v2.3 tag primitives
// -----------------------------------------------------------------------------

/**
 * Encode `n` as the 4-byte syncsafe integer ID3v2 sizes use (7 bits per
 * byte, MSB always 0).
 */
function syncsafe(n: number): Buffer {
  return Buffer.from([(n >> 21) & 0x7f, (n >> 14) & 0x7f, (n >> 7) & 0x7f, n & 0x7f]);
}

/**
 * Build one ID3v2.3 text-frame: 4-byte ID, 4-byte big-endian size, 2-byte
 * flags (`0x00 0x00`), 1-byte encoding marker (`0x03` = UTF-8), the UTF-8
 * payload, and a trailing NUL terminator. Sufficient for TIT2, TPE1,
 * TALB, TRCK — the four fields the audiobook indexer reads via
 * `lofty::tag::Accessor`.
 */
function textFrame(id: string, text: string): Buffer {
  const enc = Buffer.from([0x03]); // UTF-8
  const body = Buffer.concat([enc, Buffer.from(text, "utf8"), Buffer.from([0x00])]);
  const flags = Buffer.from([0, 0]);
  const size = Buffer.alloc(4);
  size.writeUInt32BE(body.length, 0);
  return Buffer.concat([Buffer.from(id, "ascii"), size, flags, body]);
}

/**
 * Assemble the full ID3v2.3 header + frame block for an audiobook part.
 * `track` rides as the TRCK frame which `lofty` reads as a `u32` —
 * drives the in-folder sort order in `db::audiobook::parse_groups`.
 */
function buildId3v23Tag(input: AudiobookInput): Buffer {
  const frames = Buffer.concat([
    textFrame("TIT2", input.title),
    textFrame("TPE1", input.artist),
    textFrame("TALB", input.album),
    textFrame("TRCK", String(input.track)),
  ]);
  const header = Buffer.concat([
    Buffer.from("ID3", "ascii"),
    Buffer.from([0x03, 0x00, 0x00]), // version 2.3, revision 0, flags 0
    syncsafe(frames.length),
  ]);
  return Buffer.concat([header, frames]);
}

function buildAudiobook(input: AudiobookInput): Buffer {
  return Buffer.concat([buildId3v23Tag(input), buildSilentMp3Body(input.frames)]);
}

// -----------------------------------------------------------------------------
// Driver
// -----------------------------------------------------------------------------

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
