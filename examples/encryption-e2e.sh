#!/usr/bin/env bash
# carry encryption end-to-end: "all data is encrypted at rest, decrypted only at load".
#
#   1. MinIO + known plaintext markers written with the CLI: a file (small and a
#      chunked 6 MiB one) under a marker directory, an env key/value, and a tool whose
#      bytes contain a marker.
#   2. At rest: every object key and every object's bytes (via the S3 API, and MinIO's
#      raw data dir) are scanned; no marker and no path may appear. A positive control
#      writes the same markers to an UNENCRYPTED prefix and the scanner must find them.
#   3. Docker: 2 replicas restore and see plaintext; the key is not in the app env;
#      a wrong-key replica still starts (fail-open), reports degraded/bad key, sees nothing.
#   4. Kubernetes: a kind cluster, the Helm chart (deploy/helm/carry) injecting carry
#      with an init container into an unmodified alpine app, 2 replicas; same checks.
#
# Needs: Linux, docker, curl. kind/kubectl/helm/jq are downloaded into a temp dir if
# missing. Everything it creates (containers, network, kind cluster, images it pulled
# or built, temp dir) is removed on exit.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
CHART="$HERE/../deploy/helm/carry"
IMG=${IMG:-docker.io/muthuishere/carry:0.1.1}
MINIO=chainguard/minio:latest
MC=chainguard/minio-client:latest
N="carryenc$$"; NET="$N"; CLUSTER="$N"
W="$(mktemp -d)"; BIN="$W/bin"; mkdir -p "$BIN"; export PATH="$BIN:$PATH"
U=enc
P="$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"
head -c 32 /dev/urandom | base64 > "$W/key"; chmod 600 "$W/key"
head -c 32 /dev/urandom | base64 > "$W/wrong.key"; chmod 600 "$W/wrong.key"
R="$(head -c 8 /dev/urandom | od -An -tx1 | tr -d ' \n')"
M_FILE="mkfile${R}"; M_BIG="mkbig${R}"; M_ENVK="MKENVK${R}"; M_ENVV="mkenvv${R}"
M_TOOL="mktool${R}"; M_DIR="mkdir${R}"
MARKERS=("$M_FILE" "$M_BIG" "$M_ENVK" "$M_ENVV" "$M_TOOL" "$M_DIR")
STATE=e2e
PASS=0; FAIL=0
pass(){ echo "PASS  $*"; PASS=$((PASS+1)); }
fail(){ echo "FAIL  $*"; FAIL=$((FAIL+1)); }
check(){ local n="$1"; shift; if "$@"; then pass "$n"; else fail "$n"; fi; }
log(){ echo; echo "== $*"; }

IMAGES_BEFORE="$(docker images -q --no-trunc | sort -u)"
KIND_NET_BEFORE="$(docker network ls -q --filter name=^kind$)"
cleanup(){
  echo; echo "== cleanup"
  command -v kind >/dev/null && kind get clusters 2>/dev/null | grep -qx "$CLUSTER" && kind delete cluster --name "$CLUSTER" >/dev/null 2>&1
  docker rm -f $(docker ps -aq --filter "label=$N") >/dev/null 2>&1
  docker volume rm "$N-data" >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
  [ -z "$KIND_NET_BEFORE" ] && docker network rm kind >/dev/null 2>&1
  # The scan dirs are written by root inside containers.
  docker run --rm -v "$W:/w" alpine:3.20 rm -rf /w/dump /w/dump-plain >/dev/null 2>&1
  docker rmi -f "$N-app" "$N-installer:e2e" >/dev/null 2>&1
  for i in $(docker images -q --no-trunc | sort -u); do
    grep -qx "$i" <<<"$IMAGES_BEFORE" || docker rmi -f "$i" >/dev/null 2>&1
  done
  rm -rf "$W"
  echo "removed cluster, containers, network, images and $W"
}
trap cleanup EXIT

