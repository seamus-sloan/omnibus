import type { Page } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Re-seed so the running server is indexed against the committed fixtures
// before any assertion runs — independent of other specs in this worker.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// Both CBZ fixtures are reserved for this spec (see fixtures/epubs.ts), and
// the split matters even within it: the suite is fullyParallel, so the
// write-path test (COMIC, which takes the progress AND read-status writes)
// and the read-only tests (PRISTINE, which assert a page-1 open) can run
// concurrently — a shared book would race. Opening PRISTINE still fires the
// pager's automatic unread→reading status write; that's fine because no
// test asserts PRISTINE's status, only that it carries no saved position.
const COMIC = FIXTURE_BOOKS.find((b) => b.slug === "aurora-station-01")!;
const COMIC_PAGES = 8;
const PRISTINE = FIXTURE_BOOKS.find((b) => b.slug === "aurora-station-02")!;
const PRISTINE_PAGES = 5;

// Every page turn saves through the shared progress RPC.
const PROGRESS_POST = {
  method: "POST" as const,
  url: /\/api\/rpc\/progress(?:\?|$)/,
  expectedStatus: 200,
};

// Automatic read-status transitions (open → reading, last page → finished)
// go through the read-status set RPC. The exact-path string cannot match
// the sibling `/api/rpc/read-status/get` fetch the pager issues on open.
const READ_STATUS_POST = {
  method: "POST" as const,
  url: "/api/rpc/read-status/set",
  expectedStatus: 200,
};

async function openComic(page: Page, uuid: string) {
  await gotoReady(page, `/comic/${uuid}`);
  // The pager renders its chrome only after the metadata + saved-position
  // fetch resolves; the footer label is the "ready" signal.
  await expect(page.getByTestId("comic-footer")).toBeVisible();
}

function pageLabel(page: Page) {
  return page.getByTestId("comic-footer").locator(".cr-page-label");
}

