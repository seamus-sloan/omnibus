#!/usr/bin/env python3
"""Turn TestFlight screenshot feedback into GitHub issues.

Polls the App Store Connect API `betaFeedbackScreenshotSubmissions` endpoint for
each TestFlight app, and opens one GitHub issue per new tester submission —
tester comment, device/OS environment, and the screenshot rendered inline.
Idempotent: each issue embeds a hidden `asc-feedback-id` marker and the script
skips a submission whose marker already appears in an existing issue.

Reuses the App Store Connect API key already configured for the build-upload
workflow (`ASC_API_KEY_BASE64` / `ASC_KEY_ID` / `ASC_ISSUER_ID`). Both iOS
clients live under one App Store Connect team, so that one key reads both.
Screenshots are committed to a dedicated `testflight-feedback` branch so they
render inline and persist past App Store Connect's short-lived signed URLs.

Env:
  ASC_ISSUER_ID, ASC_KEY_ID              App Store Connect API key identity.
  ASC_API_KEY_BASE64 | ASC_PRIVATE_KEY   The .p8 private key (base64, or raw PEM).
  GITHUB_TOKEN                           Repo token (issues:write + contents:write).
  GITHUB_REPOSITORY                      "owner/repo" (set automatically in Actions).
  BUNDLE_IDS                             Comma-separated bundle ids (default: both iOS clients).
  ASSET_BRANCH                           Branch for screenshot assets (default testflight-feedback).
  MAX_PAGES                              Submission pages to scan per run (default 5).
  DRY_RUN=1                              Print instead of creating issues/assets.
"""
import base64
import binascii
import os
import sys
import time

import jwt  # PyJWT[crypto]
import requests

ASC_BASE = "https://api.appstoreconnect.apple.com"
GH_BASE = "https://api.github.com"
DRY_RUN = os.environ.get("DRY_RUN") == "1"
# Bundle id -> the name shown in the issue title and Environment block.
APP_NAMES = {"com.omnibus.mobile": "Dioxus", "com.omnibus.swiftui": "SwiftUI"}
BUNDLE_IDS = [b.strip() for b in
              os.environ.get("BUNDLE_IDS", ",".join(APP_NAMES)).split(",") if b.strip()]
ASSET_BRANCH = os.environ.get("ASSET_BRANCH", "testflight-feedback")
MAX_PAGES = int(os.environ.get("MAX_PAGES", "5"))
LABELS = ["testflight", "mobile", "bug"]


