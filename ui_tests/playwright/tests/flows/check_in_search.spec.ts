import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { gotoReady } from "../utils/nav";

// Title search rides on provider RPCs, so every network edge is mocked:
// search (candidate list) and resolve-meta (the pick). Real provider behavior
// is covered by the `omnibus_db` wiremock suites. The search form lives on the
// lookup front door — its layout is asserted in `check_in_lookup.spec.ts`.

function candidate(
  over: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    isbn13: "9780756404741",
    title: "The Name of the Wind",
    authors: ["Patrick Rothfuss"],
    year: "2008",
    pages: 662,
    publisher: "DAW Books",
    description: null,
    cover_url: null,
    series: "The Kingkiller Chronicle",
    first_publish_year: 2007,
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

/// The search form is on the flow's front door, so reaching it is just
/// opening check-in — no ISBN has to fail first.
async function reachSearch(page: Page): Promise<void> {
  await gotoReady(page, "/check-in");
  await expect(page.getByTestId("check-in-search")).toBeVisible();
}

test.describe("check-in title search", () => {
  test("searches by title and picks a candidate into the chooser", async ({
    page,
  }) => {
    await reachSearch(page);
    await mockJsonPost(page, "**/api/rpc/scan/search", {
      results: [
        candidate(),
        candidate({
          isbn13: "9780756405892",
          title: "The Wise Man's Fear",
          series: null,
          first_publish_year: 2011,
        }),
      ],
    });

    await page.getByLabel("Title").fill("name of the wind");
    await expectMutation(
      page,
      {
        method: "POST",
        url: "/api/rpc/scan/search",
        expectedBody: { req: { query: "name of the wind" } },
        expectedStatus: 200,
      },
      async () => page.getByTestId("check-in-search-submit").click(),
    );

    const rows = page.getByTestId("check-in-search-result");
    await expect(rows).toHaveCount(2);
    // The provider facts the scan asked for: series + first-publish year.
    await expect(rows.nth(0)).toContainText("The Kingkiller Chronicle");
    await expect(rows.nth(0)).toContainText("First published 2007");
    await expect(rows.nth(0)).toContainText("DAW Books");
    await expect(rows.nth(0)).toContainText("662 pages");

    await mockJsonPost(page, /\/api\/rpc\/scan\/resolve-meta$/, {
      kind: "not_in_library",
      online: candidate(),
    });
    await expectMutation(
      page,
      {
        method: "POST",
        url: /\/api\/rpc\/scan\/resolve-meta$/,
        expectedStatus: 200,
      },
      async () => rows.nth(0).click(),
    );

    // The pick lands on the normal 3c chooser, richer card included.
    await expect(page.getByTestId("check-in-choose")).toBeVisible();
    await expect(page.getByTestId("check-in-book-series")).toHaveText(
      "The Kingkiller Chronicle",
    );
    await expect(page.getByTestId("check-in-book-detail")).toContainText(
      "First published 2007",
    );
    await expect(page.getByTestId("check-in-own-it")).toBeVisible();
    await expect(page.getByTestId("check-in-wishlist")).toBeVisible();
  });

  test("shows the empty-state line when nothing matches", async ({ page }) => {
    await reachSearch(page);
    await mockJsonPost(page, "**/api/rpc/scan/search", { results: [] });

    await page.getByLabel("Title").fill("no such book anywhere");
    await expectMutation(
      page,
      { method: "POST", url: "/api/rpc/scan/search", expectedStatus: 200 },
      async () => page.getByTestId("check-in-search-submit").click(),
    );

    await expect(page.getByTestId("check-in-search-empty")).toBeVisible();
    await expect(page.getByTestId("check-in-search-result")).toHaveCount(0);
  });

  test("surfaces a search failure and stays on the search screen", async ({
    page,
  }) => {
    await reachSearch(page);
    await page.route("**/api/rpc/scan/search", (route) =>
      route.request().method() === "POST"
        ? route.fulfill({
            status: 500,
            contentType: "text/plain",
            body: "forced failure",
          })
        : route.continue(),
    );

    await page.getByLabel("Title").fill("name of the wind");
    await expectMutation(
      page,
      { method: "POST", url: "/api/rpc/scan/search", expectedStatus: 500 },
      async () => page.getByTestId("check-in-search-submit").click(),
    );

    await expect(page.getByTestId("check-in-error")).toBeVisible();
    await expect(page.getByTestId("check-in-search")).toBeVisible();
  });

  test("results survive alongside the ISBN field on the same screen", async ({
    page,
  }) => {
    await reachSearch(page);
    await mockJsonPost(page, "**/api/rpc/scan/search", {
      results: [candidate()],
    });

    await page.getByLabel("Title").fill("name of the wind");
    await expectMutation(
      page,
      { method: "POST", url: "/api/rpc/scan/search", expectedStatus: 200 },
      async () => page.getByTestId("check-in-search-submit").click(),
    );

    // Searching by title never costs the reader the ISBN field — both ways in
    // stay on screen together.
    await expect(page.getByTestId("check-in-search-result")).toHaveCount(1);
    await expect(page.getByTestId("check-in-isbn")).toBeVisible();
    await expect(page.getByTestId("check-in-scan-instead")).toBeVisible();
  });
});
