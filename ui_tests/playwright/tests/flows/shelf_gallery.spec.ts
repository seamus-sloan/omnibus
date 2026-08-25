import type { APIRequestContext } from "@playwright/test";

import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle, switchToTableView } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// The gallery filters the landing book list in place, so it needs the real
// fixture library plus a manual shelf holding one distinct book. Alpha is
// also used by shelves.spec.ts rail tests, but only ever read — no spec
// mutates it, per the fixture-ownership rule.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

const createManualShelf = async (
  request: APIRequestContext,
  name: string,
  bookUuids: string[],
) => {
  const resp = await request.post("/api/rpc/shelves/create", {
    data: {
      req: {
        kind: "manual",
        name,
        description: null,
        visibility: "private",
        match_mode: null,
        rules: [],
        book_uuids: bookUuids,
      },
    },
  });
  expect(resp.status(), `POST /api/rpc/shelves/create failed for ${name}`).toBe(
    200,
  );
  const shelf = (await resp.json()) as { id: number };
  return shelf.id;
};

test("selects a shelf in place: glow moves, list filters, URL stays put", async ({
  page,
  request,
}) => {
  const alphaUuid = await fetchBookUuidByTitle(request, "Alpha");
  const name = `E2E Gallery ${Date.now()}`;
  const shelfId = await createManualShelf(request, name, [alphaUuid]);

  await gotoReady(page, "/");
  await switchToTableView(page);

  // All Books glows (is selected) by default.
  await expect(page.getByTestId("gallery-all-books")).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  const tile = page.getByTestId(`gallery-shelf-${shelfId}`);
  await expect(tile).toBeVisible();
  // The member fetch is a server-fn POST, so await it like a mutation to
  // guarantee the list swap has landed before asserting UI state.
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/page", expectedStatus: 200 },
    async () => tile.click(),
  );

  await expect(tile).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("gallery-all-books")).toHaveAttribute(
    "aria-pressed",
    "false",
  );
  await expect(page.getByTestId("lib-section-title")).toContainText(name);
  // Only the shelf member renders; selection filtered in place (no navigation).
  await expect(page.getByTestId(/^ebook-row-/)).toHaveCount(1);
  await expect(page.getByTestId("ebook-table")).toContainText("Alpha");
  expect(new URL(page.url()).pathname).toBe("/");
});

test("persists the selected shelf across a reload", async ({
  page,
  request,
}) => {
  const alphaUuid = await fetchBookUuidByTitle(request, "Alpha");
  const name = `E2E Gallery Persist ${Date.now()}`;
  const shelfId = await createManualShelf(request, name, [alphaUuid]);

  await gotoReady(page, "/");
  await switchToTableView(page);
  const tile = page.getByTestId(`gallery-shelf-${shelfId}`);
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/page", expectedStatus: 200 },
    async () => tile.click(),
  );
  await expect(tile).toHaveAttribute("aria-pressed", "true");

  // The persisted pick reconciles post-mount (rule 07) and refires the
  // member fetch — await it so the assertions can't race the list swap.
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/page", expectedStatus: 200 },
    async () => page.reload(),
  );

  await expect(page.getByTestId(`gallery-shelf-${shelfId}`)).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.getByTestId("lib-section-title")).toContainText(name);
  await expect(page.getByTestId(/^ebook-row-/)).toHaveCount(1);
});

test("returns to All Books and restores the full library", async ({
  page,
  request,
}) => {
  const alphaUuid = await fetchBookUuidByTitle(request, "Alpha");
  const name = `E2E Gallery Back ${Date.now()}`;
  const shelfId = await createManualShelf(request, name, [alphaUuid]);

  await gotoReady(page, "/");
  await switchToTableView(page);
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/page", expectedStatus: 200 },
    async () => page.getByTestId(`gallery-shelf-${shelfId}`).click(),
  );
  await expect(page.getByTestId(/^ebook-row-/)).toHaveCount(1);

  await page.getByTestId("gallery-all-books").click();

  await expect(page.getByTestId("gallery-all-books")).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.getByTestId("lib-section-title")).toContainText(
    "All Books",
  );
  await expect
    .poll(async () => page.getByTestId(/^ebook-row-/).count())
    .toBeGreaterThanOrEqual(FIXTURE_BOOKS.length);
});

