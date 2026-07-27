import type { Page } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Re-seed in this spec's beforeAll so the running server is indexed against
// the committed EPUB fixtures before any assertion runs — independent of
// whatever other specs in the same worker did before us.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// A fixture with predictable, distinctive metadata to drive most tests.
const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "alpha")!;
// A separate book for the highlight action test so seeding/deleting highlights
// doesn't pollute the empty-state assertions on TARGET (tests run in parallel).
const HL_BOOK = FIXTURE_BOOKS.find((b) => b.slug === "beta")!;
// A third book for the note-editing test so its persisted highlight never
// collides with the seed/delete cycle on HL_BOOK.
const NOTE_BOOK = FIXTURE_BOOKS.find((b) => b.slug === "gamma")!;
// A fourth book for the recolor test so its seeded highlight is isolated from
// the other highlight specs running in parallel.
const RECOLOR_BOOK = FIXTURE_BOOKS.find((b) => b.slug === "pioneers-3")!;
// Reserved books for the progress-restore tests (see fixtures/epubs.ts):
// saved position is per-book state on the shared server, so any other spec
// opening these in the reader would race the restore assertions. They must
// be real multi-page books (public-domain fixtures) — the synthetic
// one-paragraph EPUBs render as a single page with no page turns.
const PROGRESS_BOOK = FIXTURE_BOOKS.find((b) => b.slug === "frankenstein")!;
const PROGRESS_ERR_BOOK = FIXTURE_BOOKS.find((b) => b.slug === "great-gatsby")!;
// Reserved for the `?cfi=` deep-link test — book detail's saved-passages list
// opens the reader that way. Same isolation rationale as the two above (the
// test drives and reads saved position) and the same multi-page requirement.
// Deliberately one of the *smaller* public-domain fixtures: this test opens
// the book twice, and epub.js regenerates whole-book locations on the first
// cold open (later opens hit the localStorage cache). On a fresh CI shard a
// large fixture blows the pagination poll before a single page turn lands.
const DEEP_LINK_BOOK = FIXTURE_BOOKS.find(
  (b) => b.slug === "romeo-and-juliet",
)!;

// The epub.js progress POST fires on the reader's relocate events; pin the
// exact pathname so the sibling `/api/rpc/progress/get` reads never match.
const PROGRESS_POST = {
  method: "POST" as const,
  url: /\/api\/rpc\/progress(?:\?|$)/,
  expectedStatus: 200,
};

// The bottom-bar page label ("p. 3 of 12 · 25%") renders only after epub.js
// has painted AND the whole-book pagination resolved — the strongest signal
// that relocates are flowing. Note the NBSP after "p." in the label.
const PAGE_LABEL = /p\.\u00a0\d+ of \d+/;

async function footerPageLabel(page: Page): Promise<string> {
  const text = (await page.getByTestId("reader-footer").textContent()) ?? "";
  const m = text.match(PAGE_LABEL);
  return m ? m[0] : "";
}