need(){ # name url [tar-member]
  command -v "$1" >/dev/null && return 0
  echo "downloading $1"
  if [ -n "${3:-}" ]; then curl -fsSL "$2" | tar xz -C "$W" "$3" && mv "$W/$3" "$BIN/$1"
  else curl -fsSL -o "$BIN/$1" "$2"; fi
  chmod +x "$BIN/$1"
}
need jq https://github.com/jqlang/jq/releases/download/jq-1.7.1/jq-linux-amd64
need kind https://kind.sigs.k8s.io/dl/v0.24.0/kind-linux-amd64
need kubectl https://dl.k8s.io/release/v1.31.2/bin/linux/amd64/kubectl
need helm https://get.helm.sh/helm-v3.16.3-linux-amd64.tar.gz linux-amd64/helm

ENVS=(-e CARRY_REPO=s3://carry/enc -e "CARRY_S3_ENDPOINT=http://$N-minio:9000" -e CARRY_S3_REGION=us-east-1
      -e "AWS_ACCESS_KEY_ID=$U" -e "AWS_SECRET_ACCESS_KEY=$P" -e CARRY_KEY_FILE=/run/secrets/carry-key)
cli(){ docker run --rm -i --label "$N" --network "$NET" "${ENVS[@]}" -v "$W/key:/run/secrets/carry-key:ro" \
         -e HOME=/work -e CARRY_TOOL_SIGNING_KEY_FILE=/work/sign.key -v "$W:/work" "$IMG" "$@"; }
# Unencrypted control repo, same markers: proves the scanner would catch plaintext.
plain(){ docker run --rm -i --label "$N" --network "$NET" -e CARRY_REPO=s3://carry/plain \
         -e "CARRY_S3_ENDPOINT=http://$N-minio:9000" -e CARRY_S3_REGION=us-east-1 \
         -e "AWS_ACCESS_KEY_ID=$U" -e "AWS_SECRET_ACCESS_KEY=$P" -e CARRY_NO_ENCRYPTION=1 \
         -e HOME=/work -v "$W:/work" "$IMG" "$@"; }
mc(){ docker run --rm -u 0 --label "$N" --network "$NET" -e "MC_HOST_m=http://$U:$P@$N-minio:9000" -v "$W:/w" "$MC" "$@"; }

log "MinIO"
docker network create "$NET" >/dev/null
docker volume create --label "$N" "$N-data" >/dev/null
docker run -d --label "$N" --name "$N-minio" --network "$NET" -u 0 -v "$N-data:/data" \
  -e "MINIO_ROOT_USER=$U" -e "MINIO_ROOT_PASSWORD=$P" "$MINIO" server /data >/dev/null
for i in $(seq 1 60); do mc mb -p m/carry >/dev/null 2>&1 && break; sleep 1; done
check "bucket created" mc ls m/carry

log "write markers with the CLI (encrypted repo + unencrypted control)"
mkdir -p "$W/in"
printf 'secret: %s\n' "$M_FILE" > "$W/in/app.conf"
{ head -c $((6*1024*1024)) /dev/urandom | base64 -w0; printf '\n%s\n' "$M_BIG"; } > "$W/in/big.bin"
# Incompressible bytes: zstd stores them verbatim, so in the UNENCRYPTED control repo
# a 64-byte slice must show up in the bucket. That proves the scanner sees plaintext.
head -c 300000 /dev/urandom | tr -d '\n\0' > "$W/in/rand.bin"
dd if="$W/in/rand.bin" of="$W/in/rand.pat" bs=1 skip=100000 count=64 2>/dev/null
printf '#!/bin/sh\necho %s\n' "$M_TOOL" > "$W/in/mtool"; chmod +x "$W/in/mtool"
PUB="$(cli tool keygen --out /work/sign.key --json | jq -r .result.public_key)"
check "tool signing key generated" test -n "$PUB" -a "$PUB" != null
for f in cli plain; do
  check "$f: put marker file"  bash -c "$(declare -f cli plain); $(declare -p ENVS N NET W IMG U P); $f put $STATE:/etc/$M_DIR/app.conf /work/in/app.conf --json | jq -e .ok >/dev/null"
  check "$f: put 6 MiB chunked file" bash -c "$(declare -f cli plain); $(declare -p ENVS N NET W IMG U P); $f put $STATE:/etc/$M_DIR/big.bin /work/in/big.bin --json | jq -e .ok >/dev/null"
  check "$f: put incompressible file" bash -c "$(declare -f cli plain); $(declare -p ENVS N NET W IMG U P); $f put $STATE:/etc/$M_DIR/rand.bin /work/in/rand.bin --json | jq -e .ok >/dev/null"
  check "$f: env set marker" bash -c "$(declare -f cli plain); $(declare -p ENVS N NET W IMG U P); $f env set $STATE $M_ENVK=$M_ENVV --json | jq -e .ok >/dev/null"
