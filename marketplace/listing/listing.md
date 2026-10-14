# carry - shared listing copy (reuse on every marketplace)

**Title:** carry - encrypted config, env and tools for containers from S3

**Short description (<=150 chars):**
Restore config files, env and CLI tools into Docker and Kubernetes containers from a client-side-encrypted S3 bucket.

**Long description:**
Containers lose everything added after the image was built. carry restores config files, environment defaults and tools from any S3-compatible bucket (AWS S3, GCS interop, Azure via S3 gateway, MinIO, R2, DigitalOcean Spaces, Linode Object Storage, OCI) before your app starts. Every change made with the `carry` CLI is committed back as a new, versioned, client-side-encrypted (XChaCha20-Poly1305) snapshot - the bucket only ever sees ciphertext.

- One static Rust binary, under 10 MB, `FROM scratch` image, multi-arch (amd64/arm64)
- No privileges, no CSI driver, no sidecar required
- Versioned: `log`, `diff`, `rollback`
- Fail-open: if S3 is slow or down, the app starts from the last good local copy within 2 s
- Safe with many replicas: commits use S3 conditional writes
- Tools on demand from a signed tool store (`gh`, `kubectl`, `jq`, ...)

**Categories:** DevOps / Developer Tools; Storage; Security (secondary); Configuration Management
**Keywords:** kubernetes, docker, configuration, secrets, s3, encryption, gitops, devops, init container, helm
**License / EULA:** Apache License 2.0 - https://github.com/muthuishere/carry/blob/main/LICENSE
**Support:** https://github.com/muthuishere/carry/issues (community, best-effort)
**Homepage / docs:** https://github.com/muthuishere/carry
**Images:** docker.io/muthuishere/carry:0.1.1, ghcr.io/muthuishere/carry:0.1.1
**Publisher:** Deemwar (Muthukumaran Navaneethakrishnan)
**Logo:** ../logo.svg (export 256x256 / 512x512 PNG as each portal requires)

## Usage (generic Kubernetes / Helm)
```sh
kubectl create secret generic carry-s3 \
  --from-literal=AWS_ACCESS_KEY_ID=... --from-literal=AWS_SECRET_ACCESS_KEY=...
kubectl create secret generic carry-key --from-file=carry-key=./carry.key
helm install carry ./deploy/helm/carry \
  --set repo=s3://my-bucket/carry --set s3.endpoint=https://s3.eu-central-1.amazonaws.com
```
(Check value names against deploy/helm/carry/values.yaml - the chart is still being written.)

## Usage (Docker)
```dockerfile
COPY --from=ghcr.io/muthuishere/carry:0.1.1 /carry /usr/local/bin/carry
ENTRYPOINT ["carry", "run", "--"]
```
