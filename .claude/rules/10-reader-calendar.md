# 10 — Which calendar a figure is cut on

Two questions look alike and take different answers. Getting them the wrong way
round produces stats that are individually plausible and collectively
incoherent, which is the failure this rule exists to prevent.

| | Question | Source | Where |
|---|---|---|---|
| **Day boundary** | *which day is this on?* | the **asking client's** offset, one per request | `db::user_offset`, `db::stats::calendar` |
| **Time of day** | *what did the clock read?* | the offset **that session recorded** at capture | `db::stats::patterns` |

## Day boundaries: one calendar per request

A day is **ordinal** — today, yesterday, seven in a row — and a sequence whose
elements are measured on different calendars cannot be ordered. So every figure
with a day boundary resolves against exactly one offset:
`user_offset::resolve_offset_minutes`, which prefers what the client declared,
falls back to the reader's most recent session offset, then to UTC.

That covers the heatmap, the streak and active days, `listening_daily`,
`busiest_week`, both daily goals, the annual goal's year bounds, `as_of`,
`books_per_month`, the chart builder's buckets **and its axis**, and where the
Week/Month/Year window itself opens.

- **Never hand-write the shift.** Compose from `stats::calendar` —
  `local_day`, `local_day_number`, `local_month`, `window_start_expr`,
  `prev_window_start_expr`, `today_expr`. A second `date(started_at + …)` is how
  the heatmap and the streak come to disagree about a reader's Tuesday.
- **A spec resolves the offset once.** Every measure, breakdown and axis in one
  response shares it, or a series and the axis it is drawn against disagree
  about which bucket a day belongs to.
- **The cache key carries the offset.** `stats`'s aggregate cache is keyed
  `(user_id, range, offset_minutes)`. Omitting it serves a reader in Tokyo the
  days of a reader in Los Angeles.
- **The server still owns the arithmetic.** The client contributes only *where
  it is*. A client that derived its own bucketing is what makes the web page,
  the iOS tab and a widget disagree about one streak.

## Time of day: the offset the session recorded

`patterns` buckets by hour and weekday against `utc_offset_minutes` (migration
`0080`), so an evening read in Tokyo stays an evening after the reader flies
home. A distribution over hours needs no ordering, which is why it can stay
anchored to where the reading happened when the day boundaries cannot.

Rows with **no** recorded offset are excluded there, not defaulted, and come
back as `unzoned_seconds`. That exclusion is specific to the hour buckets: a day
boundary is a property of where the reader is *now*, so `started_at` alone
places any row against it and nothing is unplaceable.

## The tradeoff, and why it is the right one

One calendar means a reader who changes zones **re-dates their history**. A run
can shorten where two days merge onto one, or break where one splits in two.

The alternative — anchoring each day to the zone it was read in — was tried and
rejected. It keeps the past exact and makes the *present* incoherent: read at
08:00 in Tokyo, fly to Los Angeles, and that morning's reading sits on a day
that has not happened yet where you now are, counting toward neither today's
goal nor your streak, for the whole trip. It also splits one sitting across two
days, since completion events carry no capture offset to anchor with.

You cannot have both. Protect the present.

## The ledger decides no calendar

`reading_progress_slots` (migration `0093`) keys forward progress on the
**quarter-hour** it was observed in, and the day is resolved on the way out.

- **Never store a day.** Migration `0083` did, and a day string is the one thing
  that cannot be re-bucketed — it is what made the daily pages goal reset at
  UTC midnight.
- **Quarter-hour, not hourly.** UTC+05:30, UTC+05:45 and UTC−03:30 are real
  zones an hourly grid cannot place. Not the raw second either: that is one row
  per page turn, where a quarter-hour is bounded at 96 per reader-book-day.
- **`reading_progress_daily` is frozen, not migrated.** Its rows kept no
  instant, so assigning one would silently re-date history that was never
  re-datable. `pages::ledger_days` unions both generations; the old contributes
  its stored day verbatim.
- A new counter table keyed on a bucket joins `RETARGET_TABLES` **and**
  `fold_ledger_counters` — summed on a merge collision, never latest-wins, or a
  reader who covered ground in both editions loses one side. See
  [06-migrations.md](06-migrations.md).

## Out of scope

- What a client may queue offline — [08-offline-writes.md](08-offline-writes.md).
- Rendering a stored instant as a date in the viewer's zone (journal and
  highlight stamps) is a client concern, not this.
