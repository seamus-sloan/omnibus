---
name: jj-workspaces
description: Use jj workspaces to run multiple agents in parallel on the same repo. Triggers when the user asks to parallelize work, spin up a worktree, or run multiple agents against different branches of omnibus simultaneously.
---

# jj workspaces

For parallel agent work on a jj-driven repo, use **jj workspaces** instead of the Agent tool's `isolation: "worktree"` (which creates a git worktree). jj workspaces share the same `.jj/` store, so bookmarks and changes made in one workspace are **immediately visible** from the main workspace — no marshalling required.

## Commands

```bash
# Create a new workspace at a path, named `<name>`, starting at revision `<rev>`
jj workspace add <path> --name <name> -r <rev>
# Example:
jj workspace add ../omnibus-feat --name feat -r main

# List all workspaces
jj workspace list

# Untrack a workspace after deleting its directory
rm -rf <path>
jj workspace forget <name>
```

## Inside a workspace

`jj log` inside any workspace shows the other workspaces' working copies as `<name>@` markers, so you can see what each agent is working on.

## When to use

- Independent feature branches being developed simultaneously.
- Spawning parallel agents that each need their own checkout.
- Running long-running checks (e.g. a full build) without blocking other work.

## When NOT to use

For a throwaway experiment you'd just `jj abandon`, stay in the main workspace — workspaces are overkill.
