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

// Regression for #360: the ⌘K listener used to be registered inside
// `SearchPaletteHost`, which re-mounts on every route change. After the
// first navigation each press toggled the open signal twice (old + new
// listener), so the hotkey appeared dead everywhere except the landing
// page. Drive an in-app SPA navigation (not a full reload — which would
// mask the bug by re-running App from scratch) into each of the other
// pages that ships the search trigger, then verify ⌘K still opens the
// palette.
for (const route of ["/authors", "/series"]) {
  test(`palette opens on Cmd+K after navigating to ${route}`, async ({
    page,
  }) => {
    await gotoReady(page, "/");
    // Click an in-app `<Link>` so the router transitions in-place.
    const linkName = route === "/authors" ? "Authors" : "Series";
    await page
      .getByRole("navigation", { name: "Primary" })
      .getByRole("link", { name: linkName })
      .click();
    await expect.poll(async () => new URL(page.url()).pathname).toBe(route);

    await page.keyboard.press("Meta+k");
    await expect(page.getByTestId("sp-panel")).toBeVisible();
  });
}

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

    // Should navigate to /books/:uuid
    await expect.poll(async () => new URL(page.url()).pathname).toMatch(
      /^\/books\/[0-9a-fA-F-]{36}$/,
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

  test("clicking author result navigates to author detail page", async ({ page }) => {
    await gotoReady(page, "/");
    await page.getByTestId("search-trigger").click();
    await page.getByTestId("sp-input").fill("stoker");

    await expect
      .poll(async () => page.getByTestId("sp-author-row").count())
      .toBeGreaterThanOrEqual(1);

    await page.getByTestId("sp-author-row").first().click();
    await expect.poll(async () => new URL(page.url()).pathname).toMatch(
      /^\/authors\/\d+$/,
    );
  });

  test("clicking series result navigates to series detail page", async ({ page }) => {
    await gotoReady(page, "/");
    await page.getByTestId("search-trigger").click();
    await page.getByTestId("sp-input").fill("pioneers");

    await expect
      .poll(async () => page.getByTestId("sp-series-row").count())
      .toBeGreaterThanOrEqual(1);

    await page.getByTestId("sp-series-row").first().click();
    await expect.poll(async () => new URL(page.url()).pathname).toMatch(
      /^\/series\/\d+$/,
    );
  });

  test("pressing Enter without arrow-key navigation goes to /search", async ({ page }) => {
    await gotoReady(page, "/");
    await page.getByTestId("search-trigger").click();
    const input = page.getByTestId("sp-input");
    await input.fill("dracula");

    // Wait for at least one result so the palette has loaded, then press
    // Enter without engaging arrow keys — should route to the full-page
    // results, not drill into the first result.
    await expect
      .poll(async () => page.getByTestId("sp-book-row").count())
      .toBeGreaterThanOrEqual(1);

    await input.press("Enter");
    await expect.poll(async () => new URL(page.url()).pathname).toMatch(
      /^\/search\/dracula$/,
    );
    await expect(page.getByTestId("search-back")).toBeVisible();
  });

  test("Enter after arrow-key selection drills into the highlighted result", async ({ page }) => {
    await gotoReady(page, "/");
    await page.getByTestId("search-trigger").click();
    const input = page.getByTestId("sp-input");
    await input.fill("dracula");

    await expect
      .poll(async () => page.getByTestId("sp-book-row").count())
      .toBeGreaterThanOrEqual(1);

    // Arrow down to engage selection, then Enter to drill in.
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");

    await expect.poll(async () => new URL(page.url()).pathname).toMatch(
      /^\/books\/[0-9a-fA-F-]{36}$/,
    );
  });
});
