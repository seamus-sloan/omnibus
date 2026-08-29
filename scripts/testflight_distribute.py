#!/usr/bin/env python3
"""Hand an uploaded TestFlight build to its external tester groups.

`xcrun altool --upload-app` only delivers the binary to App Store Connect — it
never assigns the build to a tester group. Internal groups can be set to
distribute automatically, so they need nothing; **external groups have no such
setting**, and a build that is never added to one is simply invisible to every
tester on the public link. The upload workflow therefore goes green while the
group sees nothing, which is indistinguishable from success until somebody goes
looking. This script is the missing step.

It waits for processing, submits for Beta App Review when the build still needs
it (the first build of a marketing version does; later ones are usually waved
through), optionally sets "What to Test", and adds the build to each named
group.

Idempotent by construction: attaching a group is a set-semantics relationship
add (re-adding an attached build is a 204 no-op) and a build already submitted
for review is left alone, so a re-run — or a retry after a half-finished run —
is a no-op rather than a duplicate.

Reuses the App Store Connect API key the build-upload workflow already
configures (`ASC_API_KEY_BASE64` / `ASC_KEY_ID` / `ASC_ISSUER_ID`); that key is
team-scoped, so no new secrets are needed.

Env:
  ASC_ISSUER_ID, ASC_KEY_ID              App Store Connect API key identity.
  ASC_API_KEY_BASE64 | ASC_PRIVATE_KEY   The .p8 private key (base64, or raw PEM).
  BUNDLE_ID                              App to distribute (default: the SwiftUI app).
  MARKETING_VERSION, BUILD_NUMBER        Identify the build this run uploaded.
  BETA_GROUPS                            Comma-separated group names (exact match).
  WHATS_NEW                              Optional "What to Test" shown to testers.
  PROCESSING_TIMEOUT_SECS                How long to wait for processing (default 1800).
  DRY_RUN=1                              Resolve and report, change nothing.
"""
import base64
import binascii
import os
import sys
import time

import jwt  # PyJWT[crypto]
import requests

ASC_BASE = "https://api.appstoreconnect.apple.com"


