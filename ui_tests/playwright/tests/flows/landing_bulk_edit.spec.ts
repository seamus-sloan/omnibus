import type { Page } from "@playwright/test";

import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookIdByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Table-view bulk edit: row checkboxes → floating action bar → modal →
// one `rpc_bulk_save_overrides` call for every selected book.
//
// The two `bulk-target-*` fixtures are reserved for this spec (see the
// comment in fixtures/epubs.ts) — the happy-path test bulk-writes overrides
// to BOTH and reverts them at the end, and the suite is fullyParallel, so
// nothing else may read them.

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

const TARGETS = FIXTURE_BOOKS.filter((b) => b.slug.startsWith("bulk-target-"));
const [PRIMARY, SECONDARY] = [TARGETS[0]!, TARGETS[1]!];

/** Check both target rows' selection checkboxes. */
async function selectTargets(page: Page) {
  for (const target of [PRIMARY, SECONDARY]) {
    await page
      .getByTestId(`ebook-row-${target.slug}`)
      .getByTestId("ebook-select")
      .check();
  }
}

test("renders the bulk edit layout: checkboxes, select-all, and no bar while nothing is selected", async ({
  page,
}) => {
  await gotoReady(page, "/");

  await expect(page.getByTestId("ebook-select-all")).toBeVisible();
  for (const target of [PRIMARY, SECONDARY]) {
    await expect(
      page.getByTestId(`ebook-row-${target.slug}`).getByTestId("ebook-select"),
    ).toBeVisible();
  }
  await expect(page.getByTestId("bulk-edit-bar")).toHaveCount(0);
});

test("selecting rows reveals the bulk edit bar and checkbox clicks do not navigate", async ({
  page,
}) => {
  await gotoReady(page, "/");

  await selectTargets(page);
  // The checkbox cell stops propagation — the row's click-to-navigate
  // handler must not fire.
  await expect(page).toHaveURL(/\/$/);

  const bar = page.getByTestId("bulk-edit-bar");
  await expect(bar).toContainText("2 books selected");

  await page.getByTestId("bulk-edit-clear").click();
  await expect(bar).toHaveCount(0);
});

test("select all toggles every visible row", async ({ page }) => {
  await gotoReady(page, "/");

  const selectAll = page.getByTestId("ebook-select-all");
  await selectAll.check();
  const rowCount = await page.getByTestId("ebook-select").count();
  await expect(page.getByTestId("bulk-edit-bar")).toContainText(
    `${rowCount} books selected`,
  );

  await selectAll.uncheck();
  await expect(page.getByTestId("bulk-edit-bar")).toHaveCount(0);
});

test("bulk edits publisher and tags across the selected books via rpc_bulk_save_overrides", async ({
  page,
  request,
}) => {
  await gotoReady(page, "/");

  await selectTargets(page);
  await page.getByTestId("bulk-edit-open").click();

  const modal = page.getByTestId("bulk-edit-modal");
  await expect(modal).toBeVisible();
  await expect(modal).toContainText("Edit 2 books");

  await modal.getByLabel("Publisher").fill("Bulk Edited Press");
  await modal.getByTestId("bulk-add-tags-input").fill("bulk-tagged");
  await modal.getByTestId("bulk-add-tags-input").press("Enter");

  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/ebook\/overrides\/bulk$/,
      expectedStatus: 200,
    },
    async () => page.getByTestId("bulk-edit-submit").click(),
  );

  // Modal and bar close; the rows re-render from the returned metadata.
  await expect(modal).toHaveCount(0);
  await expect(page.getByTestId("bulk-edit-bar")).toHaveCount(0);
  for (const target of [PRIMARY, SECONDARY]) {
    await expect(
      page
        .getByTestId(`ebook-row-${target.slug}`)
        .getByTestId("ebook-cell-publisher"),
    ).toContainText("Bulk Edited Press");
  }

  // Cleanup: revert both books' overrides so subsequent runs start from
  // pristine fixture state. Assert each delete succeeds — a silent failure
  // would leak the override across the suite.
  for (const target of [PRIMARY, SECONDARY]) {
    const uuid = await fetchBookIdByTitle(request, target.title);
    const revertResp = await request.post(`/api/rpc/ebook/overrides/delete`, {
      data: { uuid },
    });
    expect(
      revertResp.status(),
      `cleanup revert for ${target.slug} must succeed`,
    ).toBe(200);
  }
});

test("bulk edit save error keeps the modal open and the rows unchanged", async ({
  page,
}) => {
  await gotoReady(page, "/");

  await selectTargets(page);
  await page.getByTestId("bulk-edit-open").click();
  const modal = page.getByTestId("bulk-edit-modal");
  await modal.getByLabel("Publisher").fill("Should Not Persist");

  await page.route("**/api/rpc/ebook/overrides/bulk", (route) => {
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
      url: /\/api\/rpc\/ebook\/overrides\/bulk$/,
      expectedStatus: 500,
    },
    async () => page.getByTestId("bulk-edit-submit").click(),
  );
  await page.unroute("**/api/rpc/ebook/overrides/bulk");

  // The modal surfaces the failure and stays open; nothing was written.
  await expect(page.getByTestId("bulk-edit-error")).toBeVisible();
  await expect(page.getByTestId("bulk-edit-modal")).toBeVisible();
  for (const target of [PRIMARY, SECONDARY]) {
    await expect(
      page
        .getByTestId(`ebook-row-${target.slug}`)
        .getByTestId("ebook-cell-publisher"),
    ).toContainText(target.publisher!);
  }
});