test("renders the reader layout", async ({ page, request }) => {
  // Deep-link straight to the immersive reader by the book's stable uuid,
  // resolved the same way a real click would.
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/read/${uuid}`);

  // The immersive reader mounts a viewer container plus chrome bars. We
  // assert only on the chrome — never on rendered EPUB text — because the
  // epub.js render is JS-engine-driven and may not paint headlessly (and the
  // book file may be absent in some seed states). The container/controls are
  // SSR markup and are robust to that.
  await expect(page.getByTestId("reader-viewer")).toBeVisible();

  // Top chrome: back, contents, search, Aa, highlights, bookmark.
  await expect(page.getByTestId("reader-back")).toBeVisible();
  await expect(page.getByTestId("reader-toc")).toBeVisible();
  await expect(page.getByTestId("reader-search")).toBeVisible();
  await expect(page.getByTestId("reader-aa")).toBeVisible();
  await expect(page.getByTestId("reader-highlights")).toBeVisible();
  await expect(page.getByTestId("reader-bookmark")).toBeVisible();

  // Page-turn gutters.
  await expect(page.getByTestId("reader-prev")).toBeVisible();
  await expect(page.getByTestId("reader-next")).toBeVisible();

  // Font + page-view controls are inside the Aa panel — open it first.
  await page.getByTestId("reader-aa").click();
  await expect(page.getByTestId("reader-font-decrease")).toBeVisible();
  await expect(page.getByTestId("reader-font-increase")).toBeVisible();
  await expect(page.getByTestId("reader-spread-single")).toBeVisible();
  await expect(page.getByTestId("reader-spread-double")).toBeVisible();
});

test("black theme overrides a publisher color on the EPUB body", async ({
  page,
  request,
}) => {
  await page.addInitScript(() => {
    window.localStorage.setItem("omn.theme", "black");
  });
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/read/${uuid}`);

  // Intentional exception: this regression must inspect rendered EPUB prose
  // to verify a publisher body colour cannot override the reader theme.
  const prose = page
    .frameLocator("#omnibus-viewer iframe")
    .getByText("Synthetic test content.");
  await expect(prose).toBeVisible();
  await expect
    .poll(() => prose.evaluate((el) => getComputedStyle(el).color))
    .toBe("rgb(255, 255, 255)");
});

test("opens the search and bookmarks drawers from the top chrome", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/read/${uuid}`);
  await expect(page.getByTestId("reader-viewer")).toBeVisible();

  // Search drawer opens with its query input visible.
  await page.getByTestId("reader-search").click();
  await expect(page.getByTestId("reader-search-drawer")).toBeVisible();
  await expect(page.getByTestId("reader-search-input")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("reader-search-drawer")).toHaveCount(0);

  // Bookmarks drawer: empty state + the "+ Bookmark" affordance.
  await page.getByTestId("reader-bookmark").click();
  await expect(page.getByTestId("reader-bookmarks-drawer")).toBeVisible();
  await expect(page.getByTestId("reader-bookmark-add")).toBeVisible();
});

test("opens the contents and highlights drawers from the top chrome", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/read/${uuid}`);
  await expect(page.getByTestId("reader-viewer")).toBeVisible();

  // Highlights drawer opens with the (empty) palette filter rail.
  await page.getByTestId("reader-highlights").click();
  await expect(page.getByTestId("reader-highlights-drawer")).toBeVisible();
  await expect(page.getByText("No highlights yet")).toBeVisible();

  // Escape peels the drawer back (its scrim otherwise covers the chrome).
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("reader-highlights-drawer")).toHaveCount(0);

  // Contents drawer opens from the top chrome.
  await page.getByTestId("reader-toc").click();
  await expect(page.getByTestId("reader-toc-drawer")).toBeVisible();
});

test("seeds a highlight and deletes it from the highlights drawer", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, HL_BOOK.title);

  // Seed a highlight via the REST API (bearer-authed request fixture) so the
  // reader loads it into the drawer on mount. A unique quote keeps the test
  // robust against the shared, persistent dev DB.
  const quote = `seeded passage ${Date.now()}`;
  const created = await request.post("/api/highlights", {
    data: {
      book_uuid: uuid,
      epub_cfi_range: "epubcfi(/6/4!/4/2,/1:0,/1:40)",
      color: "amber",
      text: quote,
    },
  });
  expect(created.status(), "seed highlight").toBe(200);

  await gotoReady(page, `/read/${uuid}`);
  await expect(page.getByTestId("reader-viewer")).toBeVisible();

  // Open the highlights drawer — the seeded highlight is listed.
  await page.getByTestId("reader-highlights").click();
  await expect(page.getByTestId("reader-highlights-drawer")).toBeVisible();
  const row = page
    .getByTestId("reader-highlight-row")
    .filter({ hasText: quote });
  await expect(row).toHaveCount(1);

  // Delete it — the delete RPC must fire, and the row disappears.
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/highlights\/delete(?:\?|$)/,
      expectedStatus: 200,
    },
    async () => row.getByTestId("highlight-delete").click(),
  );
  await expect(page.getByText(quote)).toHaveCount(0);
});

