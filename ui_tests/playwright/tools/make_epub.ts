/**
 * Synthetic EPUB generator for Playwright fixtures.
 *
 * Produces minimal valid EPUB3 files with arbitrary metadata so the landing
 * spec can assert against known values. Intentionally tiny: a single chapter,
 * a nav doc, and an optional 1×1 PNG cover.
 *
 * Output is deterministic — given the same inputs, the resulting bytes are
 * identical run-to-run. Generated files are committed under
 * `test_data/epubs/generated/` so CI does not need to run this tool.
 *
 * Usage:
 *   npx tsx ui_tests/playwright/tools/make_epub.ts
 *
 * To add a new fixture, edit FIXTURES below and re-run.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import JSZip from "jszip";

interface EpubInput {
  /** Output filename (no path). */
  filename: string;
  title: string;
  authors: string[];
  publisher?: string;
  /** ISO date string (YYYY-MM-DD). */
  published?: string;
  /** BCP-47 language code (e.g. "en"). */
  language: string;
  series?: string;
  seriesIndex?: string;
  /** When true, embed a 1×1 transparent PNG cover. */
  withCover?: boolean;
  /** Stable identifier used in the OPF and as the unique-identifier. */
  id: string;
}

const FIXTURES: EpubInput[] = [
  {
    filename: "alpha.epub",
    id: "urn:omnibus-test:alpha",
    title: "Alpha",
    authors: ["Ada Lovelace"],
    publisher: "Omnibus Test Press",
    published: "1843-10-01",
    language: "en",
    withCover: true,
  },
  {
    filename: "beta.epub",
    id: "urn:omnibus-test:beta",
    title: "Beta in the Series",
    authors: ["Grace Hopper", "Margaret Hamilton"],
    publisher: "Omnibus Test Press",
    published: "1969-07-20",
    language: "en",
    series: "Pioneers",
    seriesIndex: "1",
    withCover: true,
  },
  {
    filename: "gamma.epub",
    id: "urn:omnibus-test:gamma",
    title: "Gamma sin Cover",
    authors: ["Hedy Lamarr"],
    publisher: "Editorial Omnibus",
    published: "1942-08-11",
    language: "es",
    series: "Pioneers",
    seriesIndex: "2",
    withCover: false,
  },
];

/**
 * Slug derivation for fixture filenames. Matches the slug helper used on the
 * landing-page row testid so spec selectors line up with the file on disk.
 */
function slugFromFilename(name: string): string {
  return name
    .replace(/\.epub$/i, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/** Smallest possible PNG: 1×1 transparent pixel. Bytes are public-domain. */
const TINY_PNG = Buffer.from(
  "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4" +
    "890000000d49444154789c6300010000000500010d0a2db40000000049454e44ae426082",
  "hex",
);

function escapeXml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&apos;");
}

function buildOpf(input: EpubInput): string {
  const creators = input.authors
    .map(
      (name, i) =>
        `    <dc:creator id="creator-${i}">${escapeXml(name)}</dc:creator>\n` +
        `    <meta refines="#creator-${i}" property="role" scheme="marc:relators">aut</meta>`,
    )
    .join("\n");
  const publisher = input.publisher
    ? `\n    <dc:publisher>${escapeXml(input.publisher)}</dc:publisher>`
    : "";
  const date = input.published ? `\n    <dc:date>${escapeXml(input.published)}</dc:date>` : "";
  const series = input.series
    ? `\n    <meta property="belongs-to-collection" id="c01">${escapeXml(input.series)}</meta>` +
      `\n    <meta refines="#c01" property="collection-type">series</meta>` +
      (input.seriesIndex
        ? `\n    <meta refines="#c01" property="group-position">${escapeXml(input.seriesIndex)}</meta>`
        : "")
    : "";
  const coverManifestItem = input.withCover
    ? `\n    <item id="cover-image" href="cover.png" media-type="image/png" properties="cover-image"/>`
    : "";

  return `<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="bookid">${escapeXml(input.id)}</dc:identifier>
    <dc:title>${escapeXml(input.title)}</dc:title>
    <dc:language>${escapeXml(input.language)}</dc:language>${publisher}${date}
${creators}${series}
    <meta property="dcterms:modified">2024-01-01T00:00:00Z</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="chap1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>${coverManifestItem}
  </manifest>
  <spine>
    <itemref idref="chap1"/>
  </spine>
</package>
`;
}

const CONTAINER_XML = `<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>
`;

const NAV_XHTML = `<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Nav</title></head>
<body>
<nav epub:type="toc"><ol><li><a href="chapter1.xhtml">Chapter 1</a></li></ol></nav>
</body>
</html>
`;

function buildChapter(title: string): string {
  return `<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>${escapeXml(title)}</title></head>
<body><h1>${escapeXml(title)}</h1><p>Synthetic test content.</p></body>
</html>
`;
}

// Fixed timestamp so generated zip headers are byte-stable across runs.
const FIXED_DATE = new Date("2024-01-01T00:00:00Z");

async function buildEpub(input: EpubInput): Promise<Buffer> {
  const zip = new JSZip();
  // The mimetype file MUST be the first entry and stored without compression
  // for the EPUB to validate.
  zip.file("mimetype", "application/epub+zip", {
    compression: "STORE",
    date: FIXED_DATE,
  });
  zip.file("META-INF/container.xml", CONTAINER_XML, { date: FIXED_DATE });
  zip.file("OEBPS/content.opf", buildOpf(input), { date: FIXED_DATE });
  zip.file("OEBPS/nav.xhtml", NAV_XHTML, { date: FIXED_DATE });
  zip.file("OEBPS/chapter1.xhtml", buildChapter(input.title), { date: FIXED_DATE });
  if (input.withCover) {
    zip.file("OEBPS/cover.png", TINY_PNG, { date: FIXED_DATE });
  }
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
    const buf = await buildEpub(fx);
    const path = resolve(outDir, fx.filename);
    writeFileSync(path, buf);
    console.log(`wrote ${path} (${buf.length} bytes, slug=${slugFromFilename(fx.filename)})`);
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
