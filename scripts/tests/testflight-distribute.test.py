#!/usr/bin/env python3
"""Tests for scripts/testflight_distribute.py — the step that hands an uploaded
build to its external tester groups.

No network and no App Store Connect key: the real script is imported (so this
tracks the shipped implementation rather than a hand-copied duplicate) and only
its two I/O primitives — `asc_raw` and `asc_jwt` — are stubbed. Each scenario
declares what the API would answer, then asserts which writes the script makes.
Which writes it *doesn't* make is the point of half of them: the run is meant to
be re-runnable, so an already-attached group or an already-submitted review must
produce no POST at all.

Usage: scripts/tests/testflight-distribute.test.py
"""
import importlib.util
import json
import os
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = REPO_ROOT / "scripts" / "testflight_distribute.py"

APP_ID = "app-1"
BUILD_ID = "build-1"
GROUP_ID = "group-1"
GROUP_NAME = "First 20 Testers"

pass_count = 0
fail_count = 0


def check(desc, expected, actual):
    global pass_count, fail_count
    if actual == expected:
        print(f"PASS: {desc}")
        pass_count += 1
    else:
        print(f"FAIL: {desc} (expected {expected!r}, got {actual!r})", file=sys.stderr)
        fail_count += 1


class Resp:
    """The slice of requests.Response the script actually touches."""

    def __init__(self, status_code=200, payload=None):
        self.status_code = status_code
        self._payload = {} if payload is None else payload
        self.text = json.dumps(self._payload)
        self.content = self.text.encode()

    def json(self):
        return self._payload


class FakeASC:
    """A scripted App Store Connect.

    `build_states` is consumed one entry per `GET /v1/builds`, so a scenario can
    say "PROCESSING, then VALID" and exercise the poll rather than only its
    happy last iteration.
    """

    def __init__(self, build_states, external_state="READY_FOR_BETA_SUBMISSION",
                 attached_groups=(), localizations=(), groups=((GROUP_ID, GROUP_NAME),),
                 review_status=201, listed_version=None):
        self.build_states = list(build_states)
        self.external_state = external_state
        self.attached_groups = list(attached_groups)
        self.localizations = list(localizations)
        self.groups = list(groups)
        self.review_status = review_status
        # The marketing version App Store Connect actually lists the build
        # under; None matches whatever is asked for.
        self.listed_version = listed_version
        self.build_queries = []  # every preReleaseVersion.version filter tried
        self.writes = []  # (method, path, body) for every mutating call

    def __call__(self, method, path, **kw):
        if method != "GET":
            self.writes.append((method, path, kw.get("json")))

        if path == "/v1/apps":
            return Resp(payload={"data": [{"id": APP_ID}]})

        if path == f"/v1/apps/{APP_ID}/betaGroups":
            return Resp(payload={"data": [
                {"id": gid, "attributes": {"name": name, "isInternalGroup": False,
                                           "publicLinkEnabled": True}}
                for gid, name in self.groups]})

        if path == "/v1/builds":
            asked = kw["params"]["filter[preReleaseVersion.version]"]
            self.build_queries.append(asked)
            if self.listed_version is not None and asked != self.listed_version:
                return Resp(payload={"data": []})  # ASC lists it under the other spelling
            state = self.build_states.pop(0) if self.build_states else "VALID"
            if state is None:  # the build isn't listed yet
                return Resp(payload={"data": []})
            return Resp(payload={
                "data": [{"id": BUILD_ID, "attributes": {"processingState": state}}],
                "included": [{"type": "buildBetaDetails",
                              "attributes": {"externalBuildState": self.external_state}}],
            })

        if path == f"/v1/builds/{BUILD_ID}/betaGroups":
            return Resp(payload={"data": [{"id": g} for g in self.attached_groups]})

        if path == "/v1/betaAppReviewSubmissions":
            return Resp(status_code=self.review_status, payload={})

        if path == "/v1/betaBuildLocalizations":
            if method == "GET":
                return Resp(payload={"data": [
                    {"id": lid, "attributes": {"locale": loc}}
                    for lid, loc in self.localizations]})
            return Resp(status_code=201, payload={})

        if path.startswith("/v1/betaBuildLocalizations/"):
            return Resp(payload={})

        if path == f"/v1/builds/{BUILD_ID}/relationships/betaGroups":
            return Resp(status_code=204, payload={})

        raise AssertionError(f"unstubbed request: {method} {path}")


def run(fake, **env):
    """Import the script fresh under `env` and run main() against `fake`.

    Fresh because the script reads BETA_GROUPS / WHATS_NEW / DRY_RUN into module
    constants at import, so a scenario that changes them needs a new module.
    Returns (exit_code, fake).
    """
    base = {
        "ASC_ISSUER_ID": "issuer", "ASC_KEY_ID": "key", "ASC_PRIVATE_KEY": "pem",
        "BUNDLE_ID": "com.omnibus.mobile", "MARKETING_VERSION": "0.14.3",
        "BUILD_NUMBER": "42", "BETA_GROUPS": GROUP_NAME, "WHATS_NEW": "",
        "DRY_RUN": "0",
    }
    base.update(env)
    saved = dict(os.environ)
    os.environ.update(base)
    try:
        spec = importlib.util.spec_from_file_location("tf_distribute", SCRIPT)
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        mod.asc_raw = fake
        mod.asc_jwt = lambda: "stub-token"
        mod.POLL_SECS = 0  # the poll is the logic under test; the wait is not
        try:
            mod.main()
            return 0, fake
        except SystemExit as e:
            return e.code or 0, fake
    finally:
        os.environ.clear()
        os.environ.update(saved)