test("adds a note to an existing highlight from the drawer", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, NOTE_BOOK.title);

  // Seed a note-less highlight so the drawer offers the "Add note" affordance.
  const quote = `note passage ${Date.now()}`;
  const created = await request.post("/api/highlights", {
    data: {
      book_uuid: uuid,
      epub_cfi_range: "epubcfi(/6/4!/4/2,/1:0,/1:40)",
      color: "green",
      text: quote,
    },
  });
  expect(created.status(), "seed highlight").toBe(200);

  await gotoReady(page, `/read/${uuid}`);
  await expect(page.getByTestId("reader-viewer")).toBeVisible();

  // Open the drawer; the seeded highlight shows an "Add note" button.
  await page.getByTestId("reader-highlights").click();
  await expect(page.getByTestId("reader-highlights-drawer")).toBeVisible();
  const row = page
    .getByTestId("reader-highlight-row")
    .filter({ hasText: quote });
  await expect(row).toHaveCount(1);
  await expect(row.getByTestId("highlight-note")).toHaveText("Add note");

  // The note button opens the shared composer over the drawer.
  await row.getByTestId("highlight-note").click();
  await expect(page.getByTestId("reader-note-composer")).toBeVisible();

  const note = `a thought worth keeping ${Date.now()}`;
  await page.getByTestId("reader-note-input").fill(note);

  // Saving persists via the update-note RPC and closes the composer.
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/highlights\/update-note(?:\?|$)/,
      expectedStatus: 200,
    },
    async () => page.getByTestId("reader-note-save").click(),
  );
  await expect(page.getByTestId("reader-note-composer")).toHaveCount(0);

  // Reopen the drawer: the note now renders on the row, and the button flips
  // to "Edit note" to reflect the persisted note.
  await page.getByTestId("reader-highlights").click();
  const savedRow = page
    .getByTestId("reader-highlight-row")
    .filter({ hasText: quote });
  await expect(savedRow.getByText(note)).toBeVisible();
  await expect(savedRow.getByTestId("highlight-note")).toHaveText("Edit note");
});

test("recolors an existing highlight from the drawer", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, RECOLOR_BOOK.title);

  // Seed an amber highlight so the drawer offers its recolor swatches.
  const quote = `recolor passage ${Date.now()}`;
  const created = await request.post("/api/highlights", {
    data: {
      book_uuid: uuid,
      epub_cfi_range: "epubcfi(/6/4!/4/2,/1:0,/1:40)",
      color: "amber",
      text: quote,
    },
  });
  expect(created.status(), "seed highlight").toBe(200);

  await gotoReady(page, `/read/${uuid}`);
  await expect(page.getByTestId("reader-viewer")).toBeVisible();

  await page.getByTestId("reader-highlights").click();
  await expect(page.getByTestId("reader-highlights-drawer")).toBeVisible();
  const row = page
    .getByTestId("reader-highlight-row")
    .filter({ hasText: quote });
  await expect(row).toHaveCount(1);

  // Amber is the current color; green is not yet selected.
  await expect(
    row.getByTestId("reader-highlight-recolor-amber"),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(
    row.getByTestId("reader-highlight-recolor-green"),
  ).toHaveAttribute("aria-pressed", "false");

  // Picking green fires the update-color RPC and moves the selected swatch.
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/highlights\/update-color(?:\?|$)/,
      expectedStatus: 200,
    },
    async () => row.getByTestId("reader-highlight-recolor-green").click(),
  );
  await expect(
    row.getByTestId("reader-highlight-recolor-green"),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(
    row.getByTestId("reader-highlight-recolor-amber"),
  ).toHaveAttribute("aria-pressed", "false");
});