test("edits the selected shelf from the landing header pencil", async ({
  page,
  request,
}) => {
  // A smart shelf so the facet row has a rule chip to assert. The author
  // rule only reads fixture books — nothing here mutates a shared fixture.
  const name = `E2E Gallery Edit ${Date.now()}`;
  const createResp = await request.post("/api/rpc/shelves/create", {
    data: {
      req: {
        kind: "smart",
        name,
        description: null,
        visibility: "private",
        match_mode: "any",
        rules: [{ field: "author", op: "is", value: "Ada Lovelace" }],
        book_uuids: [],
      },
    },
  });
  expect(createResp.status(), "POST /api/rpc/shelves/create failed").toBe(200);
  const shelf = (await createResp.json()) as { id: number };

  await gotoReady(page, "/");
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/shelves/page", expectedStatus: 200 },
    async () => page.getByTestId(`gallery-shelf-${shelf.id}`).click(),
  );

  // Facet row under the section title states the kind, visibility, and the
  // rule query once the full-shelf fetch lands.
  const facets = page.getByTestId("shelf-facets");
  await expect(facets).toContainText("Smart shelf");
  await expect(facets).toContainText("Private");
  await expect(facets).toContainText("Author is Ada Lovelace");

  await page.getByTestId("shelf-edit").click();
  const modal = page.getByTestId("edit-shelf-modal");
  await expect(modal.getByTestId("edit-shelf-name")).toHaveValue(name);

  const renamed = `${name} v2`;
  await modal.getByTestId("edit-shelf-name").fill(renamed);
  await modal.getByTestId("shelf-vis-public").click();

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/shelves/update",
      expectedBody: {
        id: shelf.id,
        req: {
          name: renamed,
          description: null,
          visibility: "public",
          match_mode: "any",
          rules: [{ field: "author", op: "is", value: "Ada Lovelace" }],
          sync_to_kobo: false,
        },
      },
      expectedStatus: 200,
    },
    async () => modal.getByTestId("edit-shelf-save").click(),
  );

  await expect(modal).not.toBeVisible();
  await expect(page.getByTestId("lib-section-title")).toContainText(renamed);
  await expect(facets).toContainText("Public");
});

/**
 * Write an epub position for `uuid`, then load the landing page — repeating
 * until that book's hero card is on the rail.
 *
 * The rail is `recent_progress`: the five newest positions ordered by
 * `COALESCE(client_updated_at, updated_at) DESC, book_uuid`. That timestamp
 * has **second** granularity, so every progress write landing in the same
 * second ties and is ordered by uuid instead — and the suite is
 * `fullyParallel` against one server, with a dozen specs opening readers and
 * players. A book seeded once can therefore be ordered off the end of the
 * rail by uuid alone, with nothing wrong on either side.
 *
 * Re-seeding on each attempt is what converges: a later second breaks the tie
 * in this book's favour.
 */
async function seedProgressAndOpenLanding(
  page: import("@playwright/test").Page,
  request: import("@playwright/test").APIRequestContext,
  uuid: string,
): Promise<void> {
  await expect
    .poll(
      async () => {
        const resp = await request.post("/api/progress", {
          data: {
            book_uuid: uuid,
            format: "epub",
            epub_cfi: "epubcfi(/6/4!/4/2/2[c01]/2/1:0)",
          },
        });
        expect(resp.status(), "POST /api/progress failed").toBe(200);
        await gotoReady(page, "/");
        return page.getByTestId(`hero-resume-${uuid}`).count();
      },
      {
        timeout: 30_000,
        message: `book ${uuid} never reached the continue-reading stack`,
      },
    )
    .toBeGreaterThan(0);
}

