# Viewing your stats

| | |
|---|---|
| **Runs** | on its own |
| **Owner-only** | no — stats are per-user |
| **Surfaces** | web, iOS |
| **Actions** | `stats.view`, `stats.range`, `stats.scope`, `stats.goal` |

Look at what the app says you have read, and check it against what you know
you did. Stats are derived from every position and session the run has
already written, so this flow is most useful **late in your sequence**, after
a reading or listening flow — the runner tries to order it that way, but do
not refuse it if it comes first; an empty page has its own criteria below.

## Preconditions

None. A reader with no history should see a page that says so honestly rather
than one that errors or spins.

## Steps

1. Open **Stats** from the nav. The window opens on **Month**; under the
   **Library** scope tab the window pills, the clock, the superlatives and the
   "Outside the window" band all disappear — that is the scope, not a band
   vanishing.
2. Read the top of the page: the streak, the daily and annual goals, the
   activity heatmap, the list of books you have open. Ask of each figure
   whether it is *possible* for you — a streak longer than the run, a day on
   the heatmap you never read on, a book in the open list you never touched.
3. Find the **In this window** band and its **Week / Month / Year / Lifetime**
   pills. **Week is a rolling seven days ending today**, and its comparison
   is the seven days before those, cut to the same elapsed length — so a
   delta captioned "vs last week" reaches ten to thirteen days back, not to
   the previous calendar week. Judge the arithmetic on that basis; whether
   the caption should say so is a separate observation. Switch between them and watch **only that band** change — the
   streak, the goals, the heatmap and the open-books list are standing figures
   and must not move when the window does. A tile that changes outside the
   band is a finding.
4. Under **Lifetime**, the per-tile comparisons ("+12% vs last week") are
   meant to be absent, because there is no earlier lifetime to compare
   against. Do not report their absence there.
5. Switch the scope tab to **Library** and read the library figures — size,
   composition by format. Compare the book count against the library page.
6. If you read or listened earlier in this run, find that book and that time
   here. Today's minutes or pages should reflect it, and the book should be in
   the open list unless you finished it. Journal what you did earlier and what
   the page reports.
7. Occasionally, and only on your own account: go to your account page
   (Settings → Account on the web, behind the user menu's Edit link, then
   **Edit goals**; the You tab on iOS), set a daily goal, come back, and
   confirm the goal ring or bar appears with the right target. An unset goal
   shows the bare figure and no ring, which is correct.
8. Open a window tile's drill-in. Each tile opens a fixed sheet — a trend, a
   "vs last …" line, and a Close button. **Escape closes it too** — that was
   fixed by #2491, and this document previously said the opposite, so an agent
   trusting the old text would file a working control. There is
   no chart builder, so do not look for one.

## Journal

`stats.view` with the headline figures as displayed — streak, today's
minutes and pages, open-book count. `stats.range` with the pill you chose and
the tile values before and after. `stats.scope` on a Reading/Library switch.
`stats.goal` with the goal you set; this is account configuration and the
audit does not check it.

## Pass

- The page renders every section without a spinner that never resolves.
- Every figure is possible for your history.
- Switching the window changes the window band only.
- Your own reading from earlier in the run is reflected.
- Another reader's activity never appears in your figures.

## Fail

- A figure that is impossible for you — a streak longer than your history, a
  day you never read on, a book you never opened.
- **Someone else's reading in your stats.** High severity.
- The standing band changes when the window pill changes.
- A section that errors, or a page that spins forever.
- A goal you set is not reflected, or shows the wrong target.

## Sharp edges

- **Day boundaries follow your clock, not the server's.** A session near
  midnight can land on either day depending on the offset the page sent;
  journal the time and the day the page put it on rather than calling it
  wrong.
- Figures are cached briefly. Reload once before calling a stale number a
  defect.
- The "3 wk ago" beside LONGEST SIT is a sparkline axis label, not the stat —
  see [pitfalls.md](../pitfalls.md).
- Sessions recorded without a time zone are left out of the hour-and-weekday
  chart on purpose and reported as a separate "recorded without a timezone"
  figure; a day on the heatmap that is missing from the weekday chart is
  that, not a lost session.
- A runner's phantom position placed ahead of where you read counts as
  ground already covered, so a reading sitting after it can credit zero
  pages. That is the ledger being honest, not a lost sitting.
- Stats on iOS live on the **You** tab, and the window control there is a
  segmented picker rather than pills. The same criteria apply.
