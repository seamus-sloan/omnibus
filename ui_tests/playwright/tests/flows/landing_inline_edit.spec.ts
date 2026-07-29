import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookIdByTitle, switchToTableView } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// F5.9-lite PR 4 — admin inline-edit cells on the landing-page table.
//
// The seeded test user is admin via the first-user-admin promotion, so
// every test in this file exercises the admin path. The shared landing
// spec already covers the read-only render for the no-admin / no-edit
// permission case; the only thing those users gain from this PR is the
// absence of any inline-edit affordance, which we assert by checking
// that the editable class isn't present on non-admin renders.
//
// All tests revert their override at the end so subsequent runs start
// from a clean fixture state.

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// Pick a fixture with a clean prefix-cruft simulation. Alpha is the
// standard "small fixture" target used by metadata_edit.spec.ts.
const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "alpha")!;

// The Tags-cell tests get their own reserved fixture: the suite is
// `fullyParallel` (even within one file), and the Authors tests' cleanup
// deletes ALL of TARGET's overrides — which would wipe an in-flight tags
// override mid-test. standalone-ocean is read by no other spec.
const TAGS_TARGET = FIXTURE_BOOKS.find((b) => b.slug === "standalone-ocean")!;

test("renders editable cell affordances on the landing table for admins", async ({
  page,
}) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  const titleCell = page
    .getByTestId(`ebook-row-${TARGET.slug}`)
    .getByTestId("ebook-cell-title");
  await expect(titleCell).toBeVisible();
  // The `ebook-cell-editable` class is added only when the row's
  // `is_admin` prop is true — its presence is the user-visible signal
  // that inline editing is available.
  await expect(titleCell).toHaveClass(/ebook-cell-editable/);
});

test("edits a title inline and saves the override via rpc_save_overrides", async ({
  page,
  request,
}) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  const row = page.getByTestId(`ebook-row-${TARGET.slug}`);
  const titleCell = row.getByTestId("ebook-cell-title");
  // Click on the cell — must NOT navigate to the detail page (the row
  // click does); cell `stopPropagation` keeps us on /.
  await titleCell.click();

  const input = row.getByTestId("ebook-cell-title-input");
  await expect(input).toBeVisible();
  await input.fill("Alpha Inline Edited");

  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/ebook\/overrides$/,
      expectedStatus: 200,
    },
    async () => input.press("Enter"),
  );

  // URL stayed on the landing page (no navigation triggered).
  await expect(page).toHaveURL(/\/$/);
  // The cell reflects the optimistic update from the server-merged
  // metadata returned by `rpc_save_overrides`.
  await expect(titleCell).toContainText("Alpha Inline Edited");

  // Cleanup: revert via the F5.1 RPC so subsequent tests start from
  // pristine fixture state. Assert the delete succeeds rather than
  // swallowing errors — a silent failure would leak the override
  // across the suite and turn this into an order-dependent flake.
  const uuid = await fetchBookIdByTitle(request, "Alpha Inline Edited");
  const revertResp = await request.post(`/api/rpc/ebook/overrides/delete`, {
    data: { uuid },
  });
  expect(revertResp.status(), "cleanup revert must succeed").toBe(200);
});

test("inline edit save error keeps the row showing the prior value", async ({
  page,
}) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  const row = page.getByTestId(`ebook-row-${TARGET.slug}`);
  const titleCell = row.getByTestId("ebook-cell-title");
  const originalText = (await titleCell.innerText()).trim();

  await titleCell.click();
  const input = row.getByTestId("ebook-cell-title-input");
  await input.fill("Should Not Persist");

  // Force the save mutation to fail.
  await page.route("**/api/rpc/ebook/overrides", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 500,
        contentType: "text/plain",
        body: "forced failure",
      });
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
    async () => input.press("Enter"),
  );

  // Optimistic update is intentionally light — the row reverts to its
  // prior server-merged value on the next refetch. Since the failure
  // path here doesn't trigger a refetch, the cell falls back to
  // whatever `book_state` was before the save attempt: the original.
  await page.unroute("**/api/rpc/ebook/overrides");
  await expect(titleCell).toContainText(originalText);
});

