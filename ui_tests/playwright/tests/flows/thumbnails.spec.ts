import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
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

  // Pick a fixture book that has a cover and target its row directly by slug.
  // Earlier this filtered by `ebook-cell-cover`, but every row has that
  // testid (covered or not) so the filter was a no-op and the chosen row
  // depended on sort order.
  const bookWithCover = FIXTURE_BOOKS.find((b) => b.hasCover);
  expect(bookWithCover, "expected at least one fixture book with hasCover=true").toBeTruthy();

  const coverImg = page
    .getByTestId(`ebook-row-${bookWithCover!.slug}`)
    .getByRole("img", { name: /^Cover of/ });

  await expect(coverImg).toBeVisible();

  const srcset = await coverImg.getAttribute("srcset");
  expect(srcset, "srcset attribute must be present").not.toBeNull();
  expect(srcset).toMatch(/\/api\/thumbs\/\d+\/sm/);
  expect(srcset).toContain("160w");
  expect(srcset).toContain("320w");
  expect(srcset).toContain("640w");
});

test("thumb endpoint serves an image", async ({ page, request }) => {
  await gotoReady(page, "/");

  // Extract a real book ID from the srcset of the first cover <img> in the grid.
  const coverImg = page
    .getByTestId(/^ebook-row-/)
    .filter({ has: page.getByRole("img", { name: /^Cover of/ }) })
    .first()
    .getByRole("img", { name: /^Cover of/ });

  await expect(coverImg).toBeVisible();

  const srcset = await coverImg.getAttribute("srcset");
  expect(srcset).not.toBeNull();

  // Parse the book ID out of the srcset (e.g. "/api/thumbs/3/sm 160w, ...").
  const match = srcset!.match(/\/api\/thumbs\/(\d+)\/sm/);
  expect(match, "could not parse book id from srcset").not.toBeNull();
  const bookId = match![1];

  // On first request the endpoint may return the original cover (image/jpeg);
  // poll until the background WebP generation has finished (up to 10 s).
  await expect
    .poll(
      async () => {
        const resp = await request.get(`/api/thumbs/${bookId}/md`);
        if (resp.status() !== 200) return `status:${resp.status()}`;
        return resp.headers()["content-type"] ?? "missing";
      },
      {
        message: "expected /api/thumbs/{id}/md to return image/webp",
        timeout: 10_000,
        intervals: [200, 500, 1_000],
      },
    )
    .toContain("image/webp");
});

test("books without covers render fallback dash", async ({ page }) => {
  await gotoReady(page, "/");

  // "gamma" is the fixture book with hasCover=false.
  const bookWithoutCover = FIXTURE_BOOKS.find((b) => !b.hasCover);
  expect(bookWithoutCover, "expected at least one fixture book with hasCover=false").toBeTruthy();

  const row = page.getByTestId(`ebook-row-${bookWithoutCover!.slug}`);
  await expect(row).toBeVisible();

  const coverCell = row.getByTestId("ebook-cell-cover");
  // No <img> in the cover cell.
  await expect(coverCell.getByRole("img")).toHaveCount(0);
  // Fallback dash text is rendered.
  await expect(coverCell.getByText("—")).toBeVisible();
});
