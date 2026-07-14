import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectMutation } from "../utils/api";
import { fetchBookIdByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Minimal valid 1x1 PNG. detect_image_format only inspects magic bytes, so
// the body can be tiny — duplicates the constant already used by
// author_photo.spec.ts for the same reason.
const TINY_PNG = Buffer.from([
  0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49,
  0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06,
  0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44,
  0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d,
  0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42,
  0x60, 0x82,
]);

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// Alpha fixture: standalone, single author, has cover.
const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "alpha")!;

// Every test in this file mutates / reads override state on the same
// `Alpha` book (the layout test reads the title, "edits title and saves"
// writes "Alpha Edited", "reverts" deletes that override, "adds and
// removes tags" writes + reverts a tag-only override, "discard" enters
// the form). Under Playwright's default `fullyParallel: true` these tests
// race across workers — "reverts" runs before "edits" commits the
// override, so `fetchBookIdByTitle("Alpha Edited")` 404s and the revert
// button never appears. `describe.serial` pins them to a single worker so
// the source order is also the execution order.
test.describe.serial("metadata edit flow", () => {

// ---------------------------------------------------------------------------
// Layout test
// ---------------------------------------------------------------------------

test("renders the metadata edit form with pre-populated fields", async ({ page, request }) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}/edit`);

  // Page header renders with the book title and "Edit metadata" label.
  // Disambiguated by testid: "Edit metadata" also appears in the breadcrumb tail.
  await expect(page.getByTestId("me-page-title-label")).toBeVisible();
  // Title text appears in the breadcrumb link, the page heading, and the
  // identifier "urn:omnibus-test:alpha" — substring-match it under the h2 so
  // strict mode resolves to a single element.
  await expect(
    page.getByRole("heading", { level: 2 }).getByText(TARGET.title),
  ).toBeVisible();

  // Breadcrumb navigation is present with "Home" link.
  await expect(
    page.getByRole("navigation", { name: "breadcrumb" }).getByRole("link", { name: "Home" }),
  ).toBeVisible();

  // Title input is pre-populated with the fixture's title.
  const titleInput = page.getByLabel("Title");
  await expect(titleInput).toBeVisible();
  await expect(titleInput).toHaveValue(TARGET.title);

  // Author chip is visible. "Ada Lovelace" appears in the breadcrumb,
  // page heading, and chip — scope to the chip via its enclosing class.
  await expect(
    page.locator(".me-chip-item").getByText(TARGET.authors[0]),
  ).toBeVisible();

  // Save bar is present.
  await expect(page.getByTestId("me-save")).toBeVisible();
  await expect(page.getByTestId("me-discard")).toBeVisible();

  // Save is initially disabled (no dirty fields).
  await expect(page.getByTestId("me-save")).toBeDisabled();
});

// ---------------------------------------------------------------------------
// Edit title -> save -> detail page reflects change
// ---------------------------------------------------------------------------

test("edits title and saves overrides", async ({ page, request }) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}/edit`);

  // Change the title.
  const titleInput = page.getByLabel("Title");
  await titleInput.clear();
  await titleInput.fill("Alpha Edited");

  // Save bar should show 1 field edited and button should be enabled.
  await expect(page.getByTestId("me-save")).toBeEnabled();
  await expect(page.getByText("1 field edited")).toBeVisible();

  // Click save; expect the POST to the RPC endpoint.
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/ebook\/overrides/,
      expectedStatus: 200,
    },
    async () => page.getByTestId("me-save").click(),
  );

  // Should navigate to the book detail page.
  await expect(page).toHaveURL(new RegExp(`/books/${id}$`));

  // The detail page should show the edited title.
  await expect(page.getByRole("heading", { level: 1, name: "Alpha Edited" })).toBeVisible();
});

// ---------------------------------------------------------------------------
// Revert to scanned values after the above test created an override
// ---------------------------------------------------------------------------