test("clicking the Authors cell renders the chip editor inline", async ({
  page,
}) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  const row = page.getByTestId(`ebook-row-${TARGET.slug}`);
  const authorsCell = row.getByTestId("ebook-cell-author");
  await authorsCell.click();

  // The chip-editor input is rendered *inside* the cell (no sub-row).
  // testid-prefixed `ebook-cell-author` per the AuthorsCell component.
  const chipInput = authorsCell.getByTestId("ebook-cell-author-input");
  await expect(chipInput).toBeVisible();

  // Existing author chips render inside the same cell.
  for (const name of TARGET.authors) {
    await expect(authorsCell.getByText(name, { exact: true })).toBeVisible();
  }

  // The autocomplete dropdown auto-opens on focus, showing the
  // library-wide author suggestion pool with an ADD AUTHOR header.
  const dropdown = authorsCell.getByTestId("ebook-cell-author-suggestions");
  await expect(dropdown).toBeVisible();
  await expect(dropdown.getByText("ADD AUTHOR")).toBeVisible();

  // Escape exits edit mode.
  await chipInput.press("Escape");
  await expect(chipInput).toHaveCount(0);
});

// #992 — the Authors cell's ChipEditor only closed on Escape; blurring away
// (clicking elsewhere) left the amber `ebook-cell-editing` highlight stuck.

test("clicking away from an open Authors cell editor closes it", async ({
  page,
}) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  const row = page.getByTestId(`ebook-row-${TARGET.slug}`);
  const authorsCell = row.getByTestId("ebook-cell-author");
  await authorsCell.click();

  const chipInput = authorsCell.getByTestId("ebook-cell-author-input");
  await expect(chipInput).toBeVisible();
  await expect(authorsCell).toHaveClass(/ebook-cell-editing/);

  // Click a neutral, always-present element well outside the row/table —
  // a genuine click-away, not a suggestion pick.
  await page.getByRole("heading", { level: 1, name: "Your Library" }).click();

  await expect(chipInput).toHaveCount(0);
  await expect(authorsCell).not.toHaveClass(/ebook-cell-editing/);
});

test("Tab from the chip input reaches a chip's Remove button without closing the editor", async ({
  page,
}) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  const row = page.getByTestId(`ebook-row-${TARGET.slug}`);
  const authorsCell = row.getByTestId("ebook-cell-author");
  await authorsCell.click();

  const chipInput = authorsCell.getByTestId("ebook-cell-author-input");
  await expect(chipInput).toBeVisible();

  // ChipList (chips + Remove buttons) renders before the input in DOM
  // order, so Shift+Tab moves focus backward onto the preceding chip's
  // Remove button — the exact keyboard path the #992 follow-up fix
  // protects (blur must not close the editor when the newly-focused
  // element is still inside it).
  await chipInput.press("Shift+Tab");

  const removeButton = authorsCell.getByRole("button", {
    name: `Remove ${TARGET.authors[0]}`,
  });
  await expect(removeButton).toBeFocused();
  await expect(chipInput).toBeVisible();
  await expect(authorsCell).toHaveClass(/ebook-cell-editing/);
});

test("selecting an author suggestion keeps the editor open; a later click away closes it", async ({
  page,
  request,
}) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  const row = page.getByTestId(`ebook-row-${TARGET.slug}`);
  const authorsCell = row.getByTestId("ebook-cell-author");
  await authorsCell.click();

  const chipInput = authorsCell.getByTestId("ebook-cell-author-input");
  await chipInput.fill("Grace");

  const dropdown = authorsCell.getByTestId("ebook-cell-author-suggestions");
  const suggestion = dropdown.getByRole("option", { name: /Grace Hopper/ });
  await expect(suggestion).toBeVisible();

  // AC1: picking a suggestion commits the chip via `onmousedown` +
  // `preventDefault` (so the input never blurs) and must NOT close the
  // editor — the admin can keep adding authors.
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/ebook\/overrides$/,
      expectedStatus: 200,
    },
    async () => suggestion.click(),
  );
  await expect(
    authorsCell.getByText("Grace Hopper", { exact: true }),
  ).toBeVisible();
  await expect(chipInput).toBeVisible();
  await expect(authorsCell).toHaveClass(/ebook-cell-editing/);

  // AC2 (regression, chained): a genuine click-away after the pick still
  // closes the editor and clears the highlight.
  await page.getByRole("heading", { level: 1, name: "Your Library" }).click();
  await expect(chipInput).toHaveCount(0);
  await expect(authorsCell).not.toHaveClass(/ebook-cell-editing/);

  // Cleanup: revert the accumulated authors override.
  const uuid = await fetchBookIdByTitle(request, TARGET.title);
  const revertResp = await request.post(`/api/rpc/ebook/overrides/delete`, {
    data: { uuid },
  });
  expect(revertResp.status(), "cleanup revert must succeed").toBe(200);
});