test("opens the reader from the book detail Read action", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  // The EPUB "Read" CTA in the format switcher routes into the immersive
  // reader. This is a read-only client-side navigation (no mutation), so we
  // follow the SPA route change and assert on the destination chrome.
  await page.getByTestId("action-read").click();
  await expect(page).toHaveURL(new RegExp(`/read/${uuid}$`));
  await expect(page.getByTestId("reader-viewer")).toBeVisible();
});

test("restores the exact reading position when the reader is reopened", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, PROGRESS_BOOK.title);

  await gotoReady(page, `/read/${uuid}`);
  await expect(page.getByTestId("reader-viewer")).toBeVisible();

  // Page turns only produce label changes once whole-book pagination has
  // resolved (the first open generates it; later opens hit the locations
  // cache in localStorage).
  await expect
    .poll(async () => footerPageLabel(page), { timeout: 20_000 })
    .toMatch(PAGE_LABEL);
  const start = await footerPageLabel(page);

  // Turn eight spreads — far enough past the front matter to be in prose —
  // then keep turning (bounded) until the viewport start sits mid-paragraph:
  // a text-position CFI with a non-zero character offset (`…:469)`). Leaving
  // from the title/contents pages or an element boundary would under-test
  // the restore, since boundary CFIs restore at coarser granularity in
  // epub.js (a pre-existing display() limitation) while mid-text CFIs
  // restore exactly. The offset check is fixture- and version-agnostic, and
  // an environment that never yields one fails the guard below loudly
  // instead of silently under-testing. Each turn's save is awaited via
  // expectMutation so the 400 ms relocate debounce never coalesces two
  // turns into one save.
  const midTextCfi = /:[1-9]\d*\)$/;
  let leftCfi = "";
  for (let turn = 0; turn < 12; turn++) {
    const saved = await expectMutation(page, PROGRESS_POST, async () =>
      page.getByTestId("reader-next").click(),
    );
    leftCfi = saved.request.postDataJSON().update.epub_cfi as string;
    if (turn >= 7 && midTextCfi.test(leftCfi)) break;
  }
  expect(leftCfi, "should reach a mid-paragraph position").toMatch(midTextCfi);
  const leftAt = await footerPageLabel(page);
  expect(leftAt, "the turns should move the page label").not.toBe(start);

  // The stored position after a reopen must converge back on exactly where
  // we left: the restore may briefly persist a pre-correction snapshot, but
  // the settled landing (and the one-spread nudge for boundary CFIs) always
  // wins the last write.
  const storedCfi = async () => {
    const rec = await request.get(`/api/progress/${uuid}?format=epub`);
    if (!rec.ok()) return "";
    const body = (await rec.json()) as { epub_cfi?: string } | null;
    return body?.epub_cfi ?? "";
  };

  // Leave the reader (explicit navigation — the reader-back button walks
  // browser history, which in a test context leads to about:blank), then
  // reopen: it must land on the exact page we left, not a page drifted by
  // the first-pass display's pre-webfont layout. The restore POST is
  // page-load triggered (hydration + epub render + settle + debounce), so
  // give the mutation waiter more room than a click gets.
  await gotoReady(page, `/books/${uuid}`);
  await expectMutation(page, { ...PROGRESS_POST, timeout: 20_000 }, async () =>
    gotoReady(page, `/read/${uuid}`),
  );
  await expect.poll(storedCfi, { timeout: 10_000 }).toBe(leftCfi);
  await expect
    .poll(async () => footerPageLabel(page), { timeout: 20_000 })
    .toBe(leftAt);

  // And again: the first reopen must not have rewritten the stored position,
  // so a second reopen still lands on the same page (no cumulative drift).
  await gotoReady(page, `/books/${uuid}`);
  await expectMutation(page, { ...PROGRESS_POST, timeout: 20_000 }, async () =>
    gotoReady(page, `/read/${uuid}`),
  );
  await expect.poll(storedCfi, { timeout: 10_000 }).toBe(leftCfi);
  await expect
    .poll(async () => footerPageLabel(page), { timeout: 20_000 })
    .toBe(leftAt);
});