test("reverts overrides to scanned values", async ({ page, request }) => {
  // First fetch the book to confirm the override is still active.
  const id = await fetchBookIdByTitle(request, "Alpha Edited");
  await gotoReady(page, `/books/${id}/edit`);

  // The revert button should be visible since overrides exist.
  const revertBtn = page.getByTestId("revert-overrides");
  await expect(revertBtn).toBeVisible();

  // Click revert.
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/ebook\/overrides\/delete/,
      expectedStatus: 200,
    },
    async () => revertBtn.click(),
  );

  // Should navigate to the book detail page with the original title.
  await expect(page).toHaveURL(new RegExp(`/books/${id}$`));
  await expect(page.getByRole("heading", { level: 1, name: TARGET.title })).toBeVisible();
});

// ---------------------------------------------------------------------------
// Add/remove tags -> save
// ---------------------------------------------------------------------------

test("adds and removes tags via chip row", async ({ page, request }) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}/edit`);

  // Add a new tag via the inline input.
  const tagInput = page.getByPlaceholder("+ add tag…");
  await tagInput.fill("test-tag");
  await tagInput.press("Enter");

  // The new tag chip should be visible.
  await expect(page.getByText("test-tag")).toBeVisible();

  // Save bar should indicate a dirty field.
  await expect(page.getByTestId("me-save")).toBeEnabled();

  // Save.
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/ebook\/overrides/,
      expectedStatus: 200,
    },
    async () => page.getByTestId("me-save").click(),
  );

  await expect(page).toHaveURL(new RegExp(`/books/${id}$`));

  // Clean up: revert so other tests are not affected.
  await gotoReady(page, `/books/${id}/edit`);
  const revertBtn = page.getByTestId("revert-overrides");
  if (await revertBtn.isVisible()) {
    await revertBtn.click();
    await expect(page).toHaveURL(new RegExp(`/books/${id}$`));
  }
});

// ---------------------------------------------------------------------------
// Discard reverts unsaved changes
// ---------------------------------------------------------------------------

test("discard navigates back without saving", async ({ page, request }) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}/edit`);

  // Edit the title.
  const titleInput = page.getByLabel("Title");
  await titleInput.clear();
  await titleInput.fill("Should Not Be Saved");

  // Click discard — should navigate back to the detail page.
  await page.getByTestId("me-discard").click();
  await expect(page).toHaveURL(new RegExp(`/books/${id}$`));

  // The detail page should show the original title, not the unsaved one.
  await expect(page.getByRole("heading", { level: 1, name: TARGET.title })).toBeVisible();
});

// ---------------------------------------------------------------------------
// Edit button on book detail page
// ---------------------------------------------------------------------------

test("book detail page has a working edit metadata link", async ({ page, request }) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}`);

  // The edit button should be visible.
  const editLink = page.getByTestId("edit-metadata");
  await expect(editLink).toBeVisible();

  // Clicking it should navigate to the edit page.
  await editLink.click();
  await expect(page).toHaveURL(new RegExp(`/books/${id}/edit$`));
  await expect(page.getByTestId("me-page-title-label")).toBeVisible();
});

// ---------------------------------------------------------------------------
// Error paths — force 500 on the save / delete RPC and assert the UI does
// not navigate away from the edit form. The metadata edit page renders the
// `save_error` as visible text near the save bar (no `role=status` /
// `role=alert` today — see PR body for the missing-error-UX finding), so
// the most concrete observable state is that the URL still matches
// `/books/:id/edit` and the save button has come out of its "Saving…"
// state (re-enabled, label restored).
// ---------------------------------------------------------------------------

test("surfaces error and stays on edit form when save mutation fails", async ({ page, request }) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}/edit`);

  // Dirty a field so Save is enabled and a POST will fire.
  const titleInput = page.getByLabel("Title");
  await titleInput.clear();
  await titleInput.fill("Alpha Should Not Persist");
  await expect(page.getByTestId("me-save")).toBeEnabled();

  // Force-fail the override-save RPC. Match the trailing path explicitly so
  // we don't accidentally intercept the `/delete` sibling.
  await page.route("**/api/rpc/ebook/overrides", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({ status: 500, contentType: "text/plain", body: "forced failure" });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/ebook\/overrides$/,
      expectedStatus: 500,
    },
    async () => page.getByTestId("me-save").click(),
  );

  // URL must not have navigated away from the edit form.
  await expect(page).toHaveURL(new RegExp(`/books/${id}/edit$`));

  // Edited title is still in the input (state preserved).
  await expect(titleInput).toHaveValue("Alpha Should Not Persist");

  // Save button has come out of its "Saving…" state and is re-enabled so
  // the user can retry. `toBeEnabled` auto-retries until the signal settles.
  await expect(page.getByTestId("me-save")).toBeEnabled();
});