def writes_to(fake, path_fragment):
    return [w for w in fake.writes if path_fragment in w[1]]


# A build that is still processing when first polled, then valid: the poll has
# to survive both "not listed yet" and PROCESSING before it sees the build.
code, fake = run(FakeASC([None, "PROCESSING", "VALID"]))
check("happy path exits 0", 0, code)
check("happy path submits for beta review", 1,
      len(writes_to(fake, "/v1/betaAppReviewSubmissions")))
check("happy path attaches the group", 1,
      len(writes_to(fake, "/relationships/betaGroups")))

# Re-running the same distribution must not attach a second time.
code, fake = run(FakeASC(["VALID"], external_state="IN_BETA_TESTING",
                         attached_groups=[GROUP_ID]))
check("re-run exits 0", 0, code)
check("re-run does not re-attach an attached group", 0,
      len(writes_to(fake, "/relationships/betaGroups")))
check("re-run does not resubmit for review", 0,
      len(writes_to(fake, "/v1/betaAppReviewSubmissions")))

# A build already awaiting review still needs attaching — the two are separate
# facts, and treating "submitted" as "distributed" is the original bug.
code, fake = run(FakeASC(["VALID"], external_state="WAITING_FOR_BETA_REVIEW"))
check("awaiting-review build is still attached", 1,
      len(writes_to(fake, "/relationships/betaGroups")))
check("awaiting-review build is not resubmitted", 0,
      len(writes_to(fake, "/v1/betaAppReviewSubmissions")))

# Apple answers 409 when a submission already exists; that is not a failure.
code, _ = run(FakeASC(["VALID"], review_status=409))
check("a 409 on review submission is tolerated", 0, code)

# A group name that matches nothing must fail loudly rather than distribute to
# no one and exit 0.
code, fake = run(FakeASC(["VALID"], groups=[(GROUP_ID, "Some Other Group")]))
check("unknown group name fails the run", 1, code)
check("unknown group name writes nothing", 0, len(fake.writes))

# States that can never reach an external tester.
code, fake = run(FakeASC(["VALID"], external_state="MISSING_EXPORT_COMPLIANCE"))
check("missing export compliance fails the run", 1, code)
check("missing export compliance attaches nothing", 0, len(fake.writes))

code, fake = run(FakeASC(["PROCESSING", "INVALID"]))
check("an invalid build fails the run", 1, code)
check("an invalid build attaches nothing", 0, len(fake.writes))

# What to Test: update what exists, create only when there is nothing.
code, fake = run(FakeASC(["VALID"], localizations=[("loc-1", "en-US")]),
                 WHATS_NEW="Fixed the reader.")
check("whats-new patches the existing localization", 1,
      len(writes_to(fake, "/v1/betaBuildLocalizations/loc-1")))
code, fake = run(FakeASC(["VALID"]), WHATS_NEW="Fixed the reader.")
check("whats-new creates one when none exists", 1,
      [w[0] for w in writes_to(fake, "/v1/betaBuildLocalizations")].count("POST"))
code, fake = run(FakeASC(["VALID"], localizations=[("loc-1", "en-US")]))
check("no whats-new leaves localizations alone", 0,
      len(writes_to(fake, "betaBuildLocalizations")))

# App Store Connect drops a trailing zero segment, so a build stamped 0.15.0 is
# listed as 0.15. Matching only what was stamped would look identical to a build
# still processing, and fail 30 minutes later for the wrong reason.
code, fake = run(FakeASC(["VALID"], listed_version="0.15"), MARKETING_VERSION="0.15.0")
check("a X.Y.0 release is found under its X.Y listing", 0, code)
check("a X.Y.0 release is still attached", 1,
      len(writes_to(fake, "/relationships/betaGroups")))
check("both version spellings are tried", ["0.15.0", "0.15"], fake.build_queries)

# The reverse spelling, and the ordinary case that must not grow a second query.
code, fake = run(FakeASC(["VALID"], listed_version="0.15.0"), MARKETING_VERSION="0.15")
check("a X.Y release is found under its X.Y.0 listing", 0, code)
code, fake = run(FakeASC(["VALID"], listed_version="0.14.3"), MARKETING_VERSION="0.14.3")
check("a patch release is queried once", ["0.14.3"], fake.build_queries)

# Dry run resolves everything and writes nothing.
code, fake = run(FakeASC(["VALID"]), DRY_RUN="1", WHATS_NEW="Fixed the reader.")
check("dry run exits 0", 0, code)
check("dry run writes nothing", 0, len(fake.writes))

print("---")
print(f"{pass_count} passed, {fail_count} failed")
sys.exit(1 if fail_count else 0)
