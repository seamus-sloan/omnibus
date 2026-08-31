import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectNavVisible, gotoReady } from "../utils/nav";

// The chart builder is a standalone surface while `/stats` is being
// redesigned, so nothing in the UI links to it yet and this spec reaches it by
// URL. That is a deliberate departure from the navigation rule in
// `.claude/rules/04-playwright.md` — recorded here rather than left implicit,
// and it lapses the moment the builder gets its entry point on `/stats`.
const CHART = "/stats/chart";

// Every assertion below is about the picker's own contract, never about
// plotted values. Reading history is global shared state — the reader and
// read-status specs write it — so a spec that asserted a bar's height would be
// asserting whatever the rest of the suite happened to have done first.

// Control lookups are scoped to the picker: the plot's own accessible name
// ends "over N periods", which an unscoped `getByLabel("Period")` matches
// alongside the select.
const control = (page: Page, label: string) =>
  page.getByTestId("chart-controls").getByLabel(label);

test("renders the chart builder layout", async ({ page }) => {
  await gotoReady(page, CHART);

  await expectNavVisible(page);
  await expect(
    page.getByRole("heading", { name: "Chart builder" }),
  ).toBeVisible();
  await expect(page.getByTestId("chart-controls")).toBeVisible();
  await expect(page.getByTestId("chart-canvas")).toBeVisible();

  for (const label of [
    "Measure",
    "Compare with",
    "Group by",
    "Period",
    "Split",
  ]) {
    await expect(control(page, label)).toBeVisible();
  }
});

test("opens on the two-measure comparison the builder exists for", async ({
  page,
}) => {
  await gotoReady(page, CHART);

  // The default spec is a count against an average — the case a single pivot
  // query cannot serve, and the reason the page exists.
  await expect(control(page, "Measure")).toHaveValue("books_finished");
  await expect(control(page, "Compare with")).toHaveValue("avg_page_length");
  await expect(control(page, "Group by")).toHaveValue("month");
  await expect(control(page, "Period")).toHaveValue("year");
});

test("naming a measure reports the grain it is measured at", async ({
  page,
}) => {
  await gotoReady(page, CHART);
  const controls = page.getByTestId("chart-controls");

  await expect(controls).toContainText("measured per book finished");

  // A sitting-grain measure reports a different grain — the fact that lets a
  // reader see why two measures can share a bucket but not a query.
  await control(page, "Measure").selectOption("reading_minutes");
  await expect(controls).toContainText("measured per sitting");

  await control(page, "Measure").selectOption("pages_read");
  await expect(controls).toContainText("measured per day read");
});

test("offers a split only for a single per-book measure", async ({ page }) => {
  await gotoReady(page, CHART);
  const split = control(page, "Split");

  // Two measures need both axes, so there is no room for a split.
  await expect(split).toBeDisabled();
  await expect(page.getByTestId("chart-controls")).toContainText(
    "drop the comparison to split",
  );

  await control(page, "Compare with").selectOption("__none");
  await expect(split).toBeEnabled();

  // A sitting cannot carry a genre — a sitting may cover several books, and a
  // book several genres, so splitting one would double-count its minutes.
  await control(page, "Measure").selectOption("reading_minutes");
  await expect(split).toBeDisabled();
  await expect(page.getByTestId("chart-controls")).toContainText(
    "only per-book measures split",
  );
});

test("never offers the primary measure as its own comparison", async ({
  page,
}) => {
  await gotoReady(page, CHART);

  const options = control(page, "Compare with").locator("option");
  await expect(options.filter({ hasText: "Books finished" })).toHaveCount(0);

  // Swapping the primary frees the one it displaced.
  await control(page, "Measure").selectOption("avg_rating");
  await expect(options.filter({ hasText: "Books finished" })).toHaveCount(1);
  await expect(options.filter({ hasText: "Avg rating" })).toHaveCount(0);
});

test("redraws when the selection changes", async ({ page }) => {
  await gotoReady(page, CHART);
  const canvas = page.getByTestId("chart-canvas");

  // Either a chart or the empty state, depending on what reading history the
  // rest of the suite has produced — but the canvas always resolves to one of
  // them rather than hanging.
  const settled = async () =>
    expect
      .poll(async () =>
        (await page.getByTestId("chart-empty").count()) > 0
          ? "empty"
          : (await page.getByTestId("chart-legend").count()) > 0
            ? "chart"
            : "pending",
      )
      .not.toBe("pending");

  await settled();
  await control(page, "Measure").selectOption("session_count");
  await settled();
  await expect(canvas).toBeVisible();

  // A measure with a bounded coverage window states that under the chart.
  await control(page, "Measure").selectOption("pages_read");
  await settled();
  await expect(page.getByTestId("chart-caveat").first()).toContainText(
    "progress ledger",
  );
});

test("surfaces a rejected spec instead of failing silently", async ({
  page,
}) => {
  // The server re-validates every spec, so force its rejection path and assert
  // the page reports it rather than sitting on a stale chart.
  await page.route("**/api/rpc/chart-series", (route) =>
    route.fulfill({
      status: 500,
      contentType: "application/json",
      body: JSON.stringify({ error: "pick at least one measure" }),
    }),
  );

  await gotoReady(page, CHART);
  await expect(page.getByRole("alert")).toBeVisible();
});