// ---------------------------------------------------------------------------
// ChipEditor autocomplete (F5.9-lite PR 3)
//
// The author and tag chip inputs now consult `data::list_authors` /
// `data::get_tag_cloud` on mount and surface up to 5 case-insensitive
// substring matches in a dropdown anchored under the input. ↓ + Enter
// commits the highlighted suggestion. Typing a string with no match
// suppresses the dropdown but Enter still commits the raw text.
// ---------------------------------------------------------------------------

test("surfaces existing authors as chip-editor suggestions and adds via ↓+Enter", async ({
  page,
  request,
}) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}/edit`);

  const authorInput = page.getByTestId("me-authors-input");
  await expect(authorInput).toBeVisible();

  // "Niklaus Wirth" is one of the Code Quartet fixture authors — typing a
  // prefix should surface it.
  await authorInput.fill("Nik");

  const dropdown = page.getByTestId("me-authors-suggestions");
  await expect(dropdown).toBeVisible();
  await expect(dropdown.getByRole("option", { name: /Niklaus Wirth/ })).toBeVisible();
  // Each suggestion row now carries a book-count badge. Confirm the
  // count text renders alongside the name — Niklaus Wirth shows up
  // in 4 Code Quartet fixtures (and possibly elsewhere). Just assert
  // the suffix shape ("N book" or "N books") is present on the row.
  await expect(dropdown.getByRole("option", { name: /Niklaus Wirth.*books?/ })).toBeVisible();
  // Five-row suggestion cap (`MAX_SUGGESTIONS=5`) PLUS the optional
  // "+ Create '<query>'" footer row → at most 6 options when the
  // query isn't an exact match.
  await expect
    .poll(async () => dropdown.getByRole("option").count(), {
      message: "ChipEditor dropdown options must cap at MAX_SUGGESTIONS=5 + 1 create row",
    })
    .toBeLessThanOrEqual(6);

  await authorInput.press("ArrowDown");
  await authorInput.press("Enter");

  // Chip rendered. Use the chip's me-avatar host (the chip container) so we
  // don't accidentally match the original "Ada Lovelace" chip text.
  await expect(page.getByText("Niklaus Wirth", { exact: true })).toBeVisible();
  // Input cleared; dropdown gone.
  await expect(authorInput).toHaveValue("");
  await expect(dropdown).toHaveCount(0);

  // Discard so the test doesn't leak an override into subsequent runs.
  await page.getByTestId("me-discard").click();
});

test("chip-editor surfaces the +Create row when no suggestion matches and Enter commits the typed value", async ({
  page,
  request,
}) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}/edit`);

  const authorInput = page.getByTestId("me-authors-input");
  await authorInput.fill("ZzzMatchesNothing");

  // No filtered matches, but the "+ Create '<query>'" footer renders so
  // the user can commit a brand-new value without dismissing the dropdown.
  const dropdown = page.getByTestId("me-authors-suggestions");
  await expect(dropdown).toBeVisible();
  await expect(dropdown.getByRole("option", { name: /\+ Create.*ZzzMatchesNothing/ })).toBeVisible();
  await expect(dropdown.getByRole("option")).toHaveCount(1);

  // Enter commits the typed value as a new chip (free-text fallback).
  await authorInput.press("Enter");
  await expect(page.getByText("ZzzMatchesNothing", { exact: true })).toBeVisible();
  await expect(authorInput).toHaveValue("");

  // Discard the unsaved chip so we don't drift fixture state.
  await page.getByTestId("me-discard").click();
});

