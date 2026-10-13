#!/usr/bin/env bash
# carry live demo: 3 dev-box replicas + 2 nginx replicas sharing state in MinIO,
# real tools (jq, kubectl, gh) published once and installed on demand.
# Uses only the published image ghcr.io/muthuishere/carry:0.1.1. Cleans up everything on exit.
set -uo pipefail
IMG=ghcr.io/muthuishere/carry:0.1.1
N="carrydemo$$"
NET="$N"
W="$(mktemp -d)"
U=demo
P="$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"
KEY="$(head -c 32 /dev/urandom | base64)"
PASS=0; FAIL=0
pass(){ echo "PASS  $*"; PASS=$((PASS+1)); }
fail(){ echo "FAIL  $*"; FAIL=$((FAIL+1)); }
check(){ local name="$1"; shift; if "$@"; then pass "$name"; else fail "$name"; fi; }
log(){ echo; echo "== $*"; }

cleanup(){
  docker rm -f $(docker ps -aq --filter "label=$N") >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
  docker rmi -f "$N-devbox" "$N-web" >/dev/null 2>&1
  rm -rf "$W"
}
trap cleanup EXIT

ENVS=(-e CARRY_REPO=s3://carry/demo -e "CARRY_S3_ENDPOINT=http://$N-minio:9000" -e CARRY_S3_REGION=us-east-1
      -e "AWS_ACCESS_KEY_ID=$U" -e "AWS_SECRET_ACCESS_KEY=$P" -e "CARRY_KEY=$KEY")
cli(){ docker run --rm -i --label "$N" --network "$NET" "${ENVS[@]}" -e HOME=/work \
         -e CARRY_TOOL_SIGNING_KEY_FILE=/work/sign.key -v "$W:/work" "$IMG" "$@"; }

log "MinIO + bucket"
docker network create "$NET" >/dev/null
docker run -d --label "$N" --name "$N-minio" --network "$NET" -u 0 \
  -e "MINIO_ROOT_USER=$U" -e "MINIO_ROOT_PASSWORD=$P" chainguard/minio:latest server /data >/dev/null
for i in $(seq 1 60); do
  code=$(docker run --rm --network "$NET" curlimages/curl:8.11.1 -s -o /dev/null -w '%{http_code}' -X PUT \
    --aws-sigv4 "aws:amz:us-east-1:s3" --user "$U:$P" "http://$N-minio:9000/carry" 2>/dev/null)
  [[ "$code" == 200 || "$code" == 409 ]] && break; sleep 1
done
echo "bucket http $code"

log "download real tools (linux/amd64)"
cd "$W"
curl -fsSL -o jq https://github.com/jqlang/jq/releases/download/jq-1.7.1/jq-linux-amd64
curl -fsSL -o kubectl https://dl.k8s.io/release/v1.31.2/bin/linux/amd64/kubectl
curl -fsSL https://github.com/cli/cli/releases/download/v2.62.0/gh_2.62.0_linux_amd64.tar.gz | tar xz
cp gh_2.62.0_linux_amd64/bin/gh gh && rm -rf gh_2.62.0_linux_amd64
chmod +x jq kubectl gh
ls -la jq kubectl gh | awk '{print $5, $9}'
cd - >/dev/null

log "publish tools once (signed)"
PUB="$(cli tool keygen --out /work/sign.key --json | jq -r .result.public_key)"
check "keygen gives a public key" test -n "$PUB" -a "$PUB" != null
for t in jq@1.7.1:jq kubectl@1.31.2:kubectl gh@2.62.0:gh; do
  ref="${t%%:*}"; f="${t##*:}"
  check "publish $ref" bash -c "$(declare -f cli); $(declare -p ENVS N NET W IMG); cli tool publish $ref --file /work/$f --platform linux/amd64 --json | jq -e .ok >/dev/null"
done
cli tool ls --json | jq -c '.result.tools[] | {name, versions}'

log "configure state 'dev' from the CLI"
printf 'team: platform\nversion: 1\n' > "$W/team.yaml"
cli put dev:/etc/dev/team.yaml /work/team.yaml --json | jq -c .result
cli env set dev REGION=eu-central LOG_LEVEL=info --json | jq -c .result
cli tool add dev jq@1.7.1 --json | jq -c .result
cli tool add dev gh@2.62.0 --json | jq -c .result

log "build tiny app images that COPY carry from the published image"
mkdir -p "$W/devbox" "$W/web"
cat > "$W/devbox/Dockerfile" <<EOF
FROM alpine:3.20
COPY --from=$IMG /carry /usr/local/bin/carry
ENTRYPOINT ["carry","run","--"]
CMD ["sleep","infinity"]
EOF
cat > "$W/web/Dockerfile" <<EOF
FROM nginx:1.27-alpine
COPY --from=$IMG /carry /usr/local/bin/carry
ENTRYPOINT ["carry","run","--"]
CMD ["nginx","-g","daemon off;"]
EOF
docker build -q -t "$N-devbox" "$W/devbox" >/dev/null
docker build -q -t "$N-web" "$W/web" >/dev/null

log "start 3 dev-box replicas"
for i in 1 2 3; do
  docker run -d --label "$N" --name "$N-dev$i" --network "$NET" "${ENVS[@]}" \
    -e CARRY_STATE=dev -e CARRY_POLL_SECS=3 -e "CARRY_TOOL_PUBKEY=$PUB" -e "HOSTNAME=dev$i" "$N-devbox" >/dev/null