def die(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


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
    now = int(time.time())
    return jwt.encode(
        {"iss": os.environ["ASC_ISSUER_ID"], "iat": now, "exp": now + 15 * 60,
         "aud": "appstoreconnect-v1"},
        private_key(),
        algorithm="ES256",
        headers={"kid": os.environ["ASC_KEY_ID"], "typ": "JWT"},
    )


def asc_get(url, token, params=None):
    r = requests.get(url if url.startswith("http") else ASC_BASE + url,
                     headers={"Authorization": f"Bearer {token}"}, params=params, timeout=60)
    if r.status_code >= 400:
        die(f"App Store Connect {r.status_code} on {url}: {r.text[:300]}")
    return r.json()


def resolve_app(token, bundle_id):
    """Resolve a bundle id to its App Store Connect app record, or None if absent.

    A missing record warns rather than dies: an app that hasn't been created on
    the Apple side yet must not stop the other app's feedback from being filed.
    """
    data = asc_get("/v1/apps", token, {"filter[bundleId]": bundle_id, "limit": 1})["data"]
    if not data:
        print(f"warn: no App Store Connect app for bundle id {bundle_id}; skipping",
              file=sys.stderr)
        return None
    return {"id": data[0]["id"], "bundle_id": bundle_id,
            "name": APP_NAMES.get(bundle_id, bundle_id)}


def fetch_submissions(token, app_id):
    """Yield (submission, included-index) across up to MAX_PAGES pages, newest first.

    Listed via the app relationship — the top-level
    `/v1/betaFeedbackScreenshotSubmissions` collection does not allow
    GET_COLLECTION (403), only per-app/per-build listing does.
    """
    url = f"/v1/apps/{app_id}/betaFeedbackScreenshotSubmissions"
    params = {"include": "build,tester", "sort": "-createdDate", "limit": 50}
    for _ in range(MAX_PAGES):
        page = asc_get(url, token, params)
        included = {(i["type"], i["id"]): i.get("attributes", {}) for i in page.get("included", [])}
        for sub in page.get("data", []):
            yield sub, included
        nxt = page.get("links", {}).get("next")
        if not nxt:
            break
        url, params = nxt, None  # `next` is a fully-formed URL


# --- GitHub -----------------------------------------------------------------

def gh(method, path, token, **kw):
    r = requests.request(method, path if path.startswith("http") else GH_BASE + path,
                         headers={"Authorization": f"Bearer {token}",
                                  "Accept": "application/vnd.github+json"},
                         timeout=60, **kw)
    return r


def repo():
    r = os.environ.get("GITHUB_REPOSITORY")
    if not r:
        die("set GITHUB_REPOSITORY (owner/repo)")
    return r


def already_filed(token, sub_id):
    q = f'repo:{repo()} in:body "asc-feedback-id: {sub_id}"'
    r = gh("GET", "/search/issues", token, params={"q": q})
    if r.status_code != 200:
        print(f"  warn: search failed ({r.status_code}); proceeding", file=sys.stderr)
        return False
    return r.json().get("total_count", 0) > 0


def ensure_asset_branch(token):
    r = gh("GET", f"/repos/{repo()}/branches/{ASSET_BRANCH}", token)
    if r.status_code == 200:
        return
    if r.status_code != 404:
        die(f"checking asset branch failed ({r.status_code}): {r.text[:200]}")
    info = gh("GET", f"/repos/{repo()}", token)
    if info.status_code != 200:
        die(f"repo lookup failed ({info.status_code}): {info.text[:200]}")
    default = info.json()["default_branch"]
    ref = gh("GET", f"/repos/{repo()}/git/ref/heads/{default}", token)
    if ref.status_code != 200:
        die(f"default-branch ref lookup failed ({ref.status_code}): {ref.text[:200]}")
    sha = ref.json()["object"]["sha"]
    created = gh("POST", f"/repos/{repo()}/git/refs", token,
                 json={"ref": f"refs/heads/{ASSET_BRANCH}", "sha": sha})
    # 422 == ref already exists (another run raced us here) — treat as success.
    if created.status_code not in (201, 422):
        die(f"creating asset branch failed ({created.status_code}): {created.text[:200]}")


def upload_asset(token, path, content, message):
    payload = {"message": message, "branch": ASSET_BRANCH,
               "content": base64.b64encode(content).decode()}
    # If the path already exists on the branch (e.g. a retry after a partial run),
    # the Contents API requires the current blob sha to overwrite it.
    existing = gh("GET", f"/repos/{repo()}/contents/{path}", token, params={"ref": ASSET_BRANCH})
    if existing.status_code == 200:
        payload["sha"] = existing.json()["sha"]
    r = gh("PUT", f"/repos/{repo()}/contents/{path}", token, json=payload)
    if r.status_code not in (200, 201):
        print(f"  warn: asset upload failed ({r.status_code}): {r.text[:200]}", file=sys.stderr)
        return None
    return r.json()["content"]["download_url"]


def create_issue(token, title, body):
    r = gh("POST", f"/repos/{repo()}/issues", token,
           json={"title": title, "body": body, "labels": LABELS})
    if r.status_code != 201:
        die(f"issue creation failed ({r.status_code}): {r.text[:300]}")
    return r.json()["number"]


# --- rendering --------------------------------------------------------------

def build_version(sub, included):
    rel = sub.get("relationships", {}).get("build", {}).get("data")
    if rel:
        attrs = included.get((rel["type"], rel["id"]), {})
        return attrs.get("version")
    return None


def tester_name(sub, included):
    rel = sub.get("relationships", {}).get("tester", {}).get("data")
    if not rel:
        return None
    a = included.get((rel["type"], rel["id"]), {})
    name = " ".join(x for x in (a.get("firstName"), a.get("lastName")) if x).strip()
    return name or a.get("email")


def render(sub, included, app, image_urls):
    a = sub.get("attributes", {})
    sub_id = sub["id"]
    comment = (a.get("comment") or "").strip()
    device = a.get("deviceModel") or "Unknown device"
    build = build_version(sub, included)
    marketing = a.get("appVersionString")
    ver = f"{marketing} ({build})" if marketing and build else (build or marketing or "?")

    first = comment.splitlines()[0] if comment else "Screenshot feedback"
    summary = (first[:70] + "…") if len(first) > 70 else first
    title = f"[TestFlight · {app['name']}] {device} · build {ver} — {summary}"

    env = [
        f"- **App:** {app['name']} (`{app['bundle_id']}`)",
        f"- **Device:** {device}",
        f"- **OS:** {a.get('osVersion', '?')} ({a.get('devicePlatform') or a.get('appPlatform', '?')})",
        f"- **App version:** {ver}",
        f"- **Locale:** {a.get('locale', '?')}",
    ]
    if a.get("batteryPercentage") is not None:
        env.append(f"- **Battery:** {a['batteryPercentage']}%")
    if a.get("createdDate"):
        env.append(f"- **Submitted:** {a['createdDate']}")
    tester = tester_name(sub, included)
    if tester:
        env.append(f"- **Tester:** {tester}")

    shots = "\n\n".join(f"![screenshot {i + 1}]({u})" for i, u in enumerate(image_urls)) \
        or "_(screenshot could not be retrieved)_"
    asc_link = f"https://appstoreconnect.apple.com/apps/{app['id']}/testflight/ios/feedback"

    body = (
        f"## Description\n{comment or '_(no comment provided by tester)_'}\n\n"
        f"## Environment\n" + "\n".join(env) + "\n\n"
        f"## Screenshot(s)\n{shots}\n\n"
        f"## Source\n[View in App Store Connect]({asc_link}) — submission `{sub_id}`\n\n"
        f"<!-- asc-feedback-id: {sub_id} -->\n"
    )
    return title, body


def screenshot_urls(sub):
    for s in sub.get("attributes", {}).get("screenshots", []) or []:
        u = s.get("url") if isinstance(s, dict) else s
        if u:
            yield u


def collect_images(gh_token, sub_id, sub):
    """Mirror a submission's screenshots onto the asset branch; return their URLs."""
    urls = []
    for n, url in enumerate(screenshot_urls(sub)):
        if DRY_RUN:
            urls.append(url)  # temporary ASC URL — fine for preview, no upload
            continue
        img = requests.get(url, timeout=60)
        if img.status_code != 200:
            print(f"  warn: screenshot {n} for {sub_id} -> {img.status_code}", file=sys.stderr)
            continue
        raw = upload_asset(gh_token, f"assets/{sub_id}-{n}.png", img.content,
                           f"testflight feedback asset {sub_id}-{n}")
        if raw:
            urls.append(raw)
    return urls


def main():
    for req in ("ASC_ISSUER_ID", "ASC_KEY_ID", "GITHUB_TOKEN"):
        if not os.environ.get(req):
            die(f"missing env {req}")
    gh_token = os.environ["GITHUB_TOKEN"]
    asc_token = asc_jwt()
    apps = [a for a in (resolve_app(asc_token, b) for b in BUNDLE_IDS) if a]
    if not apps:
        die(f"no App Store Connect app matched any of {', '.join(BUNDLE_IDS)}")
    listed = ", ".join(f"{a['name']} {a['id']} ({a['bundle_id']})" for a in apps)
    print(f"apps: {listed}; repo {repo()}; dry_run={DRY_RUN}")

    if not DRY_RUN:
        ensure_asset_branch(gh_token)

    created = skipped = 0
    for app in apps:
        for sub, included in fetch_submissions(asc_token, app["id"]):
            sub_id = sub["id"]
            # Submission ids are unique across apps, so one marker search covers both.
            if already_filed(gh_token, sub_id):
                skipped += 1
                continue

            title, body = render(sub, included, app,
                                 collect_images(gh_token, sub_id, sub))
            if DRY_RUN:
                print(f"\n[dry] would create: {title}\n{body}")
            else:
                num = create_issue(gh_token, title, body)
                print(f"created #{num}: {title}")
            created += 1

    print(f"done: created={created} skipped(existing)={skipped}")


if __name__ == "__main__":
    main()
