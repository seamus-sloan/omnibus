---
name: create-github-issue
description: Recipe for filing GitHub issues in the omnibus house style — bracketed `[Scope]` title prefixes, a standard body template (Description / Implementation / Affected Crates / Acceptance Criteria / Attachments), consistent labels, and native sub-issue linking for large features. Triggers when the user asks to create an issue, file a ticket, break a feature into sub-tasks, or standardize an issue's description.
---

# Create a GitHub issue (omnibus house style)

Recipe for filing issues in `seamus-sloan/omnibus` so they share a consistent
shape: a bracketed title prefix, a standard body template, consistent labels,
and — when a feature is large — native sub-issue linking.

## Prerequisites

- An authenticated `gh` CLI with write access to `seamus-sloan/omnibus`.

## Issue shape

Most issues are **standalone** — one self-contained piece of work. Reach for the
epic/sub-issue split only when a feature is genuinely too big for a single
issue:

- **Epic** — a large feature broken into sub-issues; GitHub renders a progress
  bar that ticks up as the children close.
- **Sub-issue** — a child of an epic.

Not every issue needs to be an epic or a sub-issue.

## Title convention — bracket the scope

Every title starts with a `[Scope]` prefix so you can tell what an issue belongs
to from the name alone.

| Issue kind | Prefix | Example |
|---|---|---|
| Standalone / scoped | `[<Feature Area>]` | `[Metadata Edit] Per-Library Metadata Precedence Setting` |
| Sub-issue of an epic | `[<Epic Name>]` | `[Admin Panel] Device & Session List / Revoke Endpoints` |
| Tech debt | `[Tech Debt] <Low\|Medium\|High> — …` | `[Tech Debt] Low — plaintext token on disk` |

- Title-case the feature name.
- A sub-issue's bracket names its **parent epic**.

## Labels

- `enhancement` — new feature work.
- `bug` — a defect.
- `security` — touches auth/sessions/permissions/untrusted input.
- `mobile` — mobile-crate work.
- `tech-debt` — the `[Tech Debt] …` issues.

Apply exactly one type label (`enhancement` or `bug`), plus any others that fit.

## Body template (required format)

Every issue body uses this structure:

```
## Description
{2-4 sentences summarizing the issue}

## Implementation
{a short paragraph or 3-6 bullets on what the work involves — name modules/endpoints/tables}

## Affected Crates
- [x] `db`
- [ ] `frontend`
- [ ] `mobile`
- [ ] `server`
- [ ] `shared`

## Acceptance Criteria
- AC1: {verifiable done-condition}
- AC2: ...

## Attachments
{anything else — a screenshot, a linked doc, "Part of #<epic>" — or omit if nothing}
```

- **Affected Crates**: list all five in order, checking `[x]` the ones the work
  touches. An epic's set is the union of its subs'.
- Reference modules/features by name and link **source files**
  (`db/src/auth.rs`) rather than paths that may move.

## Creating an issue

```bash
gh issue create -R seamus-sloan/omnibus \
  --title "[Metadata Edit] Per-Library Metadata Precedence Setting" \
  --label enhancement \
  --body "$BODY"
```

## Epics and sub-issues (large features only)

Sub-issues are GitHub's epic/story equivalent. Create the parent and children as
normal issues, then link each child to the parent. The API wants the child's
**internal id** (`.id`), not its number:

```bash
child_id=$(gh api repos/seamus-sloan/omnibus/issues/909 --jq '.id')
gh api --method POST repos/seamus-sloan/omnibus/issues/908/sub_issues \
  -F sub_issue_id=$child_id
```

Verify the parent's progress rollup:

```bash
gh api graphql -f query='
query { repository(owner:"seamus-sloan", name:"omnibus") {
  issue(number:908) { subIssuesSummary { total completed percentCompleted } } } }'
```

## Batch creation

For many issues at once, don't hand-run `gh` per issue: put the plan in a JSON
file and drive it with a loop that creates each issue (and links any sub-issues
to their parent). Pace creates (~1s sleep) to dodge GitHub's secondary rate
limit, and log each action so a partial run is auditable and re-runnable.