test("shows the continue-reading stack once a book has progress", async ({
  page,
  request,
}) => {
  // `standalone-mountain` is reserved for this test and read by no other
  // spec. It used to use Alpha, which `merge.spec.ts` merges an audiobook
  // into and then undoes — mutating that book's formats and its
  // `reading_progress` rows mid-suite. `merge.spec.ts` takes `withLock`, but
  // that only serializes writers; a reader like this one is unprotected, so
  // the rail could be asserted against Alpha mid-merge.
  const uuid = await fetchBookUuidByTitle(request, "Berg der Beweise");
  await seedProgressAndOpenLanding(page, request, uuid);

  await expect(page.getByTestId("continue-stack")).toBeVisible();
  const card = page.getByTestId(`hero-resume-${uuid}`);
  await expect(card).toBeVisible();
  // Every card in the fan keeps a real href to its own resume route, whether
  // or not it is the one out front — that is what preserves middle-click and
  // open-in-new-tab on the resume affordance.
  await expect(card).toHaveAttribute("href", `/read/${uuid}`);
});

test("drops a book from the continue-reading stack once it is finished", async ({
  page,
  request,
}) => {
  // `standalone-desert` is reserved for this test and read by no other spec.
  // Read status is per-(user, book) server state and the suite is
  // fullyParallel against one server, so marking a book finished is globally
  // visible the moment it lands — and now also takes it off the rail.
  const uuid = await fetchBookUuidByTitle(request, "Desert Protocols");

  // Explicitly `reading` rather than relying on the absent-row default, and
  // set *before* the rail is polled: this test's own last act is to mark the
  // book finished, and that state outlives the run against the shared dev
  // server. Without the reset a second run would fail on the state the first
  // one left — and the filter under test would keep it off the rail forever.
  const reading = await request.put("/api/read-status", {
    data: { book_uuid: uuid, status: "reading" },
  });
  expect(reading.status(), "PUT /api/read-status failed").toBe(200);

  await seedProgressAndOpenLanding(page, request, uuid);
  await expect(page.getByTestId(`hero-resume-${uuid}`)).toBeVisible();

  const finished = await request.put("/api/read-status", {
    data: { book_uuid: uuid, status: "finished" },
  });
  expect(finished.status(), "PUT /api/read-status failed").toBe(200);

  await gotoReady(page, "/");
  // Anchor on a landing element that is always present: the stack hides
  // itself when it has no cards, and whether it has others depends on specs
  // running in parallel — so asserting the page rendered has to come from
  // elsewhere, or a card missing because nothing loaded would read as a pass.
  await expect(page.getByTestId("lib-section-title")).toBeVisible();
  await expect(page.getByTestId(`hero-resume-${uuid}`)).toBeHidden();
});

test("the shelves row pages horizontally and arms only the end with more", async ({
  page,
  request,
}) => {
  // Enough shelves to overflow the row at any sane viewport. They are this
  // test's own, created per-run, so nothing else reads them.
  const stamp = Date.now();
  for (let i = 0; i < 8; i++) {
    await createManualShelf(request, `E2E Paging ${stamp}-${i}`, []);
  }
  await gotoReady(page, "/");

  const row = page.getByTestId("shelf-gallery").locator("#lmq-shelf-row");
  const prev = page.getByTestId("shelf-row-prev");
  const next = page.getByTestId("shelf-row-next");

  // At the left end only the forward arrow is armed. `marquee.js` toggles the
  // `on` class and the tabindex together, so an un-armed arrow is also out of
  // the tab order rather than a focusable no-op.
  await expect(next).toHaveClass(/\bon\b/);
  await expect(prev).not.toHaveClass(/\bon\b/);
  await expect(prev).toHaveAttribute("tabindex", "-1");

  await next.click();
  // The nudge is a smooth scroll, so poll rather than asserting the frame
  // the click returned on.
  await expect
    .poll(async () => row.evaluate((n) => n.scrollLeft), { timeout: 5_000 })
    .toBeGreaterThan(0);
  await expect(prev).toHaveClass(/\bon\b/);
  await expect(prev).toHaveAttribute("tabindex", "0");

  await prev.click();
  await expect
    .poll(async () => row.evaluate((n) => n.scrollLeft), { timeout: 5_000 })
    .toBe(0);
  await expect(prev).not.toHaveClass(/\bon\b/);
});

