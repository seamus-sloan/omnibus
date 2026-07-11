---
name: create-github-issue
description: Recipe for creating GitHub issues for omnibus in the house style — bracketed `[Scope]` title prefixes, roadmap epics with native sub-issues, the `roadmap` label, and placement on Project #2 with a Priority field. Triggers when the user asks to create an issue, file a ticket, turn a roadmap doc into issues, break a feature into sub-tasks, or migrate `docs/roadmap/` into GitHub.
---

# Create a GitHub issue (omnibus house style)

Canonical recipe for filing issues in `seamus-sloan/omnibus` so they match the
team's Jira-like structure: bracketed title prefixes, epic → sub-issue nesting,
labels, and the Project board with its Priority field.

> **Status: work in progress.** Built up interactively; refine as conventions
> settle.

## Prerequisites

- `gh` CLI authenticated as `seamus-sloan`.
- Token needs the **`project`** scope for any board/field work (issue creation
  only needs `repo`). Add it with `gh auth refresh -s project`, verify with
  `gh auth status`.
- Repo: `seamus-sloan/omnibus`. Board: **Project #2** (user-owned,
  `https://github.com/users/seamus-sloan/projects/2`).

## Title convention — bracket the scope

Every title starts with a `[Scope]` prefix so you can tell what an issue belongs
to from the name alone.

| Issue kind | Prefix | Example |
|---|---|---|
| Roadmap epic (a `docs/roadmap/` feature) | `[Roadmap <phase>.<n>]` | `[Roadmap 5.4] Admin Panel` |
| Sub-issue of an epic | `[<Epic Name>]` | `[Admin Panel] Device & Session List / Revoke Endpoints` |
| Standalone leftover (see below) | `[<Feature Area>]` | `[Metadata Edit] Per-Library Metadata Precedence Setting` |

- Phase/number come from the roadmap filename (`5-4-admin-panel.md` → `5.4`).
- Title-case the feature name.
- Sub-issue bracket names the **parent epic**, not the roadmap phase.

## Labels

- `roadmap` — apply to **epics only** (the roadmap features). A label-filtered
  board view then shows exactly the features, not their sub-tasks. Standalone
  leftover issues do **not** get `roadmap`.
- `enhancement` — new feature work (epics, sub-issues, standalone leftovers).
- `security` — add when the issue touches auth/sessions/permissions/untrusted input.
- `mobile` — add for mobile-crate work.
- `tech-debt` — for the `[Tech Debt] …` issues.

## Description template (required body format)

Every issue body — epic, sub-issue, or standalone — uses this exact structure:

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
{epic → the verbatim roadmap doc in a <details> block; sub → "Part of #<epic>."; standalone → a one-line note}
```

- **Affected Crates**: always list all five in order, checking `[x]` the ones the work touches. An epic's set is the union of its subs'.
- **Attachments** is where the migrated roadmap doc lives (epics only), so deleting `docs/roadmap/` loses nothing.
- Determining crates + ACs accurately is per-issue judgment; fan it out across parallel agents (one per epic), then assemble bodies from a JSON of `{number, kind, parent, description, implementation, crates, acs}` and apply with `gh issue edit --body-file`. Watch the subagent spend limit on large batches — hand-author any groups whose agents fail rather than leaving them un-standardized.

## Verify against code, not the roadmap docs

**The `docs/roadmap/` files lag the code badly.** Before creating an epic from a
doc, confirm what's actually shipped by searching `db/`, `server/`, `frontend/`,
`shared/`, and `migrations/` — not by trusting the doc's status markers or the
absence of a TODO section. In the phase 3–6 migration, doc-based analysis
wrongly flagged several shipped features (journaling, mobile EPUB reader, ebook
upload, metadata edit) as unbuilt. Fan verification out across parallel
subagents, one per feature, each returning DONE / PARTIAL / NOT DONE per
candidate sub-issue with file-path evidence. Then:

- **Fully shipped doc** → no epic.
- **Partially shipped** → epic with only the remaining pieces as sub-issues.
- **A lone remaining gap** (one small piece) → a standalone `enhancement`
  issue, **not** a `roadmap` epic and **no** sub-issues, with a `[Feature Area]`
  prefix naming the shipped feature (e.g. `[Metadata Edit] …`).

## Recipe

### 1. Create the epic (parent) issue

```bash
gh issue create -R seamus-sloan/omnibus \
  --title "[Roadmap 5.4] Admin Panel" \
  --label enhancement,roadmap \
  --body "$BODY"