done
check "cli: publish marker tool" bash -c "$(declare -f cli); $(declare -p ENVS N NET W IMG); cli tool publish mtool@1.0.0 --file /work/in/mtool --platform linux/amd64 --json | jq -e .ok >/dev/null"
check "cli: tool add to state" bash -c "$(declare -f cli); $(declare -p ENVS N NET W IMG); cli tool add $STATE mtool@1.0.0 --json | jq -e .ok >/dev/null"
check "cli: repo reports encrypted" bash -c "$(declare -f cli); $(declare -p ENVS N NET W IMG); cli doctor --json | jq -e '.result.encrypted==true' >/dev/null"
check "cli: reads back plaintext with the key" bash -c "$(declare -f cli); $(declare -p ENVS N NET W IMG); cli get $STATE:/etc/$M_DIR/app.conf - | grep -q $M_FILE"
check "wrong key: CLI refuses with bad_key" bash -c "docker run --rm --network $NET ${ENVS[*]} -v $W/wrong.key:/run/secrets/carry-key:ro $IMG get $STATE:/etc/$M_DIR/app.conf - --json | jq -e '.error.code==\"bad_key\"' >/dev/null"

log "at rest: scan every object key and every object's bytes"
mc ls -r m/carry/enc > "$W/keys-enc.txt"; mc ls -r m/carry/plain > "$W/keys-plain.txt"
echo "encrypted repo: $(wc -l < "$W/keys-enc.txt") objects"; sed 's/.* //' "$W/keys-enc.txt" | sed 's#/[0-9a-f]\{16,\}$#/<id>#' | sort | uniq -c
mc mirror -q m/carry/enc /w/dump >/dev/null; mc mirror -q m/carry/plain /w/dump-plain >/dev/null
scan(){ # dir|- markers...; prints hits; dir "-" scans the key list on stdin
  local d="$1"; shift; local hits=0 m
  for m in "$@"; do
    if [ "$d" = - ]; then grep -q -- "$m" "$W/keys.tmp" && { echo "  key hit: $m"; hits=$((hits+1)); }
    else docker run --rm -v "$W:/w:ro" alpine:3.20 grep -rqa -- "$m" "/w/$d" && { echo "  byte hit: $m"; hits=$((hits+1)); }; fi
  done; return $hits
}
cp "$W/keys-enc.txt" "$W/keys.tmp"
nobj(){ docker run --rm -v "$W:/w:ro" alpine:3.20 find "/w/$1" -type f | wc -l; }
echo "downloaded $(nobj dump) encrypted + $(nobj dump-plain) plaintext-control objects"
check "scan has data: every encrypted object downloaded" test "$(nobj dump)" -eq "$(wc -l < "$W/keys-enc.txt")" -a "$(nobj dump)" -gt 0
check "no marker or path in any object key" scan - "${MARKERS[@]}" "/etc/" "app.conf" "big.bin"
check "no marker or path in any object body (S3 API)" scan dump "${MARKERS[@]}" "/etc/$M_DIR" "app.conf" "big.bin"
rawhits="$(docker run --rm -v "$N-data:/d:ro" alpine:3.20 grep -rlaE "$M_FILE|$M_BIG|$M_ENVV|$M_ENVK|$M_TOOL|$M_DIR" /d/carry/enc)"
check "no marker in MinIO's raw data dir (enc prefix)" test -z "$rawhits"
rawgrep(){ docker run --rm -e LC_ALL=C -v "$N-data:/d:ro" -v "$W:/w:ro" alpine:3.20 grep -rqaF -f /w/in/rand.pat "$1"; }
check "control: random slice IS in the unencrypted prefix (MinIO raw dir)" rawgrep /d/carry/plain
check "control: random slice IS in the unencrypted objects (S3 API)" rawgrep /w/dump-plain
norawgrep(){ ! rawgrep "$1"; }
check "random slice is NOT in the encrypted objects (S3 API)" norawgrep /w/dump
check "random slice is NOT in the encrypted prefix (MinIO raw dir)" norawgrep /d/carry/enc
mc cat m/carry/enc/config > "$W/config.json"
check "repo config: encrypted, data key only wrapped" jq -e '.encrypted==true and (.wrapped_keys|length)>=1' "$W/config.json" >/dev/null
check "repo config: master key not in it" bash -c "! grep -qF -- \"\$(cat $W/key)\" $W/config.json"

