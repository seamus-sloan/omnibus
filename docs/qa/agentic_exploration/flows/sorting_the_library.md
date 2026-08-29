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
3. Change the sort. The axes are **Title**, **Author**, **Series**, **Recently
   Interacted**, **Last Updated**, and **Newest Added** — those six, and no
   others. There is no sort on publication date: **Published** is a table
   column with no control behind it, so do not go looking for one and do not
   report its absence as a defect.

   **The control is not the same in both views, and neither is the reach.** In
   **grid** view there is a **Sort by** select carrying all six axes, with a
   direction toggle beside it. In **table** view that select is not in the DOM
   at all — you sort by clicking a **column header**, and click the same header
   again to reverse. An agent in table view hunting for "Sort by" will correctly
   fail to find it; that is the view, not a missing control.

   Two consequences worth knowing before you report anything. Only five of the
   axes have a sortable header — **Recently Interacted has no column**, so it is
   reachable from the grid alone; do not call it missing from the table. And the
   header for **Newest Added** is labelled just **Added**. Same axis, two names;
   that is not two different sorts disagreeing.
4. **Read the rows and check them yourself.** Do not trust the control's label —
   walk the list and confirm it really is ordered on the field you chose, using
   the values the page is *showing you*.

   **Read the whole list, not the first screenful.** The Author-sort defect
   this step exists to catch does not show up at the top: the first five rows
   look correctly ordered and the break comes further down. So when the head
   looks right and something later looks wrong, the *entire list* is your
   evidence base — the break is only provable by reading every row and testing
   what key would actually produce the order you got.
5. Flip the sort direction and confirm the order reverses.
6. Switch between the **table** and **grid** views and confirm the same books
   appear in the same order in both.
7. Reload, and confirm your chosen sort and view survived.
8. Try at least two different axes in one run. Title and Author are the most
   revealing, because you can verify them by eye.

## Journal

`library.sort` with the old and new sort key and direction, plus **the first
five titles with their authors** under each. That list is the whole evidence
base — a sort finding is unprovable without the before and after. If you are
reporting an order that breaks partway down, journal the rows on either side of
the break as well; five rows from the top do not show it.
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
- Missing values (no series, no interaction yet) have to sort somewhere.
  Consistently first or last is fine; scattered is not.
