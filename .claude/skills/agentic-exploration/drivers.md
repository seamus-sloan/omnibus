# Browsers, the ownership guard, and the iOS simulator

Companion to [SKILL.md](SKILL.md) step 7. The commands are there; this is what
they buy and how the agents use them.

## One browser per agent is not a nicety

Run `r-20260828-01` died because three subagents shared a single browser tab:
one cookie jar, three users collapsed into one, and two agents correctly
aborted rather than journal under a wrong actor. `driver.sh up <N>` gives each
agent its own server, session and browser, which makes that impossible.

Agents drive theirs with `driver.sh run <n> "<command>"`, which prints
`{"text": ..., "isError": ...}`. Hand each agent **its own** number and no
other's. Tear down with `driver.sh down` (and `ios.sh down`) when the run ends,
whatever the outcome.

## The guard

```bash
scripts/explore/driver.sh guard 1 agent-1 "$(scripts/explore/owned.sh agent-1)"
```

Every agent is guarded before any of them starts, and the uuids come from the
journals, never from the agent. `owned.sh` reads **every** journal, not just
this run's — ownership is durable provenance, the same reason `provision.sh`
keeps usernames stable. Without the guard, ownership is only a sentence in
`start.md` and every exploration account is an admin, so nothing stops one
agent destroying another's books; with it, the request is refused before it is
sent.

The owned set is read once, when the guard is installed; a book the agent
uploads later is not in it until you run `guard` again — see
[scenarios.md](scenarios.md) for the three-step hand-over that follows an
upload. The guard's refusal is a synthetic 403 carrying the request URL, so
the app renders a permission failure rather than a transport error.

After the run, `driver.sh refusals <n>` lists what each agent was stopped from
doing. **A non-empty list is a finding about the agent or the flow document,
not about the app.**

The guard is a `fetch` wrapper inside the page, not DevTools interception.
Routing every request through CDP made Chromium copy each upload body into one
event, and a large audiobook killed the browser before the upload reached the
app (#2361); the wrapper never touches a body it is not about to inspect, so an
upload of any size passes through.

## When a browser dies

`driver.sh run` answers `{"driver": "dead"}` when the agent's server went down
under a command — a locator timeout thrown inside a promise chain rather than
awaited at the top level is enough to take the Node process with it — and `{"driver": "up"}` when the server is fine and the command
never returned — an app hang, or a locator that never matched. The first is the
harness: the agent journals an `issue`, runs `driver.sh restart <n>`, and the
guard is reinstalled — it lives in the old process and does not come back on
its own. `restart` re-registers the server, so `status` still knows it and
`down` still stops it; `down` sweeps the whole port window regardless (#2363),
and `status` names any driver it finds there unregistered.

## The iOS agent

```bash
scripts/explore/ios.sh up            # boot a simulator, build, install, launch
scripts/explore/ios.sh state         # {online, running, forced_offline}
```

**One iOS agent, ever.** Two on one simulator share a keychain, a container and
a session — the same collapse as a shared browser, with no isolation available
to fix it. `--ios` adds one; asking for more is a refusal, not a clamp.

It is a full agent with its own account, so provision `N+1`. `ios.sh up`
uninstalls the app before installing it, so the container — and with it the
previous run's session, downloads and outbox — starts empty (set
`OMNIBUS_IOS_KEEP_CONTAINER=1` to keep it); the brief still tells the agent
to check the You tab first, because a kept container comes up signed in as
the previous run's iOS actor. Give it
`surface: ios`, [`ios_lane.md`](../../../docs/qa/agentic_exploration/ios_lane.md)
alongside `start.md`, and the `offline_outbox` scenario from that file **on top
of** its sampled flows — it is the only surface that can run it. There is no
ownership guard here, so keep it off `adding_book` and `merging_books`.