log "Docker: 2 replicas + 1 wrong-key replica"
mkdir -p "$W/app"
cat > "$W/app/Dockerfile" <<EOF
FROM alpine:3.20
COPY --from=$IMG /carry /usr/local/bin/carry
ENTRYPOINT ["carry","run","--"]
CMD ["sleep","infinity"]
EOF
docker build -q -t "$N-app" "$W/app" >/dev/null
for i in 1 2 3; do
  k="$W/key"; [ $i = 3 ] && k="$W/wrong.key"
  docker run -d --label "$N" --name "$N-d$i" --network "$NET" "${ENVS[@]}" -e CARRY_STATE=$STATE \
    -e CARRY_POLL_SECS=3 -e "CARRY_TOOL_PUBKEY=$PUB" -v "$k:/run/secrets/carry-key:ro" "$N-app" >/dev/null
done
sleep 5
appenv(){ docker exec "$N-d$1" sh -c 'tr "\0" "\n" < /proc/$(pgrep -x sleep)/environ'; }
KEYV="$(cat "$W/key")"
for i in 1 2; do
  check "docker d$i: marker file restored" bash -c "docker exec $N-d$i cat /etc/$M_DIR/app.conf | grep -q $M_FILE"
  check "docker d$i: chunked file restored intact" bash -c "[ \"\$(docker exec $N-d$i sha256sum /etc/$M_DIR/big.bin | cut -d' ' -f1)\" = \"\$(sha256sum $W/in/big.bin | cut -d' ' -f1)\" ]"
  check "docker d$i: env marker in app env" bash -c "$(declare -f appenv); N=$N; appenv $i | grep -qx $M_ENVK=$M_ENVV"
  check "docker d$i: tool runs, prints marker" bash -c "docker exec $N-d$i sh -c 'export PATH=/carry/bin:\$PATH; mtool' | grep -qx $M_TOOL"
  check "docker d$i: key/creds not in app env" bash -c "$(declare -f appenv); N=$N; ! appenv $i | grep -qE '^(CARRY_KEY|CARRY_KEY_FILE|AWS_SECRET_ACCESS_KEY)=' && ! appenv $i | grep -qF -- '$KEYV'"
done
check "docker d3 (wrong key): app running" bash -c "[ \"\$(docker inspect -f '{{.State.Running}}' $N-d3)\" = true ] && docker exec $N-d3 pgrep -x sleep >/dev/null"
docker exec "$N-d3" carry status --json | jq -c '.result | {mode, error}'
check "docker d3 (wrong key): status degraded, bad key" bash -c "docker exec $N-d3 carry status --json | jq -e '.result.mode==\"degraded\" and (.result.error|test(\"key\";\"i\"))' >/dev/null"
check "docker d3 (wrong key): no plaintext leaked" bash -c "! docker exec $N-d3 sh -c 'cat /etc/$M_DIR/app.conf; tr \"\\0\" \"\\n\" < /proc/\$(pgrep -x sleep)/environ; ls /carry/bin' 2>/dev/null | grep -qE '$M_FILE|$M_ENVV|mtool'"
docker rm -f "$N-d1" "$N-d2" "$N-d3" >/dev/null

