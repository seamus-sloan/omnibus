import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { switchToTableView } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Seed once before all thumbnail tests so the running server is indexed
// against the committed EPUB fixtures, independent of whatever ran earlier
// in this worker.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

test("renders book grid with srcset cover images", async ({ page }) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  // Pick a fixture book that has a cover and target its row directly by slug.
  // Earlier this filtered by `ebook-cell-cover`, but every row has that
  // testid (covered or not) so the filter was a no-op and the chosen row
  // depended on sort order.
  const bookWithCover = FIXTURE_BOOKS.find((b) => b.hasCover);
  expect(
    bookWithCover,
    "expected at least one fixture book with hasCover=true",
  ).toBeTruthy();

  const coverImg = page
    .getByTestId(`ebook-row-${bookWithCover!.slug}`)
    .getByRole("img", { name: /^Cover of/ });

  await expect(coverImg).toBeVisible();

  const srcset = await coverImg.getAttribute("srcset");
  expect(srcset, "srcset attribute must be present").not.toBeNull();
  // Thumb URLs are uuid-keyed (UUIDv5, 8-4-4-4-12 hyphenated form) —
  // see `/api/thumbs/:uuid/:size` in the router.
  expect(srcset).toMatch(/\/api\/thumbs\/[0-9a-fA-F-]{36}\/sm/);
  expect(srcset).toContain("160w");
  expect(srcset).toContain("320w");
  expect(srcset).toContain("640w");
});

test("thumb endpoint serves an image", async ({ page, request }) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  // Extract a real book ID from the srcset of the first cover <img> in the grid.
  const coverImg = page
    .getByTestId(/^ebook-row-/)
    .filter({ has: page.getByRole("img", { name: /^Cover of/ }) })
    .first()
    .getByRole("img", { name: /^Cover of/ });

  await expect(coverImg).toBeVisible();

  const srcset = await coverImg.getAttribute("srcset");
  expect(srcset).not.toBeNull();

  // Parse the book UUID out of the srcset
  // (e.g. "/api/thumbs/ad8d591f-546b-59d0-bfff-0a4de6fc7e55/sm 160w, ...").
  // The route is uuid-keyed (not id-keyed) for the same URL-stability
  // reason `/books/:uuid` uses — see `db::queries::stable_uuid`.
  const match = srcset!.match(/\/api\/thumbs\/([0-9a-fA-F-]{36})\/sm/);
  expect(match, "could not parse book uuid from srcset").not.toBeNull();
  const bookUuid = match![1];

  // On first request the endpoint may return the original cover (image/jpeg);
  // poll until the background WebP generation has finished. The CI worker
  // competes with index/scan jobs on a cold runner, so 10 s was empirically
  // too tight; allow up to 30 s.
  await expect
    .poll(
      async () => {
        const resp = await request.get(`/api/thumbs/${bookUuid}/md`);
        if (resp.status() !== 200) return `status:${resp.status()}`;
        return resp.headers()["content-type"] ?? "missing";
      },
      {
        message: "expected /api/thumbs/{uuid}/md to return image/webp",
        timeout: 30_000,
        intervals: [200, 500, 1_000, 2_000, 3_000],
      },
    )
    .toContain("image/webp");
});

test("books without covers render fallback dash", async ({ page }) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  // "gamma" is the fixture book with hasCover=false.
  const bookWithoutCover = FIXTURE_BOOKS.find((b) => !b.hasCover);
  expect(
    bookWithoutCover,
    "expected at least one fixture book with hasCover=false",
  ).toBeTruthy();

  const row = page.getByTestId(`ebook-row-${bookWithoutCover!.slug}`);
  await expect(row).toBeVisible();

  const coverCell = row.getByTestId("ebook-cell-cover");
  // No <img> in the cover cell.
  await expect(coverCell.getByRole("img")).toHaveCount(0);
  // Fallback dash text is rendered.
  await expect(coverCell.getByText("—")).toBeVisible();
});
