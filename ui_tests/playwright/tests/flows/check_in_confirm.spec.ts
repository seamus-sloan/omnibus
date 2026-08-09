import type { Page } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// The terminal branches reached once a resolved ISBN lands past the lookup
// screen: checking in a copy of a book the library already holds (3a), the
// fuzzy (title, author) close-match confirm (2b), and the two "already
// tracked" outcomes that skip the wizard and navigate straight to the book's
// detail page. Every provider RPC is mocked — same rationale as
// `check_in_search.spec.ts` — and here the check-in write itself is mocked
// too: the target book (`standalone-island`) is read by
// `physical_collection.spec.ts` elsewhere in the suite, so a real physical
// copy written against it here would leak a copy that spec never expects.
// The real check-in round trip (a book this spec never touches, created and
// then deleted) lives in `check_in_choose.spec.ts`.

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

function candidate(
  over: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    isbn13: ISBN,
    title: "A Fuzzy-Matched Title",
    authors: ["Some Author"],
    year: "2011",
    pages: null,
    publisher: null,
    description: null,
    cover_url: null,
    series: null,
    first_publish_year: null,
    source: "open_library",
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

async function reachConfirm(page: Page, uuid: string): Promise<void> {
  await mockJsonPost(page, /\/api\/rpc\/scan\/resolve$/, {
    kind: "in_library_unowned",
    book: scanBook(uuid),
  });
  await gotoReady(page, "/check-in");
  await page.getByTestId("check-in-isbn").fill(ISBN);
  await expectMutation(
    page,
    { method: "POST", url: /\/api\/rpc\/scan\/resolve$/, expectedStatus: 200 },
    async () => page.getByTestId("check-in-submit").click(),
  );
  await expect(page.getByTestId("check-in-confirm")).toBeVisible();
}

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

test.describe("check-in confirm and close-match", () => {
  test("checks in a copy of a seeded digital book after an exact ISBN match", async ({
    page,
    request,
  }) => {
    const uuid = await fetchBookUuidByTitle(request, TARGET.title);
    await reachConfirm(page, uuid);

    await expect(page.getByTestId("check-in-book")).toContainText(TARGET.title);
    await expect(page.getByText(`Scanned ISBN ${ISBN}`)).toBeVisible();

    await page
      .getByTestId("check-in-note")
      .fill("Trade paperback, 2nd printing");
    await mockJsonPost(page, "**/api/rpc/scan/check-in", {
      book_uuid: uuid,
    });
    await expectMutation(
      page,
      {
        method: "POST",
        url: "/api/rpc/scan/check-in",
        expectedBody: {
          req: {
            book_uuid: uuid,
            isbn: ISBN,
            note: "Trade paperback, 2nd printing",
          },
        },
        expectedStatus: 200,
      },
      async () => page.getByTestId("check-in-confirm-submit").click(),
    );

    await expect(page.getByTestId("check-in-success")).toContainText(
      "In your physical collection",
    );
    await expect(page.getByTestId("check-in-success")).toContainText(
      TARGET.title,
    );
    await expect(page.getByTestId("check-in-view-book")).toBeVisible();
  });

  test("cancels a check-in confirmation and returns to the lookup front door", async ({
    page,
  }) => {
    await reachConfirm(page, "island-uuid");

    await page.getByTestId("check-in-cancel").click();

    await expect(page.getByTestId("check-in-lookup")).toBeVisible();
    await expect(page.getByTestId("check-in-isbn")).toHaveValue("");
  });

  test("surfaces a check-in failure and stays on the confirm screen", async ({
    page,
  }) => {
    await reachConfirm(page, "island-uuid");

    await page.route("**/api/rpc/scan/check-in", (route) =>
      route.request().method() === "POST"
        ? route.fulfill({
            status: 500,
            contentType: "text/plain",
            body: "forced failure",
          })
        : route.continue(),
    );
    await expectMutation(
      page,
      {
        method: "POST",
        url: "/api/rpc/scan/check-in",
        expectedStatus: 500,
      },
      async () => page.getByTestId("check-in-confirm-submit").click(),
    );

    await expect(page.getByTestId("check-in-error")).toBeVisible();
    await expect(page.getByTestId("check-in-confirm")).toBeVisible();
  });

  test("confirms a close-match hit and lands on the same check-in confirm", async ({
    page,
  }) => {
    const scanned = candidate();
    await mockJsonPost(page, /\/api\/rpc\/scan\/resolve$/, {
      kind: "close_match",
      book: scanBook("island-uuid", { isbn: "9780000000000" }),
      scanned,
    });
    await gotoReady(page, "/check-in");
    await page.getByTestId("check-in-isbn").fill(ISBN);
    await expectMutation(
      page,
      {
        method: "POST",
        url: /\/api\/rpc\/scan\/resolve$/,
        expectedStatus: 200,
      },
      async () => page.getByTestId("check-in-submit").click(),
    );

    await expect(page.getByTestId("check-in-close-match")).toBeVisible();
    await expect(
      page.getByRole("heading", { level: 1, name: "Is this the book?" }),
    ).toBeVisible();
    await expect(
      page.getByText(`Scanned ISBN ${scanned.isbn13}`),
    ).toBeVisible();
    await expect(
      page.getByText("Library edition ISBN 9780000000000"),
    ).toBeVisible();

    await page.getByTestId("check-in-close-match-pick").click();

    await expect(page.getByTestId("check-in-confirm")).toBeVisible();
    await expect(
      page.getByText(`Scanned ISBN ${scanned.isbn13}`),
    ).toBeVisible();
  });

  // A book's EPUB and its unattached audiobook are two rows carrying the same
  // effective (title, author): the ladder offers both rather than declining
  // into a duplicate physical-only book (#1791).
  test("picks between two library rows that both matched the scan", async ({
    page,
  }) => {
    const scanned = candidate();
    await mockJsonPost(page, /\/api\/rpc\/scan\/resolve$/, {
      kind: "close_match",
      book: scanBook("epub-uuid", { isbn: "9780000000000" }),
      others: [
        scanBook("audiobook-uuid", {
          title: `${TARGET.title} (Unabridged)`,
          isbn: null,
        }),
      ],
      scanned,
    });
    await gotoReady(page, "/check-in");
    await page.getByTestId("check-in-isbn").fill(ISBN);
    await expectMutation(
      page,
      {
        method: "POST",
        url: /\/api\/rpc\/scan\/resolve$/,
        expectedStatus: 200,
      },
      async () => page.getByTestId("check-in-submit").click(),
    );

    await expect(
      page.getByRole("heading", { level: 1, name: "Which one is this?" }),
    ).toBeVisible();
    await expect(page.getByTestId("check-in-close-match-pick")).toHaveCount(2);
    await expect(page.getByTestId("check-in-book")).toHaveCount(2);

    // The second candidate is the one only a picker can reach.
    await page.getByTestId("check-in-close-match-pick").nth(1).click();

    await expect(page.getByTestId("check-in-confirm")).toBeVisible();
    await expect(page.getByTestId("check-in-book")).toContainText(
      `${TARGET.title} (Unabridged)`,
    );
  });

  test("declines a close match and falls through to the online chooser", async ({
    page,
  }) => {
    const scanned = candidate({ title: "Definitely Not That Book" });
    await mockJsonPost(page, /\/api\/rpc\/scan\/resolve$/, {
      kind: "close_match",
      book: scanBook("island-uuid"),
      scanned,
    });
    await gotoReady(page, "/check-in");
    await page.getByTestId("check-in-isbn").fill(ISBN);
    await expectMutation(
      page,
      {
        method: "POST",
        url: /\/api\/rpc\/scan\/resolve$/,
        expectedStatus: 200,
      },
      async () => page.getByTestId("check-in-submit").click(),
    );
    await expect(page.getByTestId("check-in-close-match")).toBeVisible();

    await page.getByTestId("check-in-close-match-no").click();

    await expect(page.getByTestId("check-in-choose")).toBeVisible();
    await expect(page.getByTestId("check-in-book")).toContainText(
      "Definitely Not That Book",
    );
  });

  test("an already-owned scan navigates straight to the book's detail page", async ({
    page,
    request,
  }) => {
    const uuid = await fetchBookUuidByTitle(request, TARGET.title);
    await mockJsonPost(page, /\/api\/rpc\/scan\/resolve$/, {
      kind: "already_owned",
      book: scanBook(uuid, { has_physical: true }),
    });
    await gotoReady(page, "/check-in");
    await page.getByTestId("check-in-isbn").fill(ISBN);
    await expectMutation(
      page,
      {
        method: "POST",
        url: /\/api\/rpc\/scan\/resolve$/,
        expectedStatus: 200,
      },
      async () => page.getByTestId("check-in-submit").click(),
    );

    await expect(page).toHaveURL(new RegExp(`/books/${uuid}$`));
    await expect(
      page.getByRole("heading", { level: 1, name: TARGET.title }),
    ).toBeVisible();
    await expect(page.getByTestId("check-in")).toHaveCount(0);
  });

  test("an on-wishlist scan navigates straight to the book's detail page", async ({
    page,
    request,
  }) => {
    const uuid = await fetchBookUuidByTitle(request, TARGET.title);
    await mockJsonPost(page, /\/api\/rpc\/scan\/resolve$/, {
      kind: "on_wishlist",
      book: scanBook(uuid),
    });
    await gotoReady(page, "/check-in");
    await page.getByTestId("check-in-isbn").fill(ISBN);
    await expectMutation(
      page,
      {
        method: "POST",
        url: /\/api\/rpc\/scan\/resolve$/,
        expectedStatus: 200,
      },
      async () => page.getByTestId("check-in-submit").click(),
    );

    await expect(page).toHaveURL(new RegExp(`/books/${uuid}$`));
    await expect(
      page.getByRole("heading", { level: 1, name: TARGET.title }),
    ).toBeVisible();
  });
});
