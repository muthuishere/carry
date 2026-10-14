# carry Helm chart

Wraps any app in `carry run --`: before the app starts, carry restores its config files, env and tools from an encrypted S3 repo, then polls for new versions. Everything in the bucket is ciphertext (XChaCha20-Poly1305 + zstd); it is decrypted only inside the pod, at load.

## Install

```sh
kubectl create secret generic web-carry-key --from-literal=key="$(openssl rand -base64 32)"
kubectl create secret generic web-carry-s3 \
  --from-literal=AWS_ACCESS_KEY_ID=YOUR_KEY_HERE --from-literal=AWS_SECRET_ACCESS_KEY=YOUR_KEY_HERE

helm install web deploy/helm/carry \
  --set carry.repo=s3://my-bucket/carry \
  --set carry.state=web \
  --set carry.endpoint=https://fsn1.your-objectstorage.com \
  --set carry.key.existingSecret=web-carry-key \
  --set carry.s3Credentials.existingSecret=web-carry-s3 \
  --set app.image.repository=nginx --set app.image.tag=1.27-alpine \
  --set-json 'app.command=["nginx","-g","daemon off;"]'
```

Use the same key on your machine (`CARRY_KEY_FILE` or a profile) to change state: `carry put web:/etc/nginx/conf.d/default.conf ./default.conf`.

## How carry gets into the pod

| `carry.inject.mode` | what happens |
|---|---|
| `initContainer` (default) | An init container from `carry.inject.installerImage` copies the static binary into an `emptyDir` at `/opt/carry`. The app image is unchanged. |
| `image` | The app image already contains carry (`COPY --from=docker.io/muthuishere/carry:0.1.1 /carry /usr/local/bin/carry`). Set `carry.inject.binary` if it lives elsewhere. |

The published carry image is `FROM scratch` (no shell, no `cp`), so it cannot copy itself. The installer image is that binary plus busybox, built from [`installer/Dockerfile`](installer/Dockerfile) and published as `ghcr.io/muthuishere/carry-installer:<version>` by the release workflow. To build your own:

```sh
docker build -t YOUR_REGISTRY/carry-installer:0.1.1 deploy/helm/carry/installer
```

Either way the container command becomes `<carry> run -- <app.command...>`, so `app.command` must spell out the full app command (the image's ENTRYPOINT/CMD are replaced).

## Secrets

- **Master key**: a Secret mounted read-only (mode 0400) at `carry.key.mountPath`, found via `CARRY_KEY_FILE`. It is never an env var, so it is not in `/proc/<pid>/environ`, `kubectl describe` or crash dumps.
- **S3 credentials**: `secretKeyRef` env vars. `carry run` strips them, and `CARRY_KEY_FILE`, from the app's environment.
- Either point at existing Secrets (`existingSecret`, recommended) or let the chart create them from `carry.key.value` / `carry.s3Credentials.*`.
- The app runs in the same container, so it can read the mounted key file and `<root>/.launcher.env`. carry keeps secrets out of the environment and the bucket, not away from a hostile app.

## Fail-open

Restore has a `restoreBudgetMs` budget (default 2000). If S3 is down or the key is wrong, the app still starts, with the last good generation or an empty one, and `carry status --json` reports `"mode": "degraded"`. Probe the app, never carry.

## Values

| key | default | |
|---|---|---|
| `replicaCount` | `2` | replicas sharing one state |
| `app.image.*` | `nginx:1.27-alpine` | the app image |
| `app.command` | nginx | full app command (required) |
| `app.env`, `app.ports`, `app.readinessProbe`, `app.resources`, `app.securityContext`, `app.volumes`, `app.volumeMounts` | | passed through |
| `carry.inject.mode` | `initContainer` | `initContainer` or `image` |
| `carry.inject.installerImage.*` | `ghcr.io/muthuishere/carry-installer:0.1.1` | init container image |
| `carry.inject.binary` | `/opt/carry/carry` or `/usr/local/bin/carry` | path of carry in the app container |
| `carry.repo` | | `s3://bucket/prefix` (required) |
| `carry.state` | | state name (required) |
| `carry.endpoint` | | S3-compatible endpoint; empty for AWS |
| `carry.region` | `us-east-1` | |
| `carry.pollSecs` | `30` | how often replicas look for new versions |
| `carry.restoreBudgetMs` | `2000` | fail-open budget |
| `carry.reloadSignal` | | e.g. `HUP`, sent to the app after a new version |
| `carry.toolPubkey` | | public key from `carry tool keygen` |
| `carry.root`, `carry.rootSizeLimit` | `/carry`, `512Mi` | emptyDir for generations and tool cache |
| `carry.key.existingSecret`, `.secretKey`, `.value`, `.mountPath` | `""`, `key`, `""`, `/var/run/carry-key` | master key |
| `carry.s3Credentials.existingSecret`, `.accessKeyIdKey`, `.secretAccessKeyKey`, `.accessKeyId`, `.secretAccessKey` | | S3 credentials |

## Reuse in your own chart

The pod pieces are named templates: `carry.initContainers`, `carry.command`, `carry.env`, `carry.volumeMounts`, `carry.volumes`. Add this chart as a dependency and `include` them in your own Deployment, with a `carry:` block in your values.

## Proof

[`examples/encryption-e2e.sh`](../../../examples/encryption-e2e.sh) writes known markers, scans every object in the bucket for them, then restores them in Docker and in a kind cluster through this chart, including a wrong-key run.