test("a book behind the front one comes forward instead of navigating", async ({
  page,
  request,
}) => {
  // `standalone-island` and `standalone-garden` are reserved for this test and
  // read by no other spec: it needs two books on the stack at once, and any
  // book another spec asserts read status on could be filtered off the rail
  // mid-run (`db::progress::recent_progress` drops `unread` and `finished`).
  const island = await fetchBookUuidByTitle(request, "The Isle of Functions");
  const garden = await fetchBookUuidByTitle(request, "The Garden of Closures");
  await seedProgressAndOpenLanding(page, request, island);
  await seedProgressAndOpenLanding(page, request, garden);

  const stack = page.getByTestId("continue-stack");
  const cards = stack.locator(".lmq-fcard");
  await expect.poll(async () => cards.count()).toBeGreaterThan(1);

  // Which book is out front depends on second-granularity ordering with the
  // uuid as tiebreak, and on whatever the parallel suite has stamped — so
  // read the fan rather than assuming, exactly as the rail specs do.
  const front = stack.locator(".lmq-fcard.lead");
  const behind = stack.locator(".lmq-fcard:not(.lead)").first();
  const frontId = await front.getAttribute("data-testid");
  const behindId = await behind.getAttribute("data-testid");
  expect(frontId, "the fan must have a front card").toBeTruthy();
  expect(behindId, "the fan must have a card behind it").toBeTruthy();

  // Pointing at the fan spreads it, which is what makes a card behind the
  // front one reachable at all: the cards overlap by ~74px at rest, so a back
  // card's centre is under its neighbour. Hover first, then click the left
  // sliver that is always the back card's own — the click a person makes.
  await stack.locator(".lmq-fan").hover();
  // Clicking a card behind the front one means "come forward", not "open" —
  // the card is a real link, but the router navigation is suppressed for it.
  await behind.click({ position: { x: 12, y: 60 } });
  await expect(stack.locator(`[data-testid="${behindId}"]`)).toHaveClass(
    /\blead\b/,
  );
  await expect(stack.locator(`[data-testid="${frontId}"]`)).not.toHaveClass(
    /\blead\b/,
  );
  await expect(page).toHaveURL(/\/$/);

  // The veil — the resume verb — travels with the front card.
  await expect(
    stack.locator(`[data-testid="${behindId}"] .lmq-veil-lab`),
  ).toBeAttached();
  await expect(
    stack.locator(`[data-testid="${frontId}"] .lmq-veil-lab`),
  ).toHaveCount(0);
});

test("resume stays reachable as an edge ribbon once the stack scrolls away", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, "Berg der Beweise");
  await seedProgressAndOpenLanding(page, request, uuid);

  const root = page.locator("#lmq-root");
  const ribbon = page.getByTestId("resume-edge");
  // Off-screen and inert while the stack itself is still on screen — the
  // ribbon exists in the DOM from the first paint so SSR and hydration agree
  // (rule 07), and `marquee.js` reveals it on scroll.
  await expect(ribbon).toBeAttached();
  await expect(root).not.toHaveClass(/lmq--past/);
  await expect(ribbon).toHaveCSS("opacity", "0");

  await page.evaluate(() => window.scrollTo(0, 600));
  await expect(root).toHaveClass(/lmq--past/);
  await expect(ribbon).toHaveCSS("opacity", "1");
  // Widening on hover is what exposes the control; the collapsed strip is
  // just the marker.
  await ribbon.hover();
  await expect(page.getByTestId(`resume-edge-go-${uuid}`)).toBeVisible();

  await page.evaluate(() => window.scrollTo(0, 0));
  await expect(root).not.toHaveClass(/lmq--past/);
});
