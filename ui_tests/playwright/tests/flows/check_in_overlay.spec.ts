import type { Page } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// The check-in flow is presented as a centered overlay over a blurred page.
// The top-nav trigger renders at desktop width, so this runs at the default
// (desktop) viewport. The full-page `/check-in` deep-link fallback is covered
// by `check_in_lookup.spec.ts`, the camera by `check_in_scan.spec.ts`.
//
// The dismiss-on-navigate tests below mock every scan RPC, the check-in write
// included, for the same reason `check_in_confirm.spec.ts` does: they run
// against `standalone-island`, which `physical_collection.spec.ts` reads, so a
// real copy filed here would leak into a spec that never expects one. What
// they assert is the overlay's own state on the destination page, which the
// mocks don't touch.

const ISBN = "9780441013593";
const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "standalone-island")!;

function scanBook(
  uuid: string,
  over: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    uuid,
    title: TARGET.title,
    authors: TARGET.authors,
    cover_url: null,
    has_physical: false,
    isbn: ISBN,
    ...over,
  };
}

async function mockJsonPost(
  page: Page,
  url: string | RegExp,
  body: unknown,
): Promise<void> {
  await page.route(url, (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(body),
        })
      : route.continue(),
  );
}

/// Raise the overlay from the top nav and wait for it to be up.
async function openOverlay(page: Page): Promise<void> {
  await gotoReady(page, "/");
  await page.getByTestId("check-in-button").click();
  await expect(page.getByTestId("check-in-overlay-scrim")).toBeVisible();
}

/// Type `ISBN` into the open overlay and submit it, awaiting the resolve.
async function submitIsbn(page: Page): Promise<void> {
  await page.getByTestId("check-in-isbn").fill(ISBN);
  await expectMutation(
    page,
    { method: "POST", url: /\/api\/rpc\/scan\/resolve$/, expectedStatus: 200 },
    async () => page.getByTestId("check-in-submit").click(),
  );
}

/// The overlay is gone and left nothing of itself behind on the new page.
async function expectOverlayDismissed(page: Page): Promise<void> {
  await expect(page.getByTestId("check-in-overlay-scrim")).toHaveCount(0);
  await expect(page.getByTestId("check-in-overlay-panel")).toHaveCount(0);
  await expect(page.getByTestId("check-in")).toHaveCount(0);
}

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