done
sleep 4
appenv(){ docker exec "$N-dev$1" sh -c 'tr "\0" "\n" < /proc/$(pgrep -x sleep)/environ'; }
for i in 1 2 3; do
  check "dev$i: config restored" bash -c "docker exec $N-dev$i cat /etc/dev/team.yaml | grep -q 'version: 1'"
  check "dev$i: env REGION applied to app" bash -c "$(declare -f appenv); N=$N; appenv $i | grep -qx REGION=eu-central"
  check "dev$i: secrets not in app env" bash -c "$(declare -f appenv); N=$N; ! appenv $i | grep -qE '^(CARRY_KEY|AWS_SECRET_ACCESS_KEY)='"
done

log "tools install on first use, in each replica"
X(){ docker exec "$N-dev$1" sh -c "export PATH=/carry/bin:\$PATH; $2"; }
check "dev1: jq runs (first use downloads)" bash -c "$(declare -f X); N=$N; X 1 'echo {\\\"a\\\":7} | jq .a' | grep -qx 7"
check "dev2: gh --version" bash -c "$(declare -f X); N=$N; X 2 'gh --version' | grep -q 'gh version 2.62.0'"
check "dev3: jq + gh" bash -c "$(declare -f X); N=$N; X 3 'jq --version && gh --version' | grep -q 'jq-1.7.1'"
docker exec "$N-dev1" carry status --json | jq -c '.result | {mode, version, tools}'

log "install a new app (kubectl) while replicas are running"
cli tool add dev kubectl@1.31.2 --json | jq -c .result
sleep 5
for i in 1 2 3; do
  check "dev$i: kubectl available without restart" bash -c "$(declare -f X); N=$N; X $i 'kubectl version --client' | grep -q 'v1.31.2'"
done

log "update config from the CLI, all replicas follow"
printf 'team: platform\nversion: 2\n' > "$W/team.yaml"
cli put dev:/etc/dev/team.yaml /work/team.yaml --json | jq -c .result
sleep 5
for i in 1 2 3; do
  check "dev$i: sees version 2" bash -c "docker exec $N-dev$i cat /etc/dev/team.yaml | grep -q 'version: 2'"
done

log "3 replicas write at the same time (compare-and-swap)"
for i in 1 2 3; do
  ( echo "written by dev$i" | docker exec -i "$N-dev$i" carry put "dev:/etc/dev/from-dev$i.txt" - --json >"$W/w$i.json" 2>&1 ) &
done
wait
for i in 1 2 3; do check "dev$i: concurrent write committed" jq -e .ok "$W/w$i.json" >/dev/null; done
vers="$(jq -r .result.version "$W"/w*.json | sort -n | tr '\n' ' ')"
echo "versions: $vers"
check "concurrent writes got distinct versions" bash -c "[ \$(echo $vers | tr ' ' '\n' | sort -u | wc -l) -eq 3 ]"
cli log dev --json | jq -r '.result.versions[] | "v\(.version) \(.op) \(.args|join(" ")) by \(.by)"'

log "rollback to the version-1 config"
v1="$(cli log dev --json | jq -r '[.result.versions[] | select(.op=="put" and (.args|index("/etc/dev/team.yaml")))] | last | .version')"
cli rollback dev "$v1" --json | jq -c .result
sleep 5
check "rollback restores the whole state (tools list too)" bash -c "docker exec $N-dev2 sh -c 'export PATH=/carry/bin:\$PATH; jq --version' 2>&1 | grep -q 'not listed'"
cli tool add dev jq@1.7.1 --json | jq -c .result
sleep 5
check "dev2: back on version 1 after rollback" bash -c "docker exec $N-dev2 cat /etc/dev/team.yaml | grep -q 'version: 1'"

log "explicit delete from the CLI; a local rm does nothing"
docker exec "$N-dev1" rm -f /etc/dev/team.yaml
check "local rm leaves the repo untouched" bash -c "$(declare -f cli); $(declare -p ENVS N NET W IMG); cli get dev:/etc/dev/team.yaml - | grep -q version"

log "web example: 2 nginx replicas serve the same page"
printf '<h1>hello from carry v1</h1>\n' > "$W/index.html"
cli put web:/usr/share/nginx/html/index.html /work/index.html --json | jq -c .result
for i in 1 2; do
  docker run -d --label "$N" --name "$N-web$i" --network "$NET" "${ENVS[@]}" -e CARRY_STATE=web -e CARRY_POLL_SECS=3 "$N-web" >/dev/null
done
sleep 4
web(){ docker run --rm --network "$NET" curlimages/curl:8.11.1 -fsS "http://$N-web$1/" 2>/dev/null; }
for i in 1 2; do check "web$i serves v1" bash -c "$(declare -f web); N=$N; NET=$NET; web $i | grep -q 'carry v1'"; done
printf '<h1>hello from carry v2</h1>\n' > "$W/index.html"
cli put web:/usr/share/nginx/html/index.html /work/index.html --json >/dev/null
sleep 5
for i in 1 2; do check "web$i serves v2 without restart" bash -c "$(declare -f web); N=$N; NET=$NET; web $i | grep -q 'carry v2'"; done

log "S3 goes down"
docker stop "$N-minio" >/dev/null
docker restart "$N-dev3" >/dev/null
sleep 4
check "dev3 restarted with S3 down: still running" bash -c "[ \"\$(docker inspect -f '{{.State.Running}}' $N-dev3)\" = true ]"
check "dev3: config still there" bash -c "docker exec $N-dev3 cat /etc/dev/team.yaml | grep -q version"
check "dev3: cached jq still runs offline" bash -c "$(declare -f X); N=$N; X 3 'echo [1,2] | jq length' | grep -qx 2"
check "dev3: status says degraded" bash -c "docker exec $N-dev3 carry status --json | jq -e '.result.mode==\"degraded\"' >/dev/null"
check "web1 keeps serving" bash -c "$(declare -f web); N=$N; NET=$NET; web 1 | grep -q 'carry v2'"

echo; echo "passed: $PASS  failed: $FAIL"
