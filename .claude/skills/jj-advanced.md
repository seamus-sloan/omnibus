---
name: jj-advanced
description: Advanced jj operations — squash, rebase, abandon, undo, op log, and conflict resolution. Triggers when the user needs to reorder/collapse commits, recover from a mistake, or resolve conflicts from a rebase.
---

# jj advanced

Beyond the basics in [jj-basics.md](jj-basics.md). If you're unsure about an invocation, run `jj <command> --help` rather than guess.

## Squash — fold a change into its parent

```bash
jj squash              # fold @ into @- (keep description of @-)
jj squash -r <rev>     # fold <rev> into its parent
jj squash --into <rev> # fold @ into a specific destination
```

Useful when you made a fixup and want it absorbed into the earlier commit.

## Rebase — move a change onto a new base

```bash
jj rebase -d main              # rebase @ (and its descendants) onto main
jj rebase -r <rev> -d <dest>   # rebase only <rev>
jj rebase -s <rev> -d <dest>   # rebase <rev> and all descendants
```

After rebasing, bookmarks don't follow automatically — move them:

```bash
jj bookmark move <name> --to @
```

## Abandon — throw away a change

```bash
jj abandon             # abandon @ (work is gone from the log but recoverable via op log)
jj abandon -r <rev>    # abandon a specific change
```

## Undo — revert the last jj operation

```bash
jj undo                # undo the most recent jj operation
```

`jj undo` is reversible itself. It works at the operation layer, not the file layer.

## Op log — the operation history

```bash
jj op log                      # list every jj operation on this repo
jj op restore <op-id>          # restore repo state to a prior op
```

Use this when you've done something destructive and `jj undo` isn't enough — find the op id before the mistake and restore to it.

## Conflict resolution

After a rebase or merge, conflicted files contain standard conflict markers (`<<<<<<<` / `=======` / `>>>>>>>`). Workflow:

1. `jj st` shows `C` next to conflicted paths.
2. Open each file, resolve manually, remove the markers.
3. `jj squash` the resolution into the parent if it was purely a merge-fix change, or `jj describe` a new message if the fix is substantive.
4. `jj st` should now show no `C` markers.

`jj resolve` can launch an external merge tool for complex cases — `jj resolve --help` lists configured tools.

## Diff between revisions

```bash
jj diff -r <rev>               # diff against parent of <rev>
jj diff --from <a> --to <b>    # diff between two revs
```
