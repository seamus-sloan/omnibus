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
  await expect(page.getByTestId("chart-measures")).toBeVisible();
  await expect(page.getByTestId("chart-canvas")).toBeVisible();

  for (const label of ["Group by", "Period", "Split"]) {
    await expect(control(page, label)).toBeVisible();
  }
});

test("opens on the two-measure comparison the builder exists for", async ({
  page,
}) => {
  await gotoReady(page, CHART);

  // The default spec is a count against an average — the case a single pivot
  // query cannot serve, and the reason the page exists.
  await expect(control(page, "Books finished")).toBeChecked();
  await expect(control(page, "Avg book length")).toBeChecked();
  await expect(control(page, "Group by")).toHaveValue("month");
  await expect(control(page, "Period")).toHaveValue("year");
});

test("offers every measure with the unit that decides its compatibility", async ({
  page,
}) => {
  await gotoReady(page, CHART);
  const measures = page.getByTestId("chart-measures");

  for (const name of [
    "Books finished",
    "Avg book length",
    "Avg rating",
    "Reading minutes",
    "Listening minutes",
    "Sittings",
    "Avg sitting length",
    "Pages read",
  ]) {
    await expect(measures.getByLabel(name)).toBeVisible();
  }
  await expect(measures).toContainText("books · per book finished");
  await expect(measures).toContainText("minutes · per sitting");
});

test("narrows the list only once both scales are claimed", async ({ page }) => {
  await gotoReady(page, CHART);

  // Books + pages, so both scales are in use: a third unit is blocked, but a
  // second pages measure still joins the scale pages already own.
  await expect(page.getByTestId("chart-scales")).toContainText(
    "Both scales in use",
  );
  await expect(control(page, "Avg rating")).toBeDisabled();
  await expect(control(page, "Reading minutes")).toBeDisabled();
  await expect(control(page, "Pages read")).toBeEnabled();

  // Freeing the pages scale reopens every unit.
  await control(page, "Avg book length").uncheck();
  await expect(page.getByTestId("chart-scales")).toContainText(
    "anything else can still join",
  );
  await expect(control(page, "Avg rating")).toBeEnabled();
  await expect(control(page, "Reading minutes")).toBeEnabled();
});

test("plots more measures than there are scales", async ({ page }) => {
  await gotoReady(page, CHART);

  // A third measure sharing an existing unit is allowed — the bound is on
  // scales, not on how many measures use them.
  await control(page, "Pages read").check();
  await expect(control(page, "Pages read")).toBeChecked();

  await expect
    .poll(async () => page.getByTestId("chart-legend").count())
    .toBeGreaterThan(0);
  const legend = page.getByTestId("chart-legend");
  await expect(legend).toContainText("Books finished");
  await expect(legend).toContainText("Avg book length");
  await expect(legend).toContainText("Pages read");
});

test("never lets the last measure be removed", async ({ page }) => {
  await gotoReady(page, CHART);

  await control(page, "Avg book length").uncheck();
  // One left, so it is held checked — an empty chart is not a reachable state.
  await expect(control(page, "Books finished")).toBeChecked();
  await expect(control(page, "Books finished")).toBeDisabled();
});

test("offers a split only for a single per-book measure", async ({ page }) => {
  await gotoReady(page, CHART);
  const split = control(page, "Split");

  // Two measures, so no single population for a split to describe.
  await expect(split).toBeDisabled();
  await expect(page.getByTestId("chart-controls")).toContainText(
    "one measure only",
  );

  await control(page, "Avg book length").uncheck();
  await expect(split).toBeEnabled();

  // A sitting cannot carry a genre — a sitting may cover several books, and a
  // book several genres, so splitting one would double-count its minutes.
  await control(page, "Books finished").check();
  await control(page, "Reading minutes").check();
  await control(page, "Books finished").uncheck();
  await expect(split).toBeDisabled();
  await expect(page.getByTestId("chart-controls")).toContainText(
    "only per-book measures split",
  );
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

  // Add before removing — the last remaining measure is held checked, so the
  // selection can never pass through empty.
  await control(page, "Pages read").check();
  await control(page, "Books finished").uncheck();
  await control(page, "Avg book length").uncheck();
  await settled();
  await expect(canvas).toBeVisible();

  // A measure with a bounded coverage window states that in the notes.
  await expect(control(page, "Pages read")).toBeChecked();
  await page
    .getByTestId("chart-notes")
    .getByText("What this chart shows")
    .click();
  await expect(page.getByTestId("chart-caveat").first()).toContainText(
    "progress ledger",
  );
});

test("reads every series at the hovered bucket", async ({ page }) => {
  await gotoReady(page, CHART);
  await expect
    .poll(async () => page.getByTestId("chart-legend").count())
    .toBeGreaterThan(0);

  // No hover state in the server-rendered markup — it is client-only, so the
  // first paint has to match it (rule 07).
  await expect(page.getByTestId("chart-tooltip")).toHaveCount(0);

  const bands = page.locator(".cb-hit");
  await expect(bands.first()).toBeAttached();
  await bands.nth(2).hover({ force: true });

  const tip = page.getByTestId("chart-tooltip");
  await expect(tip).toBeVisible();
  // Both series at once, each with its own unit, since they may be on
  // different scales.
  await expect(tip).toContainText("Books finished");
  await expect(tip).toContainText("books");
  await expect(tip).toContainText("Avg book length");
  await expect(tip).toContainText("pages");
});

test("stacks a split count into one column per bucket", async ({ page }) => {
  await gotoReady(page, CHART);
  await control(page, "Avg book length").uncheck();
  await control(page, "Split").selectOption("genre");

  await expect
    .poll(async () => page.getByTestId("chart-legend").count())
    .toBeGreaterThan(0);

  // Stacked bars share one lane, so every bar in a bucket has the same x.
  const xs = await page
    .locator("rect.cb-bar")
    .evaluateAll((nodes) => nodes.map((n) => n.getAttribute("x")));
  const labels = await page.locator("text.cb-xlabel").count();
  expect(xs.length).toBeGreaterThan(labels);
  expect(new Set(xs).size).toBeLessThanOrEqual(labels);
});

test("explains what the chart shows and why the list is narrowed", async ({
  page,
}) => {
  await gotoReady(page, CHART);
  const notes = page.getByTestId("chart-notes");
  await expect(notes).toBeVisible();

  // Closed by default, so it never pushes the chart off screen.
  await expect(page.getByTestId("chart-notes-scales")).toBeHidden();
  await notes.getByText("What this chart shows").click();

  // Each measure says what it counts, at which grain, in which unit.
  await expect(notes).toContainText("Books you marked finished");
  await expect(notes).toContainText("Measured per book finished, in books.");

  // Two units on screen, so the notes warn that a crossing means nothing and
  // say which units are holding the scales.
  await expect(page.getByTestId("chart-notes-scales")).toContainText(
    "means nothing",
  );
  await expect(page.getByTestId("chart-notes-availability")).toContainText(
    "books and pages",
  );
});

test("rewrites the notes when the selection changes", async ({ page }) => {
  await gotoReady(page, CHART);
  await page
    .getByTestId("chart-notes")
    .getByText("What this chart shows")
    .click();

  // Down to one unit: a free scale, and nothing to misread across axes.
  await control(page, "Avg book length").uncheck();
  await expect(page.getByTestId("chart-notes-availability")).toContainText(
    "still free",
  );
  await expect(page.getByTestId("chart-notes-scales")).not.toContainText(
    "means nothing",
  );

  // A measure with a bounded window states that under its own heading.
  await control(page, "Pages read").check();
  await control(page, "Books finished").uncheck();
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
