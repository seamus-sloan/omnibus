// Admin "Delete files…" dialog on the book detail page.
//
// Serial mode, and every destructive test operates on a book this spec
// created itself (an uploaded EPUB, or a fileless physical-only record).
// Committed fixtures are only ever used for the read-only layout and the
// intercepted-error paths — a spec that actually deleted one would remove the
// file from `test_data/` and break every other flow on the shared server.
import { existsSync, readFileSync, rmSync } from "node:fs";
import { resolve } from "node:path";

import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

test.describe.configure({ mode: "serial" });

// An EPUB-only fixture used read-only, by the layout and error-path tests.
const FIXTURE = FIXTURE_BOOKS.find((b) => b.slug === "beta")!;

// The uploaded book's identity. Distinct from every committed fixture so it
// can't auto-attach to one, and so its canonical folder is ours to remove.
const UPLOAD_TITLE = "Deletable Test Volume";
const UPLOAD_AUTHOR = "Delete Spec";
// `library_layout` slugs `<author>/<title>/` under the library root.
const UPLOAD_AUTHOR_DIR = resolve(fixturesDir(), "delete-spec");

test.beforeAll(async ({ request }) => {
  test.setTimeout(120_000);
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// Belt-and-braces: if a delete assertion failed mid-test, don't leave the
// uploaded file inside the committed fixtures tree.
test.afterAll(() => {
  rmSync(UPLOAD_AUTHOR_DIR, { recursive: true, force: true });
});

/** Upload an EPUB into the library and return the new book's uuid. */
async function uploadDeletableBook(
  request: import("@playwright/test").APIRequestContext,
): Promise<string> {
  const source = resolve(fixturesDir(), "generated", "alpha.epub");
  const resp = await request.post("/api/uploads/ebooks", {
    multipart: {
      file: {
        name: "deletable.epub",
        mimeType: "application/epub+zip",
        buffer: readFileSync(source),
      },
      title: UPLOAD_TITLE,
      author: UPLOAD_AUTHOR,
    },
  });
  // 201 Created — the commit handler files the upload and returns the new uuid.
  expect(resp.status(), "POST /api/uploads/ebooks failed").toBe(201);
  const { uuid } = (await resp.json()) as { uuid: string };
  return uuid;
}

/** Create a fileless physical-only book and return its uuid. */
async function createPhysicalOnlyBook(
  request: import("@playwright/test").APIRequestContext,
  title: string,
): Promise<string> {
  const resp = await request.post("/api/scan/physical-only", {
    data: {
      meta: {
        isbn13: "9781234567897",
        title,
        authors: ["Delete Spec"],
        year: null,
        pages: null,
        publisher: null,
        description: null,
        cover_url: null,
        source: "open_library",
      },
      note: null,
    },
  });
  expect(resp.status(), "POST /api/scan/physical-only failed").toBe(200);
  const { book_uuid } = (await resp.json()) as { book_uuid: string };
  return book_uuid;
}

/** Whether the book still resolves from the library RPC. */
async function bookExists(
  request: import("@playwright/test").APIRequestContext,
  uuid: string,
): Promise<boolean> {
  const resp = await request.post("/api/rpc/ebook", { data: { uuid } });
  if (resp.status() !== 200) return false;
  const body = (await resp.json()) as unknown;
  return body !== null;
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

test("renders the delete affordance on the book detail layout", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, FIXTURE.title);
  await gotoReady(page, `/books/${uuid}`);
  await expectNavVisible(page);

  // Admin storageState: the rail carries "Delete files…" alongside the
  // existing edit/merge affordances.
  await expect(page.getByTestId("edit-metadata")).toBeVisible();
  const deleteFiles = page.getByTestId("delete-files");
  await expect(deleteFiles).toBeVisible();

  await deleteFiles.click();
  const dialog = page.getByTestId("delete-book-dialog");
  await expect(dialog).toBeVisible();
  await expect(page.getByTestId("delete-select-all")).toBeVisible();
  // One checkbox row per file, and nothing is selected until the user picks.
  await expect(dialog.getByRole("checkbox")).toHaveCount(1);
  await expect(page.getByTestId("delete-continue")).toBeDisabled();

  await page.getByTestId("delete-cancel").click();
  await expect(dialog).not.toBeVisible();
});

// ---------------------------------------------------------------------------
// Action — confirm copy (no mutation; the fixture book must survive)
// ---------------------------------------------------------------------------

test("warns that deleting every file also removes the book", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, FIXTURE.title);
  await gotoReady(page, `/books/${uuid}`);

  await page.getByTestId("delete-files").click();
  await page.getByTestId("delete-select-all").click();
  await page.getByTestId("delete-continue").click();

  await expect(page.getByTestId("delete-confirm-copy")).toContainText(
    "removed from your library entirely",
  );
  // Back returns to the checklist with the selection intact, and Cancel
  // leaves the book untouched.
  await page.getByTestId("delete-back").click();
  await expect(page.getByTestId("delete-select-all")).toBeVisible();
  await page.getByTestId("delete-cancel").click();
  expect(await bookExists(request, uuid)).toBe(true);
});