test("renders the comic pager layout", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, PRISTINE.title);
  await openComic(page, uuid);

  // Chrome: back, title block, fit-mode toggle group.
  await expect(page.getByTestId("comic-back")).toBeVisible();
  await expect(page.getByRole("button", { name: "Fit width" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Fit height" })).toBeVisible();

  // Stage: the page image is a plain `<img>` against the pages endpoint.
  const img = page.getByTestId("comic-page-image");
  await expect(img).toBeVisible();
  await expect(img).toHaveAttribute("src", `/api/ebooks/${uuid}/pages/0`);
  // The image must actually decode — proves the endpoint served real bytes.
  await expect
    .poll(() => img.evaluate((el) => (el as HTMLImageElement).naturalWidth))
    .toBeGreaterThan(0);

  // Page-turn buttons and the slider; prev is disabled on page 1.
  await expect(
    page.getByRole("button", { name: "Previous page" }),
  ).toBeDisabled();
  await expect(page.getByRole("button", { name: "Next page" })).toBeEnabled();
  await expect(page.getByRole("slider", { name: "Page slider" })).toBeVisible();
  await expect(pageLabel(page)).toHaveText(`Page 1 of ${PRISTINE_PAGES}`);
});

test("fit modes toggle the stage layout class", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, PRISTINE.title);
  await openComic(page, uuid);

  const stage = page.getByTestId("comic-stage");
  // Fit-height is the default.
  await expect(stage).toHaveClass(/cr-fit-height/);
  await expect(
    page.getByRole("button", { name: "Fit height" }),
  ).toHaveAttribute("aria-pressed", "true");

  await page.getByRole("button", { name: "Fit width" }).click();
  await expect(stage).toHaveClass(/cr-fit-width/);
  await expect(page.getByRole("button", { name: "Fit width" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(
    page.getByRole("button", { name: "Fit height" }),
  ).toHaveAttribute("aria-pressed", "false");
});

test("pages forward, saves each turn, resumes, and auto-marks reading then finished", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, COMIC.title);

  // Reset the state a previous run left behind — a run that died mid-test
  // never reaches the tail resets, and the leftovers change this test's
  // behavior: a saved position reopens on the last page (auto-finishing on
  // open instead of marking reading), and a leftover finished status never
  // auto-downgrades. Reset both up front so the transitions are
  // deterministic.
  const progressReset = await request.post("/api/progress", {
    data: {
      book_uuid: uuid,
      format: "epub",
      epub_cfi: "comic-page:0",
      progress_percent: Math.round(100 / COMIC_PAGES),
    },
  });
  expect(progressReset.status()).toBe(200);
  const reset = await request.put("/api/read-status", {
    data: { book_uuid: uuid, status: "unread" },
  });
  expect(reset.status()).toBe(200);

  // Opening an unread comic marks it reading, exactly once. The write fires
  // after hydration + the metadata/status fetches settle, so give it the
  // full-page-load allowance.
  await expectMutation(
    page,
    {
      ...READ_STATUS_POST,
      expectedBody: { update: { book_uuid: uuid, status: "reading" } },
      timeout: 15_000,
    },
    async () => openComic(page, uuid),
  );

  // Three forward turns; each must POST the exact page anchor + percent.
  for (let target = 1; target <= 3; target++) {
    const { request: saved } = await expectMutation(
      page,
      {
        ...PROGRESS_POST,
        expectedBody: {
          update: {
            book_uuid: uuid,
            format: "epub",
            epub_cfi: `comic-page:${target}`,
            progress_percent: Math.round(((target + 1) * 100) / COMIC_PAGES),
          },
        },
      },
      async () => page.getByRole("button", { name: "Next page" }).click(),
    );
    void saved;
    await expect(pageLabel(page)).toHaveText(
      `Page ${target + 1} of ${COMIC_PAGES}`,
    );
  }

  // The server now holds the anchor for page index 3.
  const stored = await request.get(`/api/progress/${uuid}?format=epub`);
  expect(stored.status()).toBe(200);
  const record = (await stored.json()) as {
    epub_cfi: string | null;
    progress_percent: number | null;
  };
  expect(record.epub_cfi).toBe("comic-page:3");
  expect(record.progress_percent).toBe(50);

  // Leave, reopen: the pager restores the saved page, not page 1.
  await gotoReady(page, `/books/${uuid}`);
  await openComic(page, uuid);
  await expect(pageLabel(page)).toHaveText(`Page 4 of ${COMIC_PAGES}`);
  await expect(page.getByTestId("comic-page-image")).toHaveAttribute(
    "src",
    `/api/ebooks/${uuid}/pages/3`,
  );

  // The slider jumps to the last page: the position saves, and landing on
  // the final page auto-marks the book finished.
  await expectMutation(
    page,
    {
      ...READ_STATUS_POST,
      expectedBody: { update: { book_uuid: uuid, status: "finished" } },
    },
    async () =>
      expectMutation(page, PROGRESS_POST, async () =>
        page
          .getByRole("slider", { name: "Page slider" })
          .fill(String(COMIC_PAGES - 1)),
      ),
  );
  await expect(pageLabel(page)).toHaveText(
    `Page ${COMIC_PAGES} of ${COMIC_PAGES}`,
  );
  await expect(page.getByRole("button", { name: "Next page" })).toBeDisabled();

  // Paging back off the last page saves the position but must not
  // downgrade the status: the server still reports finished afterwards.
  await expectMutation(page, PROGRESS_POST, async () =>
    page.getByRole("slider", { name: "Page slider" }).fill("0"),
  );
  const after = await request.get(`/api/read-status/${uuid}`);
  expect(after.status()).toBe(200);
  const statusAfter = (await after.json()) as { status: string } | null;
  expect(statusAfter?.status).toBe("finished");

  // Reset the status for the next run (the page-0 save above already reset
  // the position).
  const cleanup = await request.put("/api/read-status", {
    data: { book_uuid: uuid, status: "unread" },
  });
  expect(cleanup.status()).toBe(200);
});

// PRISTINE stays progress-free here: the route interception answers the
// save before it reaches the server, so no position is ever stored.
test("a failed progress save leaves the pager usable", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, PRISTINE.title);
  await openComic(page, uuid);
  const before = await pageLabel(page).textContent();

  // Force the save to fail; the turn itself must still render.
  await page.route("**/api/rpc/progress", (route) =>
    route.fulfill({ status: 500, body: "boom" }),
  );
  await expectMutation(
    page,
    { ...PROGRESS_POST, expectedStatus: 500 },
    async () => page.getByRole("button", { name: "Next page" }).click(),
  );
  await expect(pageLabel(page)).not.toHaveText(before ?? "");
  await expect(page.getByTestId("comic-page-image")).toBeVisible();
  await page.unroute("**/api/rpc/progress");
});

test("book detail shows the comic Start reading CTA for a CBZ-only book", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, PRISTINE.title);
  await gotoReady(page, `/books/${uuid}`);

  const cta = page.getByTestId("start-reading-comic");
  await expect(cta).toBeVisible();
  await expect(page.getByTestId("no-files-disclaimer")).not.toBeVisible();
  await cta.click();
  await expect(page).toHaveURL(`/comic/${uuid}`);
  await expect(page.getByTestId("comic-footer")).toBeVisible();
});