test("a ?cfi= deep link opens the reader at that passage, not the resume point", async ({
  page,
  request,
}) => {
  // Two sequential paginated opens (cold generate, then a cached reopen) plus
  // eight debounced page turns don't fit the 60s file default — the config
  // points such tests at their own budget.
  test.setTimeout(120_000);
  const uuid = await fetchBookUuidByTitle(request, DEEP_LINK_BOOK.title);

  // Read a way in and capture both the CFI and the page it renders on. The
  // CFI comes off the progress save, so it's exactly what a highlight taken
  // here would store.
  await gotoReady(page, `/read/${uuid}`);
  await expect(page.getByTestId("reader-viewer")).toBeVisible();
  // Cold-open headroom: this is the run that generates whole-book locations
  // (the sibling restore test's 20s assumes a book already paginated earlier
  // in the file; this book is opened for the first time here).
  await expect
    .poll(async () => footerPageLabel(page), { timeout: 45_000 })
    .toMatch(PAGE_LABEL);
  const opened = await footerPageLabel(page);

  let passageCfi = "";
  for (let turn = 0; turn < 8; turn++) {
    const saved = await expectMutation(page, PROGRESS_POST, async () =>
      page.getByTestId("reader-next").click(),
    );
    passageCfi = saved.request.postDataJSON().update.epub_cfi as string;
  }
  const passagePage = await footerPageLabel(page);
  expect(passagePage, "the turns should move the page label").not.toBe(opened);

  // Now rewind the saved position to the front of the book, so "resume" and
  // "the passage" are unmistakably different destinations.
  await gotoReady(page, `/books/${uuid}`);
  const reset = await request.post("/api/rpc/progress", {
    data: {
      update: {
        book_uuid: uuid,
        format: "epub",
        epub_cfi: "epubcfi(/6/2!/4/2/1:0)",
      },
    },
  });
  expect(reset.status(), "rewind saved progress").toBe(200);

  // Opening with ?cfi= must land on the passage's page, not the rewound one.
  await gotoReady(page, `/read/${uuid}?cfi=${encodeURIComponent(passageCfi)}`);
  await expect(page.getByTestId("reader-viewer")).toBeVisible();
  await expect
    .poll(async () => footerPageLabel(page), { timeout: 20_000 })
    .toBe(passagePage);
});

test("keeps reading when the progress save POST fails", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, PROGRESS_ERR_BOOK.title);

  // Force 500 on every progress save from this page — armed before the first
  // navigation so this test never writes real progress for its book. Pin the
  // exact pathname so `/api/rpc/progress/get` reads pass through untouched.
  await page.route(
    (url) => url.pathname === "/api/rpc/progress",
    async (route) => {
      if (route.request().method() === "POST") {
        await route.fulfill({ status: 500, body: "forced failure" });
      } else {
        await route.continue();
      }
    },
  );

  await gotoReady(page, `/read/${uuid}`);
  await expect(page.getByTestId("reader-viewer")).toBeVisible();
  await expect
    .poll(async () => footerPageLabel(page), { timeout: 20_000 })
    .toMatch(PAGE_LABEL);

  // Each turn still fires its save (observing the forced 500), and the
  // reader keeps paging normally — persistence is fire-and-forget, so a
  // second turn still attempts its save and no error UI appears.
  await expectMutation(
    page,
    { ...PROGRESS_POST, expectedStatus: 500 },
    async () => page.getByTestId("reader-next").click(),
  );
  await expectMutation(
    page,
    { ...PROGRESS_POST, expectedStatus: 500 },
    async () => page.getByTestId("reader-next").click(),
  );
  await expect(page.getByTestId("reader-viewer")).toBeVisible();
  await expect(page.getByTestId("reader-error")).toHaveCount(0);
});

