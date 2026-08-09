import type { Locator, Page } from "@playwright/test";
import { expect } from "../fixtures/test";

// Opening a shelf on web means picking its tile in the landing page's shelf
// gallery, which filters the landing book list in place. `/shelves/:id` is
// **not** reachable through the web UI — the only `Link`s to it live in
// `ShelvesRail`, which is mounted on that page and nowhere else — so a spec
// that deep-links it exercises a surface no user can reach. Route every shelf
// assertion through here instead.

/** The gallery tile for `shelfId` on the landing page. */
export function galleryTile(page: Page, shelfId: number): Locator {
  return page.getByTestId(`gallery-shelf-${shelfId}`);
}

/**
 * Pick `shelfId` in the landing gallery and wait for the header to name it.
 * Assumes the caller is already on `/`.
 */
export async function selectShelfInGallery(
  page: Page,
  shelfId: number,
  shelfName: string,
): Promise<void> {
  await galleryTile(page, shelfId).click();
  await expect(page.getByTestId("lib-section-title")).toContainText(shelfName);
}

/**
 * The landing grid tile for `title`. The tile is an `<a>` with no `href` and
 * an explicit `role="listitem"` (see `landing/grid.rs`), so it is *not* a
 * `link` in the a11y tree — `getByRole("link")` never matches it.
 */
export function bookTile(page: Page, title: string): Locator {
  return page.getByRole("listitem", { name: `Open details for ${title}` });
}
