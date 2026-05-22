import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// ── Layout & interaction (no seeded data needed) ──────────────────────

test("renders search trigger button in nav", async ({ page }) => {
  await gotoReady(page, "/");
  await expectNavVisible(page);

  const trigger = page.getByTestId("search-trigger");
  await expect(trigger).toBeVisible();
  await expect(trigger).toHaveText(/Search/);
});

test("palette opens on clicking search button", async ({ page }) => {
  await gotoReady(page, "/");
  await page.getByTestId("search-trigger").click();

  const panel = page.getByTestId("sp-panel");
  await expect(panel).toBeVisible();
});

test("palette opens on Cmd+K", async ({ page }) => {
  await gotoReady(page, "/");
  await page.keyboard.press("Meta+k");

  const panel = page.getByTestId("sp-panel");
  await expect(panel).toBeVisible();
});

test("palette closes on Escape", async ({ page }) => {
  await gotoReady(page, "/");
  await page.getByTestId("search-trigger").click();
  await expect(page.getByTestId("sp-panel")).toBeVisible();

  await page.keyboard.press("Escape");
  await expect(page.getByTestId("sp-panel")).toHaveCount(0);
});

test("palette closes on scrim click", async ({ page }) => {
  await gotoReady(page, "/");
  await page.getByTestId("search-trigger").click();
  await expect(page.getByTestId("sp-panel")).toBeVisible();

  // Click the scrim (outside the panel). The scrim fills the viewport.
  await page.getByTestId("sp-scrim").click({ position: { x: 10, y: 10 } });
  await expect(page.getByTestId("sp-panel")).toHaveCount(0);
});

// ── Autofocus ─────────────────────────────────────────────────────────

test("input is focused after clicking trigger so typing works immediately", async ({
  page,
}) => {
  await gotoReady(page, "/");
  await page.getByTestId("search-trigger").click();
  await expect(page.getByTestId("sp-panel")).toBeVisible();

  // Type on the keyboard without clicking the input first.
  await page.keyboard.type("dracula");

  // The characters should have gone into the search input.
  await expect(page.getByTestId("sp-input")).toHaveValue("dracula");
});

test("input is focused after Cmd+K so typing works immediately", async ({
  page,
}) => {
  await gotoReady(page, "/");
  await page.keyboard.press("Meta+k");
  await expect(page.getByTestId("sp-panel")).toBeVisible();

  // Type on the keyboard without clicking the input first.
  await page.keyboard.type("hello");

  // The characters should have gone into the search input.
  await expect(page.getByTestId("sp-input")).toHaveValue("hello");
});

// ── Seeded-data tests (books, authors, series, tags) ──────────────────

test.describe("with seeded library", () => {
  test.beforeAll(async ({ request }) => {
    await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
  });

  test("palette shows book results", async ({ page }) => {
    await gotoReady(page, "/");
    await page.getByTestId("search-trigger").click();
    const input = page.getByTestId("sp-input");
    await input.fill("dracula");

    await expect
      .poll(async () => page.getByTestId("sp-book-row").count())
      .toBeGreaterThanOrEqual(1);
  });

  test("palette shows author results", async ({ page }) => {
    await gotoReady(page, "/");
    await page.getByTestId("search-trigger").click();
    const input = page.getByTestId("sp-input");
    await input.fill("stoker");

    await expect
      .poll(async () => page.getByTestId("sp-author-row").count())
      .toBeGreaterThanOrEqual(1);
  });

  test("palette shows result count and timing", async ({ page }) => {
    await gotoReady(page, "/");
    await page.getByTestId("search-trigger").click();
    const input = page.getByTestId("sp-input");
    await input.fill("dracula");

    await expect
      .poll(async () => page.getByTestId("sp-result-count").count())
      .toBe(1);
    await expect(page.getByTestId("sp-result-count")).toContainText(/result/);
    await expect(page.getByTestId("sp-result-count")).toContainText(/ms/);
  });

  test("clicking book result navigates to detail", async ({ page }) => {
    await gotoReady(page, "/");
    await page.getByTestId("search-trigger").click();
    const input = page.getByTestId("sp-input");
    await input.fill("dracula");

    await expect
      .poll(async () => page.getByTestId("sp-book-row").count())
      .toBeGreaterThanOrEqual(1);

    await page.getByTestId("sp-book-row").first().click();

    // Should navigate to /books/:id
    await expect.poll(async () => new URL(page.url()).pathname).toMatch(
      /^\/books\/\d+$/,
    );
  });

  test("keyboard navigation highlights results", async ({ page }) => {
    await gotoReady(page, "/");
    await page.getByTestId("search-trigger").click();
    const input = page.getByTestId("sp-input");
    await input.fill("dracula");

    // Wait for results to appear.
    await expect
      .poll(async () => page.getByTestId("sp-book-row").count())
      .toBeGreaterThanOrEqual(1);

    // Arrow down should select the first book row.
    await page.keyboard.press("ArrowDown");
    await expect(page.getByTestId("sp-book-row").first()).toHaveClass(/selected/);
  });

  test("inside text shows coming soon", async ({ page }) => {
    await gotoReady(page, "/");
    await page.getByTestId("search-trigger").click();
    const input = page.getByTestId("sp-input");
    await input.fill("dracula");

    // Wait for results, then check the placeholder.
    await expect
      .poll(async () => page.getByTestId("sp-book-row").count())
      .toBeGreaterThanOrEqual(1);

    await expect(page.getByTestId("sp-coming-soon")).toBeVisible();
    await expect(page.getByTestId("sp-coming-soon")).toHaveText("Coming soon");
  });
});
