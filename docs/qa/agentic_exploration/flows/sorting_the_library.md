# Sorting the library

| | |
|---|---|
| **Weight** | 8% |
| **Owner-only** | no |
| **Surfaces** | web, iOS |
| **Actions** | `library.sort`, `library.view` |

Rearrange the library and check that the order you asked for is the order you
got. Sorting writes nothing, so this flow is cheap — and it is one of the
easiest places for a defect to hide, because a control can *look* applied while
the rows underneath disagree.

## Preconditions

A library with at least three or four books, so an ordering is meaningful. If
there are fewer, journal that and end the flow `uncertain`.

## Steps

1. Go to the library from the nav.
2. Note the current sort and write down the first several titles **and** their
   authors, exactly as displayed.
3. Change the sort with the **Sort by** control. The axes are **Title**,
   **Author**, **Added**, **Published**, and **Recently Interacted**.
4. **Read the rows and check them yourself.** Do not trust the control's label —
   walk down the visible list and confirm it really is ordered on the field you
   chose, using the values the page is *showing you*.
5. Flip the sort direction and confirm the order reverses.
6. Switch between the **table** and **grid** views and confirm the same books
   appear in the same order in both.
7. Reload, and confirm your chosen sort and view survived.
8. Try at least two different axes in one run. Title and Author are the most
   revealing, because you can verify them by eye.

## Journal

`library.sort` with the old and new sort key and direction, plus **the first
five titles with their authors** under each. That list is the whole evidence
base — a sort finding is unprovable without the before and after.
`library.view` on a table/grid switch.

## Pass

- The visible order matches the chosen axis, judged on displayed values.
- Reversing direction reverses the order.
- Table and grid agree.
- The choice survives a reload.
- The book count does not change when only the order does.

## Fail

- The order does not match the axis — **including when it is sorted on
  something the page is not showing you**, such as a title or author that was
  corrected after import. Displayed value and sort key must agree.
- Sorting drops, duplicates, or reorders books inconsistently between views.
- The chosen sort silently reverts on reload.

## Sharp edges

- **Recently Interacted moves as other agents use the library.** Two agents
  seeing different orders under it is expected; do not report it.
- Books added by others mid-flow will change counts underneath you. Re-read the
  count rather than trusting one from a minute ago.
- Missing values (no publication date, no series) have to sort somewhere.
  Consistently first or last is fine; scattered is not.