test("clicking the Tags cell renders the chip editor inline", async ({
  page,
}) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  const row = page.getByTestId(`ebook-row-${TAGS_TARGET.slug}`);
  const tagsCell = row.getByTestId("ebook-cell-tags");
  await tagsCell.click();

  // The chip-editor input is rendered *inside* the cell, mirroring the
  // Authors cell — testid-prefixed `ebook-cell-tags` per ChipCellDisplay.
  const chipInput = tagsCell.getByTestId("ebook-cell-tags-input");
  await expect(chipInput).toBeVisible();
  await expect(chipInput).toHaveAttribute("placeholder", "+ add tag…");
  await expect(tagsCell).toHaveClass(/ebook-cell-editing/);

  // Escape exits edit mode.
  await chipInput.press("Escape");
  await expect(chipInput).toHaveCount(0);
  await expect(tagsCell).not.toHaveClass(/ebook-cell-editing/);
});

test("creates a new tag inline, then autocompletes it from the pool after reload", async ({
  page,
  request,
}) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  const row = page.getByTestId(`ebook-row-${TAGS_TARGET.slug}`);
  const tagsCell = row.getByTestId("ebook-cell-tags");
  await tagsCell.click();

  const chipInput = tagsCell.getByTestId("ebook-cell-tags-input");
  await expect(chipInput).toBeVisible();

  // Create a brand-new tag. Nothing in the pool matches, so Enter commits
  // the raw typed text and posts the full `subjects` override.
  await chipInput.fill("Speculative Fiction");
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/ebook\/overrides$/,
      expectedStatus: 200,
    },
    async () => chipInput.press("Enter"),
  );
  await expect(
    tagsCell.getByText("Speculative Fiction", { exact: true }),
  ).toBeVisible();

  // Reload so the landing page refetches the tag suggestion pool — the
  // override save materialized a `tags` row, so the freshly created tag now
  // surfaces through `get_tag_cloud` (the autocomplete source).
  await page.reload();
  await page.waitForLoadState("networkidle");
  await switchToTableView(page);

  const rowAfter = page.getByTestId(`ebook-row-${TAGS_TARGET.slug}`);
  const tagsCellAfter = rowAfter.getByTestId("ebook-cell-tags");
  await tagsCellAfter.click();
  const chipInputAfter = tagsCellAfter.getByTestId("ebook-cell-tags-input");
  await expect(chipInputAfter).toBeVisible();

  // Remove the chip first — the dropdown excludes values already on the
  // chip list, so the tag must be off the book for its suggestion to show.
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/ebook\/overrides$/,
      expectedStatus: 200,
    },
    async () =>
      tagsCellAfter
        .getByRole("button", { name: "Remove tag Speculative Fiction" })
        .click(),
  );

  // Typing a prefix surfaces the pool entry under the ADD TAG header;
  // picking it commits the chip and posts the override.
  await chipInputAfter.fill("Specul");
  const dropdown = tagsCellAfter.getByTestId("ebook-cell-tags-suggestions");
  await expect(dropdown).toBeVisible();
  await expect(dropdown.getByText("ADD TAG")).toBeVisible();
  const suggestion = dropdown.getByRole("option", {
    name: /Speculative Fiction/,
  });
  await expect(suggestion).toBeVisible();
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/ebook\/overrides$/,
      expectedStatus: 200,
    },
    async () => suggestion.click(),
  );
  await expect(
    tagsCellAfter.getByText("Speculative Fiction", { exact: true }),
  ).toBeVisible();

  // Cleanup: revert the accumulated tags override.
  const uuid = await fetchBookIdByTitle(request, TAGS_TARGET.title);
  const revertResp = await request.post(`/api/rpc/ebook/overrides/delete`, {
    data: { uuid },
  });
  expect(revertResp.status(), "cleanup revert must succeed").toBe(200);
});
