import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectMutation } from "../utils/api";
import { fetchBookIdByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// Alpha fixture: standalone, single author, has cover.
const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "alpha")!;

// ---------------------------------------------------------------------------
// Layout test
// ---------------------------------------------------------------------------

test("renders the metadata edit form with pre-populated fields", async ({ page, request }) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}/edit`);

  // Page header renders with the book title and "Edit metadata" label.
  await expect(page.getByText("Edit metadata")).toBeVisible();
  await expect(page.getByText(TARGET.title)).toBeVisible();

  // Breadcrumb navigation is present with "Home" link.
  await expect(
    page.getByRole("navigation", { name: "breadcrumb" }).getByRole("link", { name: "Home" }),
  ).toBeVisible();

  // Title input is pre-populated with the fixture's title.
  const titleInput = page.getByLabel("Title");
  await expect(titleInput).toBeVisible();
  await expect(titleInput).toHaveValue(TARGET.title);

  // Author chip is visible.
  await expect(page.getByText(TARGET.authors[0])).toBeVisible();

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
  await expect(page.getByText("Edit metadata")).toBeVisible();
});