log "Kubernetes: kind + Helm chart (init container injects carry into plain alpine)"
helm lint "$CHART" --set carry.repo=s3://carry/enc,carry.state=$STATE,carry.key.existingSecret=k,carry.s3Credentials.existingSecret=s >/dev/null
check "helm lint" test $? -eq 0
kind create cluster --name "$CLUSTER" --wait 120s >/dev/null 2>&1
check "kind cluster up" kubectl --context "kind-$CLUSTER" get nodes
K(){ kubectl --context "kind-$CLUSTER" "$@"; }
docker network connect kind "$N-minio"
MIP="$(docker inspect -f '{{(index .NetworkSettings.Networks "kind").IPAddress}}' "$N-minio")"
echo "MinIO on the kind network at $MIP"
docker build -q -t "$N-installer:e2e" --build-arg "CARRY_IMAGE=$IMG" "$CHART/installer" >/dev/null
# Single-platform archive: `kind load docker-image` breaks on Docker's containerd image store.
docker save --platform linux/amd64 -o "$W/images.tar" "$N-installer:e2e"
kind load image-archive --name "$CLUSTER" "$W/images.tar" >/dev/null
K create secret generic enc-key --from-file=key="$W/key" >/dev/null
K create secret generic bad-key --from-file=key="$W/wrong.key" >/dev/null
K create secret generic enc-s3 --from-literal=AWS_ACCESS_KEY_ID="$U" --from-literal=AWS_SECRET_ACCESS_KEY="$P" >/dev/null
inst(){ # release keysecret replicas
  helm --kube-context "kind-$CLUSTER" install "$1" "$CHART" --wait --timeout 180s \
    --set replicaCount="$3" --set carry.repo=s3://carry/enc --set carry.state=$STATE \
    --set carry.endpoint="http://$MIP:9000" --set carry.pollSecs=3 --set carry.toolPubkey="$PUB" \
    --set carry.key.existingSecret="$2" --set carry.s3Credentials.existingSecret=enc-s3 \
    --set carry.inject.installerImage.repository="$N-installer" --set carry.inject.installerImage.tag=e2e \
    --set carry.inject.installerImage.pullPolicy=Never \
    --set app.image.repository=alpine --set app.image.tag=3.20 --set app.image.pullPolicy=IfNotPresent \
    --set-json 'app.command=["sleep","infinity"]' --set-json 'app.ports=[]' >/dev/null
}
check "helm install good (2 replicas)" inst good enc-key 2
check "helm install wrong-key (1 replica)" inst bad bad-key 1
if [ "$FAIL" -gt 0 ]; then K get pods -o wide; K describe pods | grep -A12 -E '^(Events|  [a-z-]+:$)' | tail -60; fi
PODS=($(K get pods -l app.kubernetes.io/instance=good -o jsonpath='{.items[*].metadata.name}'))
BADPOD="$(K get pods -l app.kubernetes.io/instance=bad -o jsonpath='{.items[0].metadata.name}')"
check "2 good pods" test "${#PODS[@]}" -eq 2
check "key is not an env var in the pod spec" bash -c "! kubectl --context kind-$CLUSTER get deploy good-carry -o json | jq -e '.spec.template.spec.containers[0].env[] | select(.name==\"CARRY_KEY\")' >/dev/null"
kx(){ kubectl --context "kind-$CLUSTER" exec "$1" -c app -- sh -c "$2"; }
for p in "${PODS[@]}"; do
  check "k8s $p: marker file restored" bash -c "$(declare -f kx); CLUSTER=$CLUSTER; kx $p 'cat /etc/$M_DIR/app.conf' | grep -q $M_FILE"
  check "k8s $p: chunked file intact" bash -c "$(declare -f kx); CLUSTER=$CLUSTER; [ \"\$(kx $p 'sha256sum /etc/$M_DIR/big.bin' | cut -d' ' -f1)\" = \"\$(sha256sum $W/in/big.bin | cut -d' ' -f1)\" ]"
  check "k8s $p: env marker in app env" bash -c "$(declare -f kx); CLUSTER=$CLUSTER; kx $p 'tr \"\\0\" \"\\n\" < /proc/\$(pgrep -x sleep)/environ' | grep -qx $M_ENVK=$M_ENVV"
  check "k8s $p: tool runs, prints marker" bash -c "$(declare -f kx); CLUSTER=$CLUSTER; kx $p 'export PATH=/carry/bin:\$PATH; mtool' | grep -qx $M_TOOL"
  check "k8s $p: key not in /proc/1 or app environ" bash -c "$(declare -f kx); CLUSTER=$CLUSTER; ! kx $p 'cat /proc/1/environ /proc/\$(pgrep -x sleep)/environ' | tr '\0' '\n' | grep -qF -- '$KEYV'"
  check "k8s $p: no CARRY_KEY*/AWS secret in app env" bash -c "$(declare -f kx); CLUSTER=$CLUSTER; ! kx $p 'tr \"\\0\" \"\\n\" < /proc/\$(pgrep -x sleep)/environ' | grep -qE '^(CARRY_KEY|CARRY_KEY_FILE|AWS_SECRET_ACCESS_KEY|AWS_ACCESS_KEY_ID)='"
  check "k8s $p: status ok" bash -c "$(declare -f kx); CLUSTER=$CLUSTER; kx $p '/opt/carry/carry status --json' | jq -e '.result.mode==\"ok\"' >/dev/null"
