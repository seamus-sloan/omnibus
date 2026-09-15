import { resolve } from "node:path";

import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { audiobookFixturesDir, fixturesDir } from "../utils/seed";

// A committed EPUB fixture to feed the file input. Any valid EPUB works — the
// inspect endpoint parses its embedded metadata.
const SAMPLE_EPUB = resolve(fixturesDir(), "generated", "beta.epub");
// A public-domain PDF (fetched with the fixtures release) whose Info dict
// carries a title and author for the form to pre-fill.
const SAMPLE_PDF = resolve(fixturesDir(), "public_domain", "flatland.pdf");

// The committed multi-part MP3 audiobook fixture — two chapters that the
// inspect endpoint groups into one book.
const AUDIOBOOK_PARTS = [
  resolve(
    audiobookFixturesDir(),
    "generated",
    "grace_hopper_series",
    "the_compiled_tales",
    "chapter01.mp3",
  ),
  resolve(
    audiobookFixturesDir(),
    "generated",
    "grace_hopper_series",
    "the_compiled_tales",
    "chapter02.mp3",
  ),
];

const fileInput = (page: import("@playwright/test").Page) =>
  page.getByTestId("add-books-file-input");
const status = (page: import("@playwright/test").Page) =>
  page.getByTestId("add-books-status");

test("renders the add-books layout", async ({ page }) => {
  await gotoReady(page, "/add-books");

  await expect(
    page.getByRole("heading", { name: "Upload a book" }),
  ).toBeVisible();
  // One picker for every format — there is no ebook/audiobook toggle to click
  // first; the extension of what you pick decides the ingest.
  await expect(fileInput(page)).toBeVisible();
  await expect(fileInput(page)).toHaveAttribute("accept", /\.epub/);
  await expect(fileInput(page)).toHaveAttribute("accept", /\.pdf/);
  await expect(fileInput(page)).toHaveAttribute("accept", /\.m4b/);
  await expect(fileInput(page)).toHaveAttribute("multiple");
  await expect(page.getByTestId("add-books-formats")).toBeVisible();
  await expect(page.getByTestId("add-books-type-ebook")).toHaveCount(0);
  await expect(page.getByTestId("add-books-type-audiobook")).toHaveCount(0);
  await expectNavVisible(page);
});

test("auto-fills the editable form from an uploaded EPUB", async ({ page }) => {
  await gotoReady(page, "/add-books");

  // Selecting a file kicks off the inspect round-trip; the form appears
  // pre-filled with the embedded metadata.
  await expectMutation(
    page,
    { method: "POST", url: "/api/uploads/ebooks/inspect", expectedStatus: 200 },
    async () => fileInput(page).setInputFiles(SAMPLE_EPUB),
  );

  await expect(page.getByTestId("add-books-submit")).toBeVisible();
  // The title field is populated from the embedded metadata (non-empty).
  await expect(page.getByLabel("Title")).not.toHaveValue("");
  // beta.epub declares two creators: the first fills Author and the rest are
  // listed beneath it, so the form never under-reports what it will save
  // (#2355).
  await expect(page.getByLabel("Author")).toHaveValue("Grace Hopper");
  await expect(page.getByTestId("add-books-more-creators")).toContainText(
    "Margaret Hamilton",
  );
});

test("auto-fills the editable form from an uploaded PDF", async ({ page }) => {
  await gotoReady(page, "/add-books");

  await expectMutation(
    page,
    { method: "POST", url: "/api/uploads/ebooks/inspect", expectedStatus: 200 },
    async () => fileInput(page).setInputFiles(SAMPLE_PDF),
  );

  await expect(page.getByTestId("add-books-submit")).toBeVisible();
  // The Info dict fills both fields — a PDF is inspected by the same parser
  // the scan uses, so what the form shows is what the library would index.
  await expect(page.getByLabel("Title")).toHaveValue(
    "Flatland: A Romance of Many Dimensions",
  );
  await expect(page.getByLabel("Author")).toHaveValue("Edwin Abbott Abbott");
});

test("surfaces an error when inspect fails", async ({ page }) => {
  await gotoReady(page, "/add-books");

  // Force the inspect call to fail server-side.
  await page.route("**/api/uploads/ebooks/inspect", (route) =>
    route.fulfill({ status: 500, body: "boom" }),
  );

  await expectMutation(
    page,
    { method: "POST", url: "/api/uploads/ebooks/inspect", expectedStatus: 500 },
    async () => fileInput(page).setInputFiles(SAMPLE_EPUB),
  );

  // The status region surfaces the failure, and the confirm form never appears.
  await expect(status(page)).toBeVisible();
  await expect(status(page)).toHaveClass(/error/);
  await expect(page.getByTestId("add-books-submit")).toHaveCount(0);
});

test("auto-fills the form from a multi-part MP3 audiobook", async ({
  page,
}) => {
  await gotoReady(page, "/add-books");

  // Both .mp3 parts in one pick, and the page routes them to the audiobook
  // ingest from the extension alone.
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/uploads/audiobooks/inspect",
      expectedStatus: 200,
    },
    async () => fileInput(page).setInputFiles(AUDIOBOOK_PARTS),
  );

  await expect(page.getByTestId("add-books-submit")).toBeVisible();
  // The title is derived from the shared album tag across the parts.
  await expect(page.getByLabel("Title")).not.toHaveValue("");
  // The audiobook parser extracts no series, which is exactly why the fields
  // have to be offered here (#2254) — this is the only point in the flow that
  // can supply one.
  await expect(page.getByLabel("Series", { exact: true })).toBeVisible();
  await expect(page.getByLabel("Series index")).toBeVisible();
});

test("refuses a pick that mixes an EPUB with audiobook parts", async ({
  page,
}) => {
  await gotoReady(page, "/add-books");

  // Neither inspect endpoint may be asked about a pick that fits neither, so
  // any request here is a failure, not a slow assertion.
  const sent: string[] = [];
  page.on("request", (req) => {
    if (req.url().includes("/api/uploads/")) sent.push(req.url());
  });

  await fileInput(page).setInputFiles([SAMPLE_EPUB, AUDIOBOOK_PARTS[0]!]);

  await expect(status(page)).toBeVisible();
  await expect(status(page)).toHaveClass(/error/);
  await expect(status(page)).toContainText("not both");
  await expect(page.getByTestId("add-books-submit")).toHaveCount(0);
  expect(sent).toEqual([]);
});

test("names a file neither ingest takes instead of sending it", async ({
  page,
}) => {
  await gotoReady(page, "/add-books");

  const sent: string[] = [];
  page.on("request", (req) => {
    if (req.url().includes("/api/uploads/")) sent.push(req.url());
  });

  // `accept` only shapes the dialog; a drop or a lenient browser can still
  // hand the page anything, so the routing has to refuse it by name.
  await fileInput(page).setInputFiles({
    name: "notes.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("not a book"),
  });

  await expect(status(page)).toBeVisible();
  await expect(status(page)).toHaveClass(/error/);
  await expect(status(page)).toContainText("notes.txt");
  await expect(page.getByTestId("add-books-submit")).toHaveCount(0);
  expect(sent).toEqual([]);
});