```

Body guidance:
- Lead with a `**Phase X.Y · Priority: Pn**` line and a one-paragraph objective.
- State what's already shipped vs. what the epic tracks (from the code check).
- **Do not link sibling roadmap docs** (`0-3-auth.md`) — those break when the
  folder is deleted. Reference features by code (F0.3, F5.1) and link **source
  files** (`db/src/auth.rs`) instead.
- End with `_Migrated from docs/roadmap/<file>.md._`.

### 2. Create sub-issues (one per remaining piece)

```bash
gh issue create -R seamus-sloan/omnibus \
  --title "[Admin Panel] Endpoint to Toggle registration_enabled" \
  --label enhancement \
  --body "$BODY1"   # start body with: "Part of #<epic> (<epic title>)."
```

### 3. Link sub-issues to the parent (native sub-issues)

Sub-issues are GitHub's epic/story equivalent — the parent renders a progress
bar that ticks up as children close. The API wants the child's **internal id**
(`.id`), not its number:

```bash
child_id=$(gh api repos/seamus-sloan/omnibus/issues/909 --jq '.id')
gh api --method POST repos/seamus-sloan/omnibus/issues/908/sub_issues \
  -F sub_issue_id=$child_id
```

Verify:

```bash
gh api graphql -f query='
query { repository(owner:"seamus-sloan", name:"omnibus") {
  issue(number:908) { subIssuesSummary { total completed percentCompleted } } } }'
```

### 4. Add to the Project board

Add **epics** to the board (so the roadmap view shows them). Leave sub-issues
**off** the board — they're tracked under the epic's progress bar — to keep the
Priority/other views uncluttered.

```bash
gh project item-add 2 --owner seamus-sloan \
  --url https://github.com/seamus-sloan/omnibus/issues/908
```

### 5. Set the Priority field

Priority is a **project field**, not an issue property. IDs for Project #2:

- Project: `PVT_kwHOAvpBfM4BdEVy`
- Priority field: `PVTSSF_lAHOAvpBfM4BdEVyzhXomZU`
- Options: **P0** `79628723` · **P1** `0a877460` · **P2** `da944a9c`
  (no P3 option exists — map roadmap P3 to P2 or leave unset)

```bash
gh api graphql -f query='
mutation { updateProjectV2ItemFieldValue(input:{
  projectId:"PVT_kwHOAvpBfM4BdEVy",
  itemId:"<item-id-from-step-4>",
  fieldId:"PVTSSF_lAHOAvpBfM4BdEVyzhXomZU",
  value:{ singleSelectOptionId:"da944a9c" }
}) { projectV2Item { id } } }'
```

## Batch migrations — data-driven script

For many issues at once, don't hand-run `gh` per issue. Put the plan in a JSON
file (`epics[]` with `title`/`labels`/`body`/`subs[]`, plus `standalone[]`) and
drive it with a bash loop that creates each epic, adds it to the board, then
creates + links its subs. Pace creates (~1s sleep) to dodge GitHub's secondary
rate limit, log every action to a TSV map, and run it as a background Bash task.
A working example script lived at the session scratchpad during the phase 3–6
migration (`create-roadmap-issues.sh` + `roadmap-issues.json`).

## Not possible via API

- **Creating/configuring Project views** (Board/Table, filters, grouping) is
  UI-only — there is no `createProjectV2View` mutation. Guide the user through
  the clicks, or drive it with browser automation.

## Migration progress

- **Phases 3–6:** migrated (epics #911, #916, #921, #929, #933, #938, #943,
  #946, #951, #956, #960, #968, plus #908 for 5.4; standalone #972, #973).
  Shipped features skipped: 3.1, 3.2, 3.3, 4.3, 5.1, 5.7, 5.10, 6.2.
- **Phases 0–2:** not yet migrated.

## TODO / open questions

- Roadmap view: currently filtered by `label:roadmap`. Phase lives only in the
  title prefix — no groupable `Phase` field yet (user declined adding one).
- Priorities not yet set on the phase 3–6 epics (board has no P3 option).
- Migrate phases 0–2 (mostly shipped — expect few epics, mostly skips).