// ---------------------------------------------------------------------------
// Action — error path (intercepted, so nothing is actually deleted)
// ---------------------------------------------------------------------------

test("surfaces a server error and leaves the book in place", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, FIXTURE.title);
  await gotoReady(page, `/books/${uuid}`);

  await page.route("**/api/rpc/books/delete-files", (route) =>
    route.fulfill({ status: 500, contentType: "text/plain", body: "delete exploded" }),
  );

  await page.getByTestId("delete-files").click();
  await page.getByTestId("delete-select-all").click();
  await page.getByTestId("delete-continue").click();
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/books/delete-files", expectedStatus: 500 },
    async () => page.getByTestId("delete-confirm").click(),
  );

  // The dialog stays open with an alert; the book is still there.
  await expect(page.getByRole("alert")).toBeVisible();
  await page.getByTestId("delete-cancel").click();
  expect(await bookExists(request, uuid)).toBe(true);
});

// ---------------------------------------------------------------------------
// Action — real deletes, on books this spec created
// ---------------------------------------------------------------------------

test("deletes an uploaded ebook, its record, and its file on disk", async ({ page, request }) => {
  test.slow();
  const uuid = await uploadDeletableBook(request);
  await gotoReady(page, `/books/${uuid}`);

  await page.getByTestId("delete-files").click();
  await page.getByTestId("delete-select-all").click();
  await page.getByTestId("delete-continue").click();
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/books/delete-files",
      expectedStatus: 200,
    },
    async () => page.getByTestId("delete-confirm").click(),
  );

  // Last file gone → the book goes too, and the page returns to the library.
  await expect(page).toHaveURL(/\/$/);
  expect(await bookExists(request, uuid)).toBe(false);
  // The canonical `<author>/<title>/` folders are pruned along with the file.
  expect(existsSync(UPLOAD_AUTHOR_DIR)).toBe(false);
});

test("deletes a physical-only book by un-recording its copy", async ({ page, request }) => {
  const uuid = await createPhysicalOnlyBook(request, "Deletable Physical Copy");
  await gotoReady(page, `/books/${uuid}`);

  await page.getByTestId("delete-files").click();
  // No files, but the physical copy is a deletable item in its own section.
  const dialog = page.getByTestId("delete-book-dialog");
  await expect(dialog).toContainText("PHYSICAL COPIES");
  await expect(dialog).toContainText("no file on disk");
  await page.getByTestId("delete-select-all").click();
  await expect(page.getByTestId("delete-continue")).toHaveText(/Delete 1 item/);
  await page.getByTestId("delete-continue").click();

  // Every item selected → the record goes.
  await expect(page.getByTestId("delete-confirm-copy")).toContainText(
    "removed from your library entirely",
  );
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/books/delete-files", expectedStatus: 200 },
    async () => page.getByTestId("delete-confirm").click(),
  );

  await expect(page).toHaveURL(/\/$/);
  expect(await bookExists(request, uuid)).toBe(false);
});
