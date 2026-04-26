# 05 — Pull requests

How to open a PR. Apply mechanically — these are not preferences.

Not every change needs a PR. Only open one when the user explicitly asks for it (e.g. "open a PR", "push it up as a PR"). For local commits or pushes without a PR request, stop after the push.

## Title

Match the conventional-commit prefix used on the branch's commits. The prefix maps 1:1 with the type of work:

- `feat: …` — new functionality
- `fix: …` — bug fix
- `chore: …` — refactor, docs, deps, infra, tooling

If the branch contains multiple commits, the title summarizes the dominant change with the same prefix used on the lead commit. Don't drift between `feat:` commits and a `chore:` PR title (or vice versa).

Keep titles under ~70 chars. Detail goes in the body, not the title.

## Body

Follow the repo's PR template at [.github/pull_request_template.md](../../.github/pull_request_template.md). The required sections, in order:

```markdown
## Summary
- 1-3 bullet points describing what changed and why.

## Test plan
- [ ] Bulleted checklist of how to verify the change.

## Notes
- Anything else worth flagging (follow-ups, caveats, screenshots). Omit if empty.
```

Pull facts from the actual diff and the conversation that led to the change — never invent items to fill space. If the change is doc-only, the test plan is "N/A — docs only" and that's fine.

## Assignee

Always assign the current user (the GitHub login resolved via `gh api user --jq .login`). Without an assignee the PR drops out of the dashboard view.

```bash
gh pr create --assignee @me ...
```

## Labels

Pick exactly one of the following based on the dominant change type:

| Change type | Label |
|---|---|
| New feature / behavior | `enhancement` |
| Bug fix | `bug` |
| Docs-only (CLAUDE.md, rules, roadmap, READMEs) | `documentation` |

Refactors, dependency bumps, and infra tweaks: choose the closest fit (usually `enhancement` for a behavior-affecting refactor, `documentation` for pure doc moves).

**Additionally:** if the PR diff touches anything under `ui_tests/`, add the `run_ui_tests` label too. This gates the Playwright CI job.

```bash
gh pr create --label enhancement --label run_ui_tests ...
```

## End-to-end example

Run `gh` from the repo's main checkout (e.g. `/Users/seamus/Repos/omnibus`). When working from a jj workspace (e.g. `omnibus-xray`), `cd` into the main repo first — `gh` resolves the upstream repo from the working directory, and a workspace path may not have one wired up the same way.

```bash
cd /Users/seamus/Repos/omnibus
gh pr create \
  --title "feat: add ratings & journaling tables" \
  --assignee @me \
  --label enhancement \
  --label run_ui_tests \
  --body "$(cat <<'EOF'
## Summary
- Adds `user_ratings` and `user_journal_entries` tables keyed by `book_uuid` (soft ref).
- Wires the rating UI into the book detail page slot from F1.4.
- Adds a Playwright flow covering rate → journal → reload.

## Test plan
- [ ] `cargo test -p omnibus-db user_ratings`
- [ ] `cargo test -p omnibus ratings`
- [ ] `cd ui_tests/playwright && npx playwright test ratings.spec.ts`
EOF
)"
```

## Sanity check before opening

- Title prefix matches the lead commit's prefix.
- Body's summary describes the actual diff (not a stale plan).
- Assignee is set.
- One type label is set; `run_ui_tests` is set iff `ui_tests/` was touched.
