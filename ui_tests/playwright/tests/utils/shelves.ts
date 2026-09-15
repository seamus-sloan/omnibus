import type { APIRequestContext, Locator, Page } from "@playwright/test";
import { expect } from "../fixtures/test";
import { gotoReady } from "./nav";

/**
 * Create a shelf owned by the suite's admin straight through the API and
 * return its id. `body` overrides the empty-private-manual defaults.
 */
export async function createShelf(
  request: APIRequestContext,
  body: Record<string, unknown>,
): Promise<number> {
  const resp = await request.post("/api/rpc/shelves/create", {
    data: {
      req: {
        description: null,
        visibility: "private",
        match_mode: null,
        rules: [],
        book_uuids: [],
        ...body,
      },
    },
  });
  expect(
    resp.status(),
    `POST /api/rpc/shelves/create failed for ${body.name}`,
  ).toBe(200);
  return ((await resp.json()) as { id: number }).id;
}

// A shelf opens two ways on web, and they are different surfaces. Both start
// in the landing page's shelf row — the top nav has no Shelves link. The row's
// "All shelves" link reaches the `/shelves` index, whose cards navigate to
// `/shelves/:id` (`openShelfFromIndex`); selecting a shelf in the row itself
// filters the landing book list in place and never navigates
// (`selectShelfInGallery`). Never `page.goto` a shelf id directly — arrive
// the way a reader does.
//
// The row carries a subset: another reader's private shelves and *any*
// wishlist but the viewer's own stocked one are filtered out of it (see
// `rail_shelves`). Reach those through the index, which lists everything.

/** The card for `shelfId` on the `/shelves` index. */
export function shelfCard(page: Page, shelfId: number): Locator {
  return page.getByTestId(`shelf-card-${shelfId}`);
}

/** Open `shelfId` from the `/shelves` index and wait for its page. */
export async function openShelfFromIndex(
  page: Page,
  shelfId: number,
): Promise<void> {
  await gotoReady(page, "/shelves");
  await shelfCard(page, shelfId).click();
  await expect(page).toHaveURL(new RegExp(`/shelves/${shelfId}$`));
}

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
