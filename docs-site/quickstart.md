# Quickstart: Docker in 5 minutes with MinIO

You will run a local MinIO, create an encrypted carry repository in it, push an nginx page and an env var, and watch a running container pick up a change without being rebuilt.

Requirements: Docker, `openssl`, `curl`. Nothing else: the carry CLI itself runs from its image.

## 1. Start MinIO and create a bucket

```sh
docker network create carry-demo
docker run -d --name minio --network carry-demo -p 9000:9000 \
  -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
  chainguard/minio:latest server /data

# create bucket "carry" with a SigV4-signed PUT (no mc needed)
sleep 2
curl -s -o /dev/null -w '%{http_code}\n' -X PUT \
  --aws-sigv4 "aws:amz:us-east-1:s3" --user minioadmin:minioadmin \
  http://localhost:9000/carry          # 200
```

!!! note
    `minio/minio` is no longer pullable from Docker Hub; `chainguard/minio` is used by the repo's own `examples/docker-compose.yml`.

## 2. Make a key and a CLI alias

```sh
openssl rand -base64 32 > carry.key && chmod 600 carry.key

alias carry='docker run --rm -i --network carry-demo \
  -e CARRY_REPO=s3://carry/demo -e CARRY_S3_ENDPOINT=http://minio:9000 -e CARRY_S3_REGION=us-east-1 \
  -e AWS_ACCESS_KEY_ID=minioadmin -e AWS_SECRET_ACCESS_KEY=minioadmin \
  -e CARRY_KEY_FILE=/k -v "$PWD/carry.key:/k:ro" -v "$PWD:/w" -w /w \
  ghcr.io/muthuishere/carry:0.1.1'
```

On Linux you can instead download the static binary from the [releases page](https://github.com/muthuishere/carry/releases) (`carry-linux-amd64` / `carry-linux-arm64`, checksums in `SHA256SUMS`) and export the same variables.

## 3. Initialise and check the store

```sh
carry init
carry doctor
```

`doctor` checks reachability and that the store honours `If-None-Match` / `If-Match` conditional writes, which carry needs to commit safely.

## 4. Put some state

```sh
cat > index.html <<'HTML'
<h1>hello from carry v1</h1>
HTML
carry put web-1:/usr/share/nginx/html/index.html ./index.html
carry env set web-1 GREETING=hi
carry ls web-1
carry log web-1
```

## 5. Build an image with carry inside

```sh
cat > Dockerfile <<'EOF2'
FROM nginx:1.27-alpine
COPY --from=ghcr.io/muthuishere/carry:0.1.1 /carry /usr/local/bin/carry
ENTRYPOINT ["carry", "run", "--"]
CMD ["nginx", "-g", "daemon off;"]
EOF2
docker build -t carry-nginx .

docker run -d --name web --network carry-demo -p 8080:80 \
  -e CARRY_REPO=s3://carry/demo -e CARRY_S3_ENDPOINT=http://minio:9000 -e CARRY_S3_REGION=us-east-1 \
  -e AWS_ACCESS_KEY_ID=minioadmin -e AWS_SECRET_ACCESS_KEY=minioadmin \
  -e CARRY_KEY_FILE=/k -v "$PWD/carry.key:/k:ro" \
  -e CARRY_STATE=web-1 -e CARRY_POLL_SECS=5 -e CARRY_RELOAD_SIGNAL=HUP \
  carry-nginx

curl -s localhost:8080     # <h1>hello from carry v1</h1>
```

## 6. Change it live

```sh
echo '<h1>hello from carry v2</h1>' > index.html
carry put web-1:/usr/share/nginx/html/index.html ./index.html
sleep 6; curl -s localhost:8080     # v2
docker exec web carry status --json
```

## 7. Roll back

```sh
carry log web-1
carry rollback web-1 1       # makes version 1's tree current again, as a NEW version
sleep 6; curl -s localhost:8080
```

## 8. Prove fail-open

```sh
docker stop minio
docker restart web
curl -s localhost:8080       # still served from the last good local generation
docker exec web carry status --json   # "mode": "degraded"
docker start minio
```

!!! tip
    `docker restart` keeps the container filesystem (including `/carry`). A brand-new container with S3 down starts with an *empty* generation instead: still starts, but without your files. On Kubernetes an `emptyDir` gives the same "survives container restart, not pod replacement" behaviour.

## Clean up

```sh
docker rm -f web minio && docker network rm carry-demo
```

For a fuller playground (nginx + a Node app using on-demand tools), see [`examples/`](https://github.com/muthuishere/carry/tree/main/examples) and `examples/e2e.sh` in the repo.