// ---------------------------------------------------------------------------
// Series field autocomplete (#990)
//
// Unlike the author/tag chip editors, Series is a single-value field
// (`SuggestField`) — no chip list, but the same substring-match dropdown:
// typing surfaces matches, picking one overwrites the field with the exact
// existing name, and typed text with no match is still accepted as free
// text (creates a new series on save).
// ---------------------------------------------------------------------------

test("surfaces existing series as suggestions and fills the field on selection", async ({
  page,
  request,
}) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}/edit`);

  // Alpha is a standalone fixture (no series), so the field starts empty.
  const seriesInput = page.getByLabel("Series");
  await expect(seriesInput).toBeVisible();
  await expect(seriesInput).toHaveValue("");

  // "Code Quartet" is the fixture series backing the four Code Quartet
  // books (Niklaus Wirth et al.) — typing a prefix should surface it, same
  // interaction shape as the Author(s) field's dropdown.
  await seriesInput.fill("Cod");

  const dropdown = page.getByTestId("me-series-suggestions");
  await expect(dropdown).toBeVisible();
  await expect(dropdown.getByRole("option", { name: /Code Quartet/ })).toBeVisible();

  await seriesInput.press("ArrowDown");
  await seriesInput.press("Enter");

  // Selecting the suggestion fills the field with the exact existing name
  // and the dropdown closes.
  await expect(seriesInput).toHaveValue("Code Quartet");
  await expect(dropdown).toHaveCount(0);

  // Save bar reflects the dirty Series field.
  await expect(page.getByTestId("me-save")).toBeEnabled();

  // Discard so the test doesn't leak an override into subsequent runs.
  await page.getByTestId("me-discard").click();
  await expect(page).toHaveURL(new RegExp(`/books/${id}$`));
});

test("accepts a series name with no matching suggestion as free text", async ({
  page,
  request,
}) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}/edit`);

  const seriesInput = page.getByLabel("Series");
  await seriesInput.fill("ZzzBrandNewSeries");

  // No existing series matches, so no dropdown surfaces — free text is
  // still accepted directly in the field (no separate "create" row, unlike
  // the multi-chip author/tag editors, since there's no committed list to
  // add to).
  await expect(page.getByTestId("me-series-suggestions")).toHaveCount(0);
  await expect(seriesInput).toHaveValue("ZzzBrandNewSeries");

  await page.getByTestId("me-discard").click();
});

