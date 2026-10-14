# Docker guide

## Add carry to an image

carry ships as a `FROM scratch` image that contains only `/carry` (plus CA certificates). Copy the binary into your own image and make `carry run --` the entrypoint:

```dockerfile
FROM node:22-slim
COPY --from=ghcr.io/muthuishere/carry:0.1.1 /carry /usr/local/bin/carry
ENTRYPOINT ["carry", "run", "--"]
CMD ["node", "server.js"]
```

- Images: `ghcr.io/muthuishere/carry:0.1.1` and `docker.io/muthuishere/carry:0.1.1` (multi-arch: `linux/amd64`, `linux/arm64`).
- The binary is static (musl, rustls), so it works on Alpine, Debian, distroless, anything Linux.
- Pin the tag. Copying from `:latest` makes your builds non-reproducible.

!!! tip "If your image already has an ENTRYPOINT"
    Wrap it: `ENTRYPOINT ["carry", "run", "--", "/docker-entrypoint.sh"]`. Everything after `--` is the command carry supervises.

## Configure with environment variables

| Variable | Required | Meaning |
|---|---|---|
| `CARRY_REPO` | yes (else carry is disabled) | `s3://bucket/prefix` |
| `CARRY_STATE` | strongly recommended | State to restore. Default: `$HOSTNAME`, then `default` |
| `CARRY_S3_ENDPOINT` | non-AWS | e.g. `http://minio:9000`, `https://fsn1.your-objectstorage.com` |
| `CARRY_S3_REGION` | usually | e.g. `us-east-1` |
| `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` | yes | S3 credentials (`AWS_SESSION_TOKEN` is stripped from the app too) |
| `CARRY_KEY` or `CARRY_KEY_FILE` | yes for encrypted repos | Master key: base64 of 32 bytes, or a file holding it |
| `CARRY_ROOT` | no | Local restore root, default `/carry` |
| `CARRY_POLL_SECS` | no | Poll interval, default `30` |
| `CARRY_RESTORE_BUDGET_MS` | no | Fail-open budget at start, default `2000` |
| `CARRY_RELOAD_SIGNAL` | no | Signal sent to the app after a new version lands, e.g. `HUP`, `USR1`, `15` |
| `CARRY_TOOL_PUBKEY` | if you use tools | Public key from `carry tool keygen` |
| `CARRY_TIMEOUT` | no | Store timeout in seconds, default `30` |

```sh
docker run -d --name web \
  -e CARRY_REPO=s3://my-bucket/carry \
  -e CARRY_STATE=web-1 \
  -e CARRY_S3_ENDPOINT=https://s3.eu-central-1.amazonaws.com \
  -e CARRY_S3_REGION=eu-central-1 \
  -e AWS_ACCESS_KEY_ID -e AWS_SECRET_ACCESS_KEY \
  -e CARRY_KEY_FILE=/run/secrets/carry-key -v "$PWD/carry.key:/run/secrets/carry-key:ro" \
  -e CARRY_RELOAD_SIGNAL=HUP \
  my-nginx
```

Prefer `CARRY_KEY_FILE` with a mounted secret over `CARRY_KEY` in `docker run -e`, which ends up in `docker inspect`.

## What the app sees

- **Files** you `carry put` appear at their absolute paths as symlinks into `/carry/current/files/...`. The parent directories are created if missing.
- **Env** from `carry env set` is applied at start. Your container's own env wins over stored defaults. Env changes need a **container restart** to reach the app (a running process's environment cannot be changed); file changes do not.
- **Tools** listed with `carry tool add` are on `PATH` via `/carry/bin`.
- **Secrets are stripped**: `CARRY_KEY`, `CARRY_KEY_FILE`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `CARRY_TOOL_SIGNING_KEY`, `CARRY_TOOL_SIGNING_KEY_FILE` are removed from the app's environment. Tool launchers read them from `/carry/.launcher.env` (mode 0600) instead; see [Security](../security.md#what-the-app-can-see).

## Persisting `/carry`

`/carry` holds the last good generation. With no volume, it lives in the container layer: it survives `docker restart` but not `docker rm`. If you want a re-created container to come up with the last good config even when S3 is down at that moment, mount a volume:

```sh
docker run -v carry-web-1:/carry ...
```

## Reload behaviour

With `CARRY_RELOAD_SIGNAL=HUP`, nginx, HAProxy, many Go daemons and others re-read their config. Apps that do not support reload can leave it unset and read files on each request, or you can restart them.

## Running as a non-root user

carry needs to write `CARRY_ROOT` and create symlinks at the paths you put. Either run as root, or make sure the user owns `/carry` and the target directories (e.g. put files under `/home/app/...`, or set `CARRY_ROOT=/tmp/carry`). Paths it cannot link are skipped with a warning; the app still starts.

## docker compose

```yaml
services:
  web:
    image: my-nginx
    environment:
      CARRY_REPO: s3://carry/demo
      CARRY_S3_ENDPOINT: http://minio:9000
      CARRY_S3_REGION: us-east-1
      AWS_ACCESS_KEY_ID: ${AWS_ACCESS_KEY_ID}
      AWS_SECRET_ACCESS_KEY: ${AWS_SECRET_ACCESS_KEY}
      CARRY_KEY: ${CARRY_KEY}
      CARRY_STATE: web-1
      CARRY_POLL_SECS: 5
      CARRY_RELOAD_SIGNAL: HUP
```

A complete MinIO + nginx + Node setup is in [`examples/docker-compose.yml`](https://github.com/muthuishere/carry/blob/main/examples/docker-compose.yml).

## Checking a container

```sh
docker exec web carry status --json
```

```json
{"ok": true, "result": {"state": "web-1", "mode": "ok", "version": 7, "root": "/carry",
  "last_restore_ms": 143, "error": null, "tools": [{"name": "jq", "status": "ready"}]}}
```
