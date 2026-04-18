---
name: jj-basics
description: Core jj workflow for omnibus — fetch, status, new change, describe, bookmark, push. Triggers when the user asks to commit, push, create a branch/bookmark, or perform routine version-control operations.
---

# jj basics

This repo uses [jj](https://github.com/martinvonz/jj) instead of plain git. jj sits on top of the same git storage, so remotes, pushes, and fetches all go through `jj git ...`.

## Branch naming

- Personal branches: `u/<last_name>/<feat_name>` (e.g. `u/sloan/fix-failing-tests`).
- Issue branches: `omnibus-<issue_number>/<branch_name>` (e.g. `omnibus-6/add-more-things`).

## Commits

Use [conventional commits](https://www.conventionalcommits.org/) with only these prefixes: `feat`, `fix`, `chore`. No scopes like `docs(...)`, `refactor(...)`, etc.

## Daily workflow

```bash
jj git fetch                                      # pull latest
jj st                                             # check working copy
jj new main                                       # start a new change off main
jj bookmark create u/sloan/my-feature             # create the bookmark
jj describe -m "feat: add my feature"             # describe the change
# ...edit files...
jj bookmark move u/sloan/my-feature --to @        # move bookmark to current change
jj git push                                       # push to origin
```

## Multi-commit work

After each logical commit:

```bash
jj describe -m "chore: ..."
jj bookmark move u/sloan/my-feature --to @
jj new                                             # start the next change on top
```

## Useful one-liners

```bash
jj log -r 'main..@'                                # see all changes since main
jj diff                                            # show uncommitted diff
jj --help                                          # or `jj <cmd> --help` for more
```

For rebase / squash / conflict resolution / undo, see [jj-advanced](../jj-advanced/SKILL.md). For parallel agent work, see [jj-workspaces](../jj-workspaces/SKILL.md).
