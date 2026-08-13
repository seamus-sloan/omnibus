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
 * Usage (from the pnpm project dir):
 *   cd ui_tests/playwright && pnpm exec tsx tools/make_epub.ts
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
  /** Optional publisher colour applied through a class on the chapter body. */
  publisherBodyColor?: string;
  /**
   * When true, fill the chapter with ~32KB of deterministic prose (~32
   * epub.js 1024-char locations) instead of one short paragraph. The
   * cross-format specs need a jump target that is *visually* distinct: a
   * one-location book maps every percentage to the start of the book, so a
   * correct auto-jump and a silently-dropped one render identically.
   */
  longBody?: boolean;
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
    // Mirrors Calibre EPUBs such as The Debt of Time: the class selector beats
    // epub.js's plain body selector unless the reader owns the root foreground.
    publisherBodyColor: "#000000",
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

  // --- Pioneers series, extended (beta=#1, gamma=#2 above) ---
  {
    filename: "pioneers-3.epub",
    id: "urn:omnibus-test:pioneers-3",
    title: "Pioneers Vol 3: Cipher",
    authors: ["Joan Clarke"],
    publisher: "Omnibus Test Press",
    published: "1947-06-12",
    language: "en",
    series: "Pioneers",
    seriesIndex: "3",
    withCover: true,
  },
  {
    filename: "pioneers-4.epub",
    id: "urn:omnibus-test:pioneers-4",
    title: "Pioneers Vol 4: Loop",
    authors: ["Karen Sparck Jones"],
    publisher: "Omnibus Test Press",
    published: "1972-11-02",
    language: "en",
    series: "Pioneers",
    seriesIndex: "4",
    withCover: true,
  },
  {
    filename: "pioneers-5.epub",
    id: "urn:omnibus-test:pioneers-5",
    title: "Pioneers Vol 5: Signal",
    authors: ["Radia Perlman"],
    publisher: "Omnibus Test Press",
    published: "1985-03-15",
    language: "en",
    series: "Pioneers",
    seriesIndex: "5",
    withCover: true,
  },

  // --- Code Quartet (4 books, mostly single-author, one duo, mixed languages) ---
  {
    filename: "code-quartet-1.epub",
    id: "urn:omnibus-test:quartet-1",
    title: "Quartet I: Lexer",
    authors: ["Niklaus Wirth"],
    publisher: "Verlag Algorithmus",
    published: "1976-04-01",
    language: "de",
    series: "Code Quartet",
    seriesIndex: "1",
    withCover: true,
  },
  {
    filename: "code-quartet-2.epub",
    id: "urn:omnibus-test:quartet-2",
    title: "Quartet II: Parser",
    authors: ["Niklaus Wirth"],
    publisher: "Verlag Algorithmus",
    published: "1977-08-12",
    language: "de",
    series: "Code Quartet",
    seriesIndex: "2",
    withCover: true,
  },
  {
    filename: "code-quartet-3.epub",
    id: "urn:omnibus-test:quartet-3",
    title: "Quartet III: Type Checker",
    authors: ["Niklaus Wirth", "Per Brinch Hansen"],
    publisher: "Verlag Algorithmus",
    published: "1978-09-10",
    language: "en",
    series: "Code Quartet",
    seriesIndex: "3",
    withCover: false,
  },
  {
    filename: "code-quartet-4.epub",
    id: "urn:omnibus-test:quartet-4",
    title: "Quartet IV: Codegen",
    authors: ["Niklaus Wirth"],
    publisher: "Verlag Algorithmus",
    published: "1979-12-01",
    language: "en",
    series: "Code Quartet",
    seriesIndex: "4",
    withCover: true,
  },

  // --- Polyglot Tales (3 books, each in a different language) ---
  {
    filename: "polyglot-1.epub",
    id: "urn:omnibus-test:polyglot-1",
    title: "Polyglot Tales: Recits",
    authors: ["Evariste Galois"],
    publisher: "Maison Polyglotte",
    published: "1830-05-29",
    language: "fr",
    series: "Polyglot Tales",
    seriesIndex: "1",
    withCover: true,
  },
  {
    filename: "polyglot-2.epub",
    id: "urn:omnibus-test:polyglot-2",
    title: "Polyglot Tales: Cuentos",
    authors: ["Jorge Luis Borges"],
    publisher: "Maison Polyglotte",
    published: "1944-06-01",
    language: "es",
    series: "Polyglot Tales",
    seriesIndex: "2",
    withCover: true,
  },
  {
    filename: "polyglot-3.epub",
    id: "urn:omnibus-test:polyglot-3",
    title: "Polyglot Tales: Monogatari",
    authors: ["Soseki Natsume"],
    publisher: "Maison Polyglotte",
    published: "1914-04-20",
    language: "ja",
    series: "Polyglot Tales",
    seriesIndex: "3",
    withCover: true,
  },

  // --- Compiler Compendium (3 books, all multi-author with a shared lead) ---
  {
    filename: "compiler-compendium-1.epub",
    id: "urn:omnibus-test:compiler-1",
    title: "Compendium of Compilers I",
    authors: ["Alfred Aho", "Jeffrey Ullman"],
    publisher: "MIT Press Mirror",
    published: "1977-01-01",
    language: "en",
    series: "Compiler Compendium",
    seriesIndex: "1",
    withCover: true,
  },
  {
    filename: "compiler-compendium-2.epub",
    id: "urn:omnibus-test:compiler-2",
    title: "Compendium of Compilers II",
    authors: ["Alfred Aho", "Monica Lam"],
    publisher: "MIT Press Mirror",
    published: "1986-10-15",
    language: "en",
    series: "Compiler Compendium",
    seriesIndex: "2",
    withCover: true,
  },
  {
    filename: "compiler-compendium-3.epub",
    id: "urn:omnibus-test:compiler-3",
    title: "Compendium of Compilers III",
    authors: ["Alfred Aho", "Ravi Sethi", "Jeffrey Ullman"],
    publisher: "MIT Press Mirror",
    published: "2006-08-31",
    language: "en",
    series: "Compiler Compendium",
    seriesIndex: "3",
    withCover: false,
  },

  // --- Mathematica Minor (3 books, mixed languages, mixed cover availability) ---
  {
    filename: "mathematica-minor-1.epub",
    id: "urn:omnibus-test:mathematica-1",
    title: "Minor Lemmas I",
    authors: ["Emmy Noether"],
    publisher: "Klein Mathematik",
    published: "1918-07-23",
    language: "de",
    series: "Mathematica Minor",
    seriesIndex: "1",
    withCover: true,
  },
  {
    filename: "mathematica-minor-2.epub",
    id: "urn:omnibus-test:mathematica-2",
    title: "Minor Lemmas II",
    authors: ["Sophie Germain"],
    publisher: "Klein Mathematik",
    published: "1815-01-08",
    language: "fr",
    series: "Mathematica Minor",
    seriesIndex: "2",
    withCover: true,
  },
  {
    filename: "mathematica-minor-3.epub",
    id: "urn:omnibus-test:mathematica-3",
    title: "Minor Lemmas III",
    authors: ["Sofya Kovalevskaya"],
    publisher: "Klein Mathematik",
    published: "1888-12-24",
    language: "en",
    series: "Mathematica Minor",
    seriesIndex: "3",
    withCover: false,
  },

  // --- Standalones (9 unique-author books spanning languages/publishers) ---
  {
    filename: "standalone-island.epub",
    id: "urn:omnibus-test:standalone-island",
    title: "The Isle of Functions",
    authors: ["Haskell Curry"],
    publisher: "Lambda Books",
    published: "1934-09-12",
    language: "en",
    withCover: true,
  },
  {
    filename: "standalone-garden.epub",
    id: "urn:omnibus-test:standalone-garden",
    title: "The Garden of Closures",
    authors: ["Alonzo Church"],
    publisher: "Lambda Books",
    published: "1936-04-19",
    language: "en",
    withCover: true,
  },
  {
    filename: "standalone-river.epub",
    id: "urn:omnibus-test:standalone-river",
    title: "Rio de Monadas",
    authors: ["Maria Zambrano"],
    publisher: "Editorial Omnibus",
    published: "1957-03-03",
    language: "es",
    withCover: false,
  },
  {
    filename: "standalone-mountain.epub",
    id: "urn:omnibus-test:standalone-mountain",
    title: "Berg der Beweise",
    authors: ["David Hilbert"],
    publisher: "Klein Mathematik",
    published: "1900-08-08",
    language: "de",
    withCover: true,
  },
  {
    filename: "standalone-forest.epub",
    id: "urn:omnibus-test:standalone-forest",
    title: "La Foret des Algorithmes",
    authors: ["Henri Poincare"],
    publisher: "Maison Polyglotte",
    published: "1899-11-17",
    language: "fr",
    withCover: true,
  },
  {
    filename: "standalone-ocean.epub",
    id: "urn:omnibus-test:standalone-ocean",
    title: "Ocean of Bytes",
    authors: ["Donald Knuth", "Leslie Lamport"],
    publisher: "MIT Press Mirror",
    published: "1989-06-21",
    language: "en",
    withCover: true,
  },
  {
    filename: "standalone-desert.epub",
    id: "urn:omnibus-test:standalone-desert",
    title: "Desert Protocols",
    authors: ["Vint Cerf", "Bob Kahn"],
    publisher: "Omnibus Test Press",
    published: "1974-05-10",
    language: "en",
    withCover: false,
  },
  {
    filename: "standalone-meadow.epub",
    id: "urn:omnibus-test:standalone-meadow",
    title: "Meadow of Bits",
    authors: ["Claude Shannon"],
    publisher: "Omnibus Test Press",
    published: "1948-07-30",
    language: "en",
    withCover: true,
  },
  {
    filename: "standalone-canyon.epub",
    id: "urn:omnibus-test:standalone-canyon",
    title: "Canyon Echoes",
    authors: ["Annie Easley"],
    publisher: "Lambda Books",
    published: "1955-04-23",
    language: "en",
    withCover: true,
  },
  {
    filename: "standalone-tundra.epub",
    id: "urn:omnibus-test:standalone-tundra",
    title: "Tundra Signals",
    authors: ["Mary Jackson"],
    publisher: "Omnibus Test Press",
    published: "1958-09-12",
    language: "en",
    withCover: true,
  },

  // --- Dual-format book: this EPUB shares a normalized (title, author) with
  // the "Immersive Voyage" audiobook in `make_audiobook.ts`, so the indexer
  // auto-attaches the pair into ONE book carrying both formats — the
  // precondition for the Immersive Read CTA (immersive.spec.ts). Keep the
  // title/author byte-identical across both generators so the attached book's
  // metadata is the same regardless of which library indexes first. See
  // `tests/fixtures/dual_format.ts`.
  {
    filename: "immersive-voyage.epub",
    id: "urn:omnibus-test:immersive-voyage",
    title: "Immersive Voyage",
    authors: ["Alan Turing"],
    publisher: "Omnibus Test Press",
    published: "1950-10-01",
    language: "en",
    withCover: true,
  },

  // --- Second dual-format pair, RESERVED for cross_format_prompts.spec.ts:
  // that spec links the formats and writes reading/listening progress, all
  // globally visible server state — no other spec may touch this book. Same
  // byte-identical (title, author) rule as "Immersive Voyage"; the author
  // appears nowhere else in either generator, and the title sorts before
  // "The Isle of Functions" so the landing sort test is undisturbed.
  {
    filename: "parallel-latitudes.epub",
    id: "urn:omnibus-test:parallel-latitudes",
    title: "Parallel Latitudes",
    authors: ["Vera Molnar"],
    publisher: "Omnibus Test Press",
    published: "1968-05-01",
    language: "en",
    withCover: true,
    // Cross-format jump assertions need real location granularity — see
    // the `longBody` doc on EpubInput.
    longBody: true,
  },

  // --- Bulk-edit targets (2 books) ---
  // Reserved for landing_bulk_edit.spec.ts, which bulk-writes publisher/tag
  // overrides to BOTH books (reverted at test end, but the suite is
  // fullyParallel, so no other spec may read them). Author is unique across
  // ALL fixtures (ebook + audiobook) — shelves.spec.ts asserts exact
  // author-scoped match counts.
  {
    filename: "bulk-target-1.epub",
    id: "urn:omnibus-test:bulk-target-1",
    title: "Bulk Target One",
    authors: ["Katherine Johnson"],
    publisher: "Bulk Test House",
    published: "1972-02-02",
    language: "en",
    withCover: true,
  },
  {
    filename: "bulk-target-2.epub",
    id: "urn:omnibus-test:bulk-target-2",
    title: "Bulk Target Two",
    authors: ["Katherine Johnson"],
    publisher: "Bulk Test House",
    published: "1974-03-03",
    language: "en",
    withCover: true,
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
    "890000000b49444154789c6360000200000500017a5eab3f0000000049454e44ae426082",
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
  const date = input.published
    ? `\n    <dc:date>${escapeXml(input.published)}</dc:date>`
    : "";
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
  const styleManifestItem = input.publisherBodyColor
    ? `\n    <item id="publisher-style" href="publisher.css" media-type="text/css"/>`
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
    <item id="chap1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>${coverManifestItem}${styleManifestItem}
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

function buildChapter(input: EpubInput): string {
  const stylesheet = input.publisherBodyColor
    ? `<link href="publisher.css" rel="stylesheet" type="text/css"/>`
    : "";
  const bodyClass = input.publisherBodyColor ? ` class="calibre"` : "";
  // Deterministic (no randomness) so the generated zip stays byte-stable.
  const prose = input.longBody
    ? Array.from({ length: 40 }, (_, i) => {
        const sentence =
          `Paragraph ${i + 1} of the long-body fixture. ` +
          "The expedition charted latitude after latitude, logging each " +
          "reading twice and comparing the drift against the morning's " +
          "sightings before the fog closed over the instruments again. ".repeat(
            4,
          );
        return `<p>${sentence.trim()}</p>`;
      }).join("\n")
    : "<p>Synthetic test content.</p>";
  return `<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>${escapeXml(input.title)}</title>${stylesheet}</head>
<body${bodyClass}><h1>${escapeXml(input.title)}</h1>${prose}</body>
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
  zip.file("OEBPS/chapter1.xhtml", buildChapter(input), { date: FIXED_DATE });
  if (input.publisherBodyColor) {
    zip.file(
      "OEBPS/publisher.css",
      `.calibre { color: ${input.publisherBodyColor}; }\n`,
      { date: FIXED_DATE },
    );
  }
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
    console.log(
      `wrote ${path} (${buf.length} bytes, slug=${slugFromFilename(fx.filename)})`,
    );
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