def die(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def positive_int_env(name, default):
    """Read an integer setting, naming the culprit rather than tracebacking.

    A bare int() here raises at import, and an Actions log that opens on a
    ValueError stack trace reads as a broken script rather than a typo'd input.
    """
    raw = (os.environ.get(name) or "").strip() or str(default)
    if not raw.isdigit() or int(raw) <= 0:
        die(f"{name} must be a positive whole number of seconds, got: {raw!r}")
    return int(raw)


DRY_RUN = os.environ.get("DRY_RUN") == "1"
BUNDLE_ID = os.environ.get("BUNDLE_ID", "com.omnibus.mobile")
BETA_GROUPS = [g.strip() for g in os.environ.get("BETA_GROUPS", "").split(",") if g.strip()]
WHATS_NEW = (os.environ.get("WHATS_NEW") or "").strip()
PROCESSING_TIMEOUT_SECS = positive_int_env("PROCESSING_TIMEOUT_SECS", 1800)
POLL_SECS = 30
# Locale used when a build has no "What to Test" localization at all yet. Apple
# seeds one per the app's configured locales, so this is only the cold-start case.
DEFAULT_LOCALE = "en-US"


def private_key():
    raw = os.environ.get("ASC_PRIVATE_KEY")
    if raw:
        return raw
    b64 = os.environ.get("ASC_API_KEY_BASE64")
    if not b64:
        die("set ASC_PRIVATE_KEY or ASC_API_KEY_BASE64")
    try:  # secrets often arrive with stray whitespace/newlines
        return base64.b64decode(b64.strip(), validate=True).decode()
    except (binascii.Error, ValueError, UnicodeDecodeError) as e:
        die(f"ASC_API_KEY_BASE64 is not valid base64: {e}")


def asc_jwt():
    """Mint a short-lived API token.

    Minted per request rather than once: waiting out processing can outlast any
    single token, and a 401 twenty minutes into a poll would read as a
    credentials problem rather than an expiry.
    """
    now = int(time.time())
    return jwt.encode(
        {"iss": os.environ["ASC_ISSUER_ID"], "iat": now, "exp": now + 15 * 60,
         "aud": "appstoreconnect-v1"},
        private_key(),
        algorithm="ES256",
        headers={"kid": os.environ["ASC_KEY_ID"], "typ": "JWT"},
    )


def asc_raw(method, path, **kw):
    """Call the API and hand back the response, however it went."""
    url = path if path.startswith("http") else ASC_BASE + path
    return requests.request(method, url,
                            headers={"Authorization": f"Bearer {asc_jwt()}"},
                            timeout=60, **kw)


def asc(method, path, **kw):
    r = asc_raw(method, path, **kw)
    if r.status_code >= 400:
        die(f"App Store Connect {r.status_code} on {method} {path}: {r.text[:300]}")
    # Relationship writes answer 204 with an empty body.
    return r.json() if r.status_code != 204 and r.content else {}


# --- resolution -------------------------------------------------------------

def resolve_app_id():
    data = asc("GET", "/v1/apps", params={"filter[bundleId]": BUNDLE_ID, "limit": 1})["data"]
    if not data:
        die(f"no App Store Connect app for bundle id {BUNDLE_ID}")
    return data[0]["id"]


def resolve_groups(app_id):
    """Look up each requested group by exact name.

    A miss is fatal and prints the names that do exist: a renamed or
    mistyped group would otherwise distribute to nothing and still exit 0,
    which is the failure this script exists to prevent.
    """
    found = {g["attributes"]["name"]: g
             for g in asc("GET", f"/v1/apps/{app_id}/betaGroups",
                          params={"limit": 200})["data"]}
    missing = [n for n in BETA_GROUPS if n not in found]
    if missing:
        die(f"no TestFlight group named {', '.join(repr(m) for m in missing)}; "
            f"groups on this app: {', '.join(sorted(found)) or '(none)'}")
    return [found[n] for n in BETA_GROUPS]


def version_candidates(version):
    """Both spellings App Store Connect might list a marketing version under.

    It drops a trailing zero segment, so a build uploaded as 0.15.0 comes back
    as preReleaseVersion 0.15. Querying only what we stamped would never match a
    minor release, and the miss looks exactly like a build still processing —
    a 30-minute wait that then fails for the wrong reason.
    """
    parts = version.split(".")
    if len(parts) == 3 and parts[2].isdigit() and int(parts[2]) == 0:
        return [version, ".".join(parts[:2])]
    if len(parts) == 2:
        return [version, f"{version}.0"]
    return [version]


def find_build(app_id):
    """Return (build, buildBetaDetail attrs) for this run's version, or (None, {})."""
    for version in version_candidates(os.environ["MARKETING_VERSION"]):
        page = asc("GET", "/v1/builds", params={
            "filter[app]": app_id,
            "filter[version]": os.environ["BUILD_NUMBER"],
            "filter[preReleaseVersion.version]": version,
            "include": "buildBetaDetail",
            "limit": 1,
        })
        data = page.get("data") or []
        if data:
            detail = next((i for i in page.get("included", [])
                           if i["type"] == "buildBetaDetails"), {})
            return data[0], detail.get("attributes", {})
    return None, {}


def wait_for_build(app_id):
    """Poll until the uploaded build exists and has finished processing.

    Upload and processing are separate: `altool` returns as soon as the bytes
    land, and the build resource does not exist for a minute or two afterwards.
    """
    version = f"{os.environ['MARKETING_VERSION']} ({os.environ['BUILD_NUMBER']})"
    deadline = time.time() + PROCESSING_TIMEOUT_SECS
    last = None
    while True:
        build, detail = find_build(app_id)
        state = build["attributes"].get("processingState") if build else "(not yet listed)"
        if build and state == "VALID":
            print(f"build {version} processed ({build['id']})")
            return build, detail
        if state in ("FAILED", "INVALID"):
            die(f"build {version} finished processing as {state}; App Store Connect "
                "emails the reason to the account holder")
        if state != last:  # one line per transition, not one per poll
            print(f"waiting for build {version}: {state}")
            last = state
        if time.time() >= deadline:
            die(f"build {version} was still {state} after {PROCESSING_TIMEOUT_SECS}s; "
                "raise PROCESSING_TIMEOUT_SECS or add it to the group by hand")
        time.sleep(POLL_SECS)


# --- distribution -----------------------------------------------------------

def submit_for_beta_review(build_id, external_state):
    """Submit for Beta App Review when the build is waiting on it.

    Only `READY_FOR_BETA_SUBMISSION` needs the call; every other state either
    already has a submission or cannot take one, and posting anyway is an error
    rather than a no-op.
    """
    if external_state == "MISSING_EXPORT_COMPLIANCE":
        die("build is missing export compliance, so it cannot go to external testers. "
            "omnibus-ios/Info.plist sets ITSAppUsesNonExemptEncryption to answer this "
            "at build time — check the key survived into the uploaded binary.")
    if external_state != "READY_FOR_BETA_SUBMISSION":
        print(f"beta review: nothing to submit (external state {external_state})")
        return
    if DRY_RUN:
        print("beta review: [dry] would submit")
        return
    r = asc_raw("POST", "/v1/betaAppReviewSubmissions", json={"data": {
        "type": "betaAppReviewSubmissions",
        "relationships": {"build": {"data": {"type": "builds", "id": build_id}}},
    }})
    if r.status_code in (200, 201):
        print("beta review: submitted")
    elif r.status_code == 409:  # a concurrent run, or Apple auto-submitted on attach
        print("beta review: already submitted")
    else:
        die(f"beta review submission failed ({r.status_code}): {r.text[:300]}")


def set_whats_new(build_id):
    """Write the tester-facing "What to Test" note, if one was supplied.

    Updates every localization the build already has rather than guessing which
    one testers read; a build with none yet gets one in DEFAULT_LOCALE.
    """
    if not WHATS_NEW:
        return
    existing = asc("GET", "/v1/betaBuildLocalizations",
                   params={"filter[build]": build_id, "limit": 50})["data"]
    if DRY_RUN:
        locales = ", ".join(l["attributes"]["locale"] for l in existing) or DEFAULT_LOCALE
        print(f"what to test: [dry] would set for {locales}")
        return
    for loc in existing:
        asc("PATCH", f"/v1/betaBuildLocalizations/{loc['id']}", json={"data": {
            "type": "betaBuildLocalizations",
            "id": loc["id"],
            "attributes": {"whatsNew": WHATS_NEW},
        }})
    if not existing:
        asc("POST", "/v1/betaBuildLocalizations", json={"data": {
            "type": "betaBuildLocalizations",
            "attributes": {"whatsNew": WHATS_NEW, "locale": DEFAULT_LOCALE},
            "relationships": {"build": {"data": {"type": "builds", "id": build_id}}},
        }})
    print(f"what to test: set for {len(existing) or 1} localization(s)")


def add_to_groups(build_id, groups):
    """Attach the build to each group, from the group side.

    Deliberately no membership pre-check: App Store Connect forbids reading a
    build's `betaGroups` relationship (403 `GET_RELATED`; only CREATE/DELETE
    are allowed), and the group-side relationship add is set-semantics —
    re-adding an already-attached build answers 204 and changes nothing — so
    posting unconditionally is what keeps the script idempotent.
    """
    names = ", ".join(repr(g["attributes"]["name"]) for g in groups)
    if DRY_RUN:
        print(f"groups: [dry] would attach to {names}")
        return
    for g in groups:
        asc("POST", f"/v1/betaGroups/{g['id']}/relationships/builds",
            json={"data": [{"type": "builds", "id": build_id}]})
    print(f"groups: attached to {names}")


def describe(group):
    a = group["attributes"]
    kind = "internal" if a.get("isInternalGroup") else "external"
    link = " · public link" if a.get("publicLinkEnabled") else ""
    return f"{a['name']!r} ({kind}{link})"


def main():
    for req in ("ASC_ISSUER_ID", "ASC_KEY_ID", "MARKETING_VERSION", "BUILD_NUMBER"):
        if not os.environ.get(req):
            die(f"missing env {req}")
    if not BETA_GROUPS:
        die("set BETA_GROUPS to the TestFlight group name(s) to distribute to")

    app_id = resolve_app_id()
    groups = resolve_groups(app_id)
    print(f"app {BUNDLE_ID} ({app_id}); groups: {', '.join(describe(g) for g in groups)}; "
          f"dry_run={DRY_RUN}")

    build, detail = wait_for_build(app_id)
    build_id = build["id"]

    # Order matters: fastlane's pilot submits for review before attaching groups,
    # and the external state that decides whether a submission is needed is read
    # from the build as it stands before any attachment changes it.
    submit_for_beta_review(build_id, detail.get("externalBuildState"))
    set_whats_new(build_id)
    add_to_groups(build_id, groups)

    print(f"done: build {os.environ['MARKETING_VERSION']} "
          f"({os.environ['BUILD_NUMBER']}) is with {len(groups)} group(s)")


if __name__ == "__main__":
    main()