test("surfaces error and stays on edit form when revert mutation fails", async ({ page, request }) => {
  // First, create an override on the fixture so the revert button shows up.
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}/edit`);

  const titleInput = page.getByLabel("Title");
  await titleInput.clear();
  await titleInput.fill("Alpha Revert Setup");
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/ebook\/overrides$/,
      expectedStatus: 200,
    },
    async () => page.getByTestId("me-save").click(),
  );
  await expect(page).toHaveURL(new RegExp(`/books/${id}$`));

  // Re-open the edit form — the revert button should now be visible.
  const editId = await fetchBookIdByTitle(request, "Alpha Revert Setup");
  await gotoReady(page, `/books/${editId}/edit`);
  const revertBtn = page.getByTestId("revert-overrides");
  await expect(revertBtn).toBeVisible();

  // Force-fail the delete RPC.
  await page.route("**/api/rpc/ebook/overrides/delete", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({ status: 500, contentType: "text/plain", body: "forced failure" });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/ebook\/overrides\/delete$/,
      expectedStatus: 500,
    },
    async () => revertBtn.click(),
  );

  // URL must not have navigated away from the edit form.
  await expect(page).toHaveURL(new RegExp(`/books/${editId}/edit$`));

  // Revert button is still visible (override still active) and re-enabled.
  await expect(revertBtn).toBeVisible();
  await expect(revertBtn).toBeEnabled();

  // Clean up: stop intercepting and revert successfully so subsequent runs
  // start from a clean fixture state. `page.unroute` removes our handler.
  await page.unroute("**/api/rpc/ebook/overrides/delete");
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/ebook\/overrides\/delete$/,
      expectedStatus: 200,
    },
    async () => revertBtn.click(),
  );
  await expect(page).toHaveURL(new RegExp(`/books/${editId}$`));

  // Verify the override was actually removed, not just that the delete
  // returned 200 — the detail page should now show the original scanned
  // title, and re-opening /edit should hide the revert button.
  await expect(page.getByRole("heading", { level: 1, name: TARGET.title })).toBeVisible();
  await gotoReady(page, `/books/${editId}/edit`);
  await expect(page.getByTestId("revert-overrides")).toHaveCount(0);
});

// ---------------------------------------------------------------------------
// Cover upload / revert (sidebar CoverEditor)
// ---------------------------------------------------------------------------

// Seeding the override via a direct multipart POST (rather than driving the
// sidebar's `<input type=file>`) mirrors `author_photo.spec.ts`'s proven
// pattern for this same "web FormData/Blob upload" mechanism, so the test
// exercises the real REST contract without depending on a Playwright
// synthetic file-change event reaching the Dioxus web upload path.
async function uploadCover(request: import("@playwright/test").APIRequestContext, id: string) {
  const resp = await request.post(`/api/ebooks/${id}/cover`, {
    multipart: { cover: { name: "cover.png", mimeType: "image/png", buffer: TINY_PNG } },
  });
  expect(resp.status(), "POST cover should succeed").toBe(200);
}

test("shows an uploaded cover override and reverts it", async ({ page, request }) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}/edit`);
  await expect(page.getByTestId("cover-hint")).toHaveText("extracted from file");
  await expect(page.getByTestId("cover-remove-override")).toHaveCount(0);

  await uploadCover(request, id);
  await gotoReady(page, `/books/${id}/edit`);

  await expect(page.getByTestId("cover-hint")).toHaveText("custom upload");
  const revertBtn = page.getByTestId("cover-remove-override");
  await expect(revertBtn).toBeVisible();

  // Revert via the real UI control so subsequent tests see the original state.
  await expectMutation(
    page,
    { method: "DELETE", url: new RegExp(`/api/ebooks/${id}/cover$`), expectedStatus: 200 },
    async () => revertBtn.click(),
  );
  await expect(page.getByTestId("cover-hint")).toHaveText("extracted from file");
  await expect(page.getByTestId("cover-remove-override")).toHaveCount(0);
});

test("surfaces an error and keeps the override active when revert fails", async ({ page, request }) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await uploadCover(request, id);
  await gotoReady(page, `/books/${id}/edit`);

  const revertBtn = page.getByTestId("cover-remove-override");
  await expect(revertBtn).toBeVisible();

  await page.route(`**/api/ebooks/${id}/cover`, (route) => {
    if (route.request().method() === "DELETE") {
      return route.fulfill({ status: 500, contentType: "text/plain", body: "forced failure" });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    { method: "DELETE", url: new RegExp(`/api/ebooks/${id}/cover$`), expectedStatus: 500 },
    async () => revertBtn.click(),
  );

  // The override is still active (the delete failed) — status line surfaces
  // the failure and the revert control stays visible and re-enabled so the
  // user can retry.
  await expect(page.getByTestId("cover-upload-status")).toContainText("Revert failed");
  await expect(page.getByTestId("cover-hint")).toHaveText("custom upload");
  await expect(revertBtn).toBeVisible();
  await expect(revertBtn).toBeEnabled();

  // Clean up: stop intercepting and revert successfully.
  await page.unroute(`**/api/ebooks/${id}/cover`);
  await expectMutation(
    page,
    { method: "DELETE", url: new RegExp(`/api/ebooks/${id}/cover$`), expectedStatus: 200 },
    async () => revertBtn.click(),
  );
  await expect(page.getByTestId("cover-hint")).toHaveText("extracted from file");
});

}); // test.describe.serial
