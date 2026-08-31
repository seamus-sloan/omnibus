# Omnibus Helm chart

Deploys [Omnibus](https://github.com/seamus-sloan/omnibus) — a self-hosted
ebook/audiobook library — onto Kubernetes, wrapping the same `sesloan/omnibus`
image the [Docker guide](../../docs/docker.md) uses.

```bash
helm install omnibus ./charts/omnibus \
  --namespace omnibus --create-namespace \
  --set ingress.enabled=true \
  --set ingress.hosts[0].host=books.example.com \
  --set ingress.hosts[0].paths[0].path=/ \
  --set ingress.hosts[0].paths[0].pathType=Prefix \
  --set ingress.tls[0].secretName=omnibus-tls \
  --set ingress.tls[0].hosts[0]=books.example.com \
  --set libraries.ebooks.enabled=true \
  --set libraries.ebooks.volume.persistentVolumeClaim.claimName=my-ebooks
```

The **first account you register becomes the admin**, so register before
exposing the instance. Then `helm test omnibus -n omnibus` to confirm the
health endpoint answers.

This chart is deliberately small. It covers Deployment, Service, PVCs, Ingress,
ConfigMap/Secret and a health test — nothing speculative. If you already run
[bjw-s `app-template`](https://bjw-s-labs.github.io/helm-charts/docs/app-template/)
for everything else, use that instead and lift the constraints below into your
own values; they are what actually matter.

## The four things that matter

Everything here follows from the app, not from Helm convention.

### 1. One replica, always

Omnibus keeps everything — library metadata, per-reader progress, highlights,
shelves — in one SQLite file on `/config`. Two pods writing it concurrently
corrupts it. So `replicas` is **hard-coded to 1 and not exposed as a value**,
and the strategy is `Recreate`: a `RollingUpdate` would briefly run both pods
against the same volume, which is exactly what the single replica prevents.
The cost is a few seconds of downtime per upgrade.

PVCs default to `ReadWriteOnce` for the same reason. Putting `/config` on RWX
shared storage does not enable scaling — it just removes the guard rail.

### 2. The access URL, or every write 403s

`OMNIBUS_PUBLIC_ORIGIN` is a CSRF allowlist. When it doesn't match the URL in
the browser, reads keep working and writes fail — which looks like a broken
app, not a config error. The chart derives it from `ingress.hosts`, picking
each host's scheme from whether **that host** appears in a TLS block, so a
mixed-TLS ingress gets `https://a,http://b` rather than one scheme for both.

`OMNIBUS_SECURE_COOKIES` follows the same derivation, because a Secure cookie
is never sent over `http://` and login silently fails to stick. Mixing TLS and
plaintext hosts breaks login on one of them either way; `NOTES.txt` warns and
names them.

Set `publicOrigin` / `secureCookies` explicitly for LoadBalancer or NodePort
installs, where there is no ingress host to derive from.

### 3. Journal images are durable, and the image gets this wrong

| Mount | Contents | Back up |
|---|---|---|
| `/config` | SQLite DB, covers, **journal images** | **Yes** |
| `/cache` | thumbnails, HLS segments, KEPUB/converted output, export EPUBs, logs | No |
| library mounts | your books | your data |

The image defaults `OMNIBUS_DATA_DIR=/cache/data` and leaves
`OMNIBUS_JOURNAL_IMAGES_DIR` unset, so images embedded in journal entries land
under `/cache` — the volume every doc describes as safe to delete.
`.env.example` says the opposite of that directory: *"Durable user data — NOT a
regenerable cache."* The chart pins `OMNIBUS_JOURNAL_IMAGES_DIR` into
`/config`. Compose users still have the bug, tracked separately.

### 4. Libraries are passthrough, never provisioned

Each entry under `libraries` takes a **raw Kubernetes volume source** spliced
into the pod as-is, so PVC, NFS, hostPath and CSI all work:

```yaml
libraries:
  ebooks:
    enabled: true
    readOnly: false      # in-app uploads write here
    volume:
      persistentVolumeClaim: { claimName: my-ebooks }
  audiobooks:
    enabled: true
    readOnly: true
    volume:
      nfs: { server: 10.0.0.5, path: /export/audiobooks }
```

`volume` ships **empty**: Helm deep-merges values, so a default source would
merge with yours instead of being replaced, producing a two-source volume the
API server rejects. Enabling a library without one fails the render.

`libraryPathsFromEnv` publishes the mount paths as `EBOOK_LIBRARY_PATH` /
`AUDIOBOOK_LIBRARY_PATH`. The server re-seeds those on **every** boot, and the
hook writes *both* rows whenever either variable is present — so a chart
setting only one would silently clear the other. Set it `false` to manage
paths from the Settings UI instead.

## Smaller notes

**Security context.** The image's entrypoint remaps a PUID/PGID user as root
and drops privileges with `gosu`; Kubernetes does that with `fsGroup`, so the
chart bypasses it (`command: ["/app/server"]`) and runs unprivileged, non-root,
read-only-rootfs, all capabilities dropped — satisfying the `restricted` Pod
Security Standard. On **NFS-backed volumes `fsGroup` has no effect**: set
ownership on the server to match `runAsUser`/`runAsGroup`, or mount with a uid
mapping. `fsGroupChangePolicy: OnRootMismatch` keeps a large cover directory
from being recursively relabelled on every pod start.

**Probes** all hit `GET /api/_health`, unauthenticated by design (`auth::gate`
whitelists it). Startup allows 5 minutes — migrations and the `_norm` backfills
run before the first response. Liveness is slack on purpose: killing a pod
mid-transcode throws the work away.

**Ingress annotations** are yours to set; the chart adds none. Two worth
knowing on ingress-nginx, both noted in `values.yaml`: `proxy-body-size` must
match `config.maxUploadBytes` (nginx's 1 MB default 413s book uploads before
the app sees them), and `proxy-read-timeout` needs raising for audiobook
streams. Also note `/metrics` is **not** behind the auth gate — only `/api/*`
is — so block it at the edge if the ingress routes `/`.

**Rate limiting.** `config.trustForwardedFor` lets the login throttle key on
`X-Forwarded-For`, and defaults off: on a directly reachable Service any client
can spoof a fresh bucket. Note the app consults the header only when it has no
direct peer address (`client_ip` in `server/src/rate_limit.rs` prefers
`ConnectInfo`). Behind an ingress the peer is the controller pod, so depending
on whether the serving stack supplies `ConnectInfo` the throttle may end up
keyed on one shared bucket for all users — verify before relying on it.

## Operations

**Back up `/config`.** Nothing else in the release is recoverable. That PVC
carries `helm.sh/resource-policy: keep` so `helm uninstall` won't delete it
(`persistence.config.retain=false` to opt out). SQLite is live, so prefer a
volume snapshot or `sqlite3 .backup` against a stopped pod over a file copy.

**Upgrades** stop the old pod before starting the new one. Migrations run at
boot, are forward-only and checksummed — roll *forward* after a failed upgrade;
an older image against a migrated database will not work.

**Admin recovery**: set `config.initialAdmin=<username>`, upgrade, log in, then
clear it and upgrade again. It re-promotes on every boot while set.

## Values

See [`values.yaml`](values.yaml) — every key is commented in place. The ones
you will actually set:

| Key | Default | Notes |
|---|---|---|
| `libraries.{ebooks,audiobooks}.enabled` | `false` | Enable what you have |
| `libraries.*.volume` | `{}` | Raw volume source; required when enabled |
| `publicOrigin` | derived from ingress | Required for LoadBalancer/NodePort |
| `secureCookies` | derived from TLS | Force `"false"` for plain-http LAN installs |
| `persistence.config.size` | `10Gi` | DB + covers + journal images |
| `persistence.cache.size` | `20Gi` | Keep above the sum of the cache caps |
| `ingress.annotations` | `{}` | See "Smaller notes" |
| `secrets.*` | `""` | Hardcover / Google Books keys, SMTP; or `existingSecret` |

## Not covered

No autoscaling, PDB or HPA (meaningless at one replica), no NetworkPolicy, no
ServiceMonitor (`/metrics` is there if you want to scrape it — add your own
3-line ServiceMonitor), and no backup CronJob — use volume snapshots.

The chart is **not published to a Helm repo**, so it installs from a checkout
rather than `helm repo add`. The repo already publishes `gh-pages` for the
marketing site, so `chart-releaser` could serve it from the same branch if
that's ever wanted.