done
check "k8s wrong key: pod Running (fail-open)" bash -c "[ \"\$(kubectl --context kind-$CLUSTER get pod $BADPOD -o jsonpath='{.status.phase}')\" = Running ] && $(declare -f kx); CLUSTER=$CLUSTER; kx $BADPOD 'pgrep -x sleep' >/dev/null"
kx "$BADPOD" '/opt/carry/carry status --json' | jq -c '.result | {mode, error}'
check "k8s wrong key: status degraded, bad key" bash -c "$(declare -f kx); CLUSTER=$CLUSTER; kx $BADPOD '/opt/carry/carry status --json' | jq -e '.result.mode==\"degraded\" and (.result.error|test(\"key\";\"i\"))' >/dev/null"
check "k8s wrong key: no plaintext leaked" bash -c "$(declare -f kx); CLUSTER=$CLUSTER; ! kx $BADPOD 'cat /etc/$M_DIR/app.conf; tr \"\\0\" \"\\n\" < /proc/\$(pgrep -x sleep)/environ; ls /carry/bin' 2>/dev/null | grep -qE '$M_FILE|$M_ENVV|mtool'"

log "live update reaches pods; bucket still clean"
M_NEW="mknew${R}"; printf 'secret: %s\n' "$M_NEW" > "$W/in/app.conf"
cli put $STATE:/etc/$M_DIR/app.conf /work/in/app.conf --json >/dev/null
sleep 6
check "k8s ${PODS[0]}: picks up new version without restart" bash -c "$(declare -f kx); CLUSTER=$CLUSTER; kx ${PODS[0]} 'cat /etc/$M_DIR/app.conf' | grep -q $M_NEW"
docker run --rm -v "$W:/w" alpine:3.20 rm -rf /w/dump >/dev/null; mc mirror -q m/carry/enc /w/dump >/dev/null
check "re-downloaded objects after update" test "$(nobj dump)" -gt 0
check "no new marker in any object body after update" scan dump "$M_NEW" "${MARKERS[@]}"

echo; echo "passed: $PASS  failed: $FAIL"
[ "$FAIL" -eq 0 ]