test.describe("check-in overlay", () => {
  test("the top-nav Check in button opens the overlay without navigating", async ({
    page,
  }) => {
    await gotoReady(page, "/");
    await expectNavVisible(page);
    await expect(page.getByTestId("check-in-overlay-scrim")).toHaveCount(0);

    await page.getByTestId("check-in-button").click();

    // Floats in place over the current page — no route change.
    await expect(page.getByTestId("check-in-overlay-scrim")).toBeVisible();
    await expect(page.getByTestId("check-in")).toBeVisible();
    // Opens on the fields, not the camera — raising the overlay from the top
    // nav must never start a webcam.
    await expect(
      page.getByRole("heading", { name: "Check in a book" }),
    ).toBeVisible();
    await expect(page.getByTestId("barcode-scanner")).toHaveCount(0);
    await expect(page).toHaveURL(/\/$/);

    // The overlay shows exactly one close affordance: its own dismiss button.
    // The flow's own cancel × (a Link to /) is hidden inside the overlay so it
    // doesn't double up or navigate the user away on close.
    await expect(page.getByTestId("check-in-overlay-close")).toBeVisible();
    await expect(page.getByTestId("check-in-close")).toBeHidden();
  });

  test("the close button dismisses the overlay", async ({ page }) => {
    await gotoReady(page, "/");
    await page.getByTestId("check-in-button").click();
    await expect(page.getByTestId("check-in-overlay-scrim")).toBeVisible();

    await page.getByTestId("check-in-overlay-close").click();

    await expect(page.getByTestId("check-in-overlay-scrim")).toHaveCount(0);
    await expect(page).toHaveURL(/\/$/);
  });

  test("clicking the scrim outside the card dismisses the overlay", async ({
    page,
  }) => {
    await gotoReady(page, "/");
    await page.getByTestId("check-in-button").click();
    await expect(page.getByTestId("check-in-overlay-scrim")).toBeVisible();

    // Top-left corner is scrim, well outside the centered card.
    await page.getByTestId("check-in-overlay-scrim").click({
      position: { x: 8, y: 8 },
    });

    await expect(page.getByTestId("check-in-overlay-scrim")).toHaveCount(0);
  });

  test("Escape dismisses the overlay", async ({ page }) => {
    await gotoReady(page, "/");
    await page.getByTestId("check-in-button").click();
    await expect(page.getByTestId("check-in-overlay-scrim")).toBeVisible();

    // Opening the overlay moves focus into the dialog panel, so a global
    // Escape (no prior click) dismisses it — the real keyboard-user contract.
    await expect(page.getByTestId("check-in-overlay-panel")).toBeFocused();
    await page.keyboard.press("Escape");

    await expect(page.getByTestId("check-in-overlay-scrim")).toHaveCount(0);
  });

  // Every route wraps its own `ScreenLayout`, so a navigation rebuilds the
  // overlay's whole subtree — the modal cannot notice the route change after
  // the fact, and the paths that navigate have to dismiss it themselves.
  test("View book on the success screen navigates and dismisses the overlay", async ({
    page,
    request,
  }) => {
    const uuid = await fetchBookUuidByTitle(request, TARGET.title);
    await mockJsonPost(page, /\/api\/rpc\/scan\/resolve$/, {
      kind: "in_library_unowned",
      book: scanBook(uuid),
    });
    await mockJsonPost(page, "**/api/rpc/scan/check-in", { book_uuid: uuid });
    await openOverlay(page);
    await submitIsbn(page);
    await expect(page.getByTestId("check-in-confirm")).toBeVisible();
    await expectMutation(
      page,
      { method: "POST", url: "/api/rpc/scan/check-in", expectedStatus: 200 },
      async () => page.getByTestId("check-in-confirm-submit").click(),
    );
    await expect(page.getByTestId("check-in-view-book")).toBeVisible();

    await page.getByTestId("check-in-view-book").click();

    await expect(page).toHaveURL(new RegExp(`/books/${uuid}$`));
    await expect(
      page.getByRole("heading", { level: 1, name: TARGET.title }),
    ).toBeVisible();
    await expectOverlayDismissed(page);
  });

  test("an already-owned scan from the overlay navigates and dismisses it", async ({
    page,
    request,
  }) => {
    const uuid = await fetchBookUuidByTitle(request, TARGET.title);
    await mockJsonPost(page, /\/api\/rpc\/scan\/resolve$/, {
      kind: "already_owned",
      book: scanBook(uuid, { has_physical: true }),
    });
    await openOverlay(page);

    await submitIsbn(page);

    await expect(page).toHaveURL(new RegExp(`/books/${uuid}$`));
    await expect(
      page.getByRole("heading", { level: 1, name: TARGET.title }),
    ).toBeVisible();
    await expectOverlayDismissed(page);
  });

  // The same navigating outcome off the other resolve entry point: a title
  // search result the reader picked, which resolves through `resolve-meta`.
  test("picking a wishlisted search result navigates and dismisses the overlay", async ({
    page,
    request,
  }) => {
    const uuid = await fetchBookUuidByTitle(request, TARGET.title);
    await mockJsonPost(page, "**/api/rpc/scan/search", {
      results: [
        {
          isbn13: ISBN,
          title: TARGET.title,
          authors: TARGET.authors,
          year: null,
          pages: null,
          publisher: null,
          description: null,
          cover_url: null,
          series: null,
          first_publish_year: null,
          source: "open_library",
        },
      ],
    });
    await mockJsonPost(page, /\/api\/rpc\/scan\/resolve-meta$/, {
      kind: "on_wishlist",
      book: scanBook(uuid),
    });
    await openOverlay(page);
    // Scoped to the panel: the overlay floats over the landing page, whose
    // sort control is also labelled "Title".
    await page
      .getByTestId("check-in-overlay-panel")
      .getByLabel("Title")
      .fill(TARGET.title);
    await expectMutation(
      page,
      { method: "POST", url: "/api/rpc/scan/search", expectedStatus: 200 },
      async () => page.getByTestId("check-in-search-submit").click(),
    );

    await expectMutation(
      page,
      {
        method: "POST",
        url: /\/api\/rpc\/scan\/resolve-meta$/,
        expectedStatus: 200,
      },
      async () => page.getByTestId("check-in-search-result").first().click(),
    );

    await expect(page).toHaveURL(new RegExp(`/books/${uuid}$`));
    await expectOverlayDismissed(page);
  });
});