// The reader is one shared component: the native mobile shell renders the
// same tree in a WebView, so the phone-width layout (F6.2 mobile ereader) is
// verified here at a phone viewport. Chrome is SSR markup, robust to the
// epub.js render not painting headlessly.
test.describe("mobile reader (phone viewport)", () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test("renders the mobile reader layout", async ({ page, request }) => {
    const uuid = await fetchBookUuidByTitle(request, TARGET.title);
    await gotoReady(page, `/read/${uuid}`);

    // Same chrome contract as desktop — the viewer and top tools stay present.
    await expect(page.getByTestId("reader-viewer")).toBeVisible();
    await expect(page.getByTestId("reader-back")).toBeVisible();
    await expect(page.getByTestId("reader-toc")).toBeVisible();

    // The circular page-turn gutters are suppressed on phone widths (mobile
    // turns pages by swipe/tap; the buttons would sit over the prose).
    await expect(page.getByTestId("reader-prev")).toBeHidden();
    await expect(page.getByTestId("reader-next")).toBeHidden();

    // Phone toolbar swaps the separate highlights + bookmarks tools for the
    // single combined Annotations tool.
    await expect(page.getByTestId("reader-annotations")).toBeVisible();
    await expect(page.getByTestId("reader-highlights")).toBeHidden();
    await expect(page.getByTestId("reader-bookmark")).toBeHidden();
  });

  test("opens the combined annotations sheet with kind filters", async ({
    page,
    request,
  }) => {
    const uuid = await fetchBookUuidByTitle(request, TARGET.title);
    await gotoReady(page, `/read/${uuid}`);
    await expect(page.getByTestId("reader-viewer")).toBeVisible();

    // One sheet for highlights + notes + bookmarks, with an All / Bookmarks
    // kind filter and the add-bookmark affordance in its header.
    await page.getByTestId("reader-annotations").click();
    const sheet = page.getByTestId("reader-annotations-drawer");
    await expect(sheet).toBeVisible();
    await expect(
      page.getByTestId("annotations-filter-bookmarks"),
    ).toBeVisible();
    await expect(page.getByTestId("reader-annotations-add")).toBeVisible();

    // Kind filter narrows to bookmarks (empty on the shared dev DB, so the
    // empty state holds) and Escape dismisses the sheet.
    await page.getByTestId("annotations-filter-bookmarks").click();
    await expect(sheet).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(sheet).toHaveCount(0);
  });

  test("drawers and the display panel span the full width", async ({
    page,
    request,
  }) => {
    const uuid = await fetchBookUuidByTitle(request, TARGET.title);
    await gotoReady(page, `/read/${uuid}`);
    await expect(page.getByTestId("reader-viewer")).toBeVisible();

    // Contents drawer stretches edge to edge instead of a 380px side panel.
    await page.getByTestId("reader-toc").click();
    const drawer = page.getByTestId("reader-toc-drawer");
    await expect(drawer).toBeVisible();
    const drawerBox = await drawer.boundingBox();
    expect(drawerBox, "toc drawer box").not.toBeNull();
    expect(drawerBox!.width).toBeGreaterThan(370);
    await page.keyboard.press("Escape");
    await expect(drawer).toHaveCount(0);

    // The Aa display panel becomes a sheet pinned under the top bar, not a
    // 320px tool-anchored popover.
    await page.getByTestId("reader-aa").click();
    await expect(page.getByTestId("reader-font-increase")).toBeVisible();
    const aaBox = await page.locator(".rd-aa-panel").boundingBox();
    expect(aaBox, "aa panel box").not.toBeNull();
    expect(aaBox!.width).toBeGreaterThan(340);
  });
});
