import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectMutation } from "../utils/api";
import { fetchBookIdByTitle } from "../utils/ebooks";
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

test("renders editable cell affordances on the landing table for admins", async ({ page }) => {
  await gotoReady(page, "/");

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

test("inline edit save error keeps the row showing the prior value", async ({ page }) => {
  await gotoReady(page, "/");

  const row = page.getByTestId(`ebook-row-${TARGET.slug}`);
  const titleCell = row.getByTestId("ebook-cell-title");
  const originalText = (await titleCell.innerText()).trim();

  await titleCell.click();
  const input = row.getByTestId("ebook-cell-title-input");
  await input.fill("Should Not Persist");

  // Force the save mutation to fail.
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
    async () => input.press("Enter"),
  );

  // Optimistic update is intentionally light — the row reverts to its
  // prior server-merged value on the next refetch. Since the failure
  // path here doesn't trigger a refetch, the cell falls back to
  // whatever `book_state` was before the save attempt: the original.
  await page.unroute("**/api/rpc/ebook/overrides");
  await expect(titleCell).toContainText(originalText);
});

test("clicking the Authors cell renders the chip editor inline", async ({ page }) => {
  await gotoReady(page, "/");

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
