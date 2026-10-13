#!/usr/bin/env bash
# End-to-end test for carry against MinIO, driven entirely from outside the binary.
# Implements the testable scenarios of openspec/changes/carry-v0-1/specs/
# {distribution,container-runtime}/spec.md (plus tool-store first use / bad signature).
#
# Usage: examples/e2e.sh [--keep] [--no-build]
#   --keep      leave MinIO, containers and the work dir running for inspection
#   --no-build  reuse existing carry:dev / example images
# Env: CARRY_IMAGE (default carry:dev), WEB_PORT (18080), NODE_PORT (13000)
# Needs on the host: docker (with compose), curl, jq, openssl.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
KEEP=0
BUILD=1
for a in "$@"; do
  case "$a" in
    --keep) KEEP=1 ;;
    --no-build) BUILD=0 ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "unknown arg: $a" >&2; exit 2 ;;
  esac
done

for bin in docker curl jq openssl; do
  command -v "$bin" >/dev/null || { echo "missing required tool: $bin" >&2; exit 2; }
done

CARRY_IMAGE="${CARRY_IMAGE:-carry:dev}"
WEB_PORT="${WEB_PORT:-18080}"
NODE_PORT="${NODE_PORT:-13000}"
POLL=2
BUDGET_MS=2000
RUN_ID="e2e$$"
export CARRY_NETWORK="carry-${RUN_ID}"
export COMPOSE_PROJECT_NAME="carry-${RUN_ID}"
export MINIO_ROOT_USER="${MINIO_ROOT_USER:-carry-e2e}" MINIO_ROOT_PASSWORD="${MINIO_ROOT_PASSWORD:-carry-e2e-local-only}"
export MINIO_PORT=0 MINIO_CONSOLE_PORT=0   # host ports unused; random to avoid clashes
COMPOSE=(docker compose -f "$HERE/docker-compose.yml" -p "carry-${RUN_ID}")
REPO="s3://carry/${RUN_ID}"
NGINX_IMG="carry-example-nginx:dev"
NODE_IMG="carry-example-node:dev"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/carry-e2e.XXXXXX")"
CARRY_KEY="$(openssl rand -base64 32)"
ARCH="$(docker version --format '{{.Server.Arch}}')"
case "$ARCH" in amd64|arm64) ;; aarch64) ARCH=arm64 ;; x86_64) ARCH=amd64 ;; *) echo "unsupported arch $ARCH" >&2; exit 2 ;; esac
JQ_VERSION=1.7.1

PASSED=0
FAILED=0
FAILED_NAMES=()
CONTAINERS=()

log() { printf '\n== %s\n' "$*" >&2; }
pass() { PASSED=$((PASSED + 1)); printf 'PASS  %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); FAILED_NAMES+=("$1"); printf 'FAIL  %s%s\n' "$1" "${2:+ -- $2}"; }

cleanup() {
  local rc=$?
  if [[ $KEEP -eq 1 ]]; then
    echo "--keep: leaving network $CARRY_NETWORK, containers ${CONTAINERS[*]:-none}, work dir $WORK" >&2
    return $rc
  fi
  for c in "${CONTAINERS[@]:-}"; do [[ -n "$c" ]] && docker rm -f "$c" >/dev/null 2>&1 || true; done
  "${COMPOSE[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
  docker volume rm -f "carry-${RUN_ID}-web" >/dev/null 2>&1 || true
  rm -rf "$WORK"
  return $rc
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# Common carry settings, passed by name so values never hit the command line.
export CARRY_REPO="$REPO" CARRY_S3_ENDPOINT="http://minio:9000" CARRY_S3_REGION="us-east-1"
export AWS_ACCESS_KEY_ID="$MINIO_ROOT_USER" AWS_SECRET_ACCESS_KEY="$MINIO_ROOT_PASSWORD" CARRY_KEY
CARRY_ENV=(-e CARRY_REPO -e CARRY_S3_ENDPOINT -e CARRY_S3_REGION -e AWS_ACCESS_KEY_ID -e AWS_SECRET_ACCESS_KEY -e CARRY_KEY)

# Run the carry image as the laptop CLI. /work is the host work dir; HOME=/work so
# profile config and keygen output land there.
carry_cli() {
  docker run --rm -i --network "$CARRY_NETWORK" "${CARRY_ENV[@]}" \
    -e HOME=/work -e CARRY_TOOL_SIGNING_KEY_FILE="${SIGNING_KEY_FILE:-}" \
    -v "$WORK:/work" -w /work "$CARRY_IMAGE" "$@"
}
# carry_json <args...>: run with --json, require ok:true, print .result
carry_json() {
  local out
  out="$(carry_cli --json "$@")" || { echo "carry $* failed: $out" >&2; return 1; }
  [[ "$(jq -r '.ok' <<<"$out")" == "true" ]] || { echo "carry $* not ok: $out" >&2; return 1; }
  jq -c '.result' <<<"$out"
}

# start_app <name> <image> <host-port> <container-port> [docker run args...]
start_app() {
  local name="$1" img="$2" hport="$3" cport="$4"; shift 4
  docker rm -f "$name" >/dev/null 2>&1 || true
  CONTAINERS+=("$name")
  docker run -d --name "$name" --network "$CARRY_NETWORK" -p "127.0.0.1:${hport}:${cport}" \
    -e CARRY_POLL_SECS="$POLL" -e CARRY_RESTORE_BUDGET_MS="$BUDGET_MS" "$@" "$img" >/dev/null
}

# wait_http <url> <seconds> : succeed once the URL answers 2xx
wait_http() {
  local url="$1" deadline=$(( $(date +%s) + $2 ))
  while (( $(date +%s) < deadline )); do
    curl -fsS --max-time 1 "$url" >/dev/null 2>&1 && return 0
    sleep 0.2
  done
  return 1
}

# wait_body <url> <needle> <seconds>
wait_body() {
  local url="$1" needle="$2" deadline=$(( $(date +%s) + $3 ))
  while (( $(date +%s) < deadline )); do
    curl -fsS --max-time 1 "$url" 2>/dev/null | grep -qF -- "$needle" && return 0
    sleep 0.3
  done
  return 1
}

now_ms() { python3 -c 'import time;print(int(time.time()*1000))' 2>/dev/null || echo $(( $(date +%s) * 1000 )); }

# ---------------------------------------------------------------- setup
log "build images"
if [[ $BUILD -eq 1 ]]; then
  docker build -q -t "$CARRY_IMAGE" "$ROOT" >/dev/null
  docker build -q --build-arg CARRY_IMAGE="$CARRY_IMAGE" -t "$NGINX_IMG" "$HERE/nginx-config" >/dev/null
  docker build -q --build-arg CARRY_IMAGE="$CARRY_IMAGE" -t "$NODE_IMG" "$HERE/node-tools" >/dev/null
fi

log "start MinIO on network $CARRY_NETWORK"
"${COMPOSE[@]}" up -d minio >/dev/null
"${COMPOSE[@]}" run --rm minio-init >&2

mkdir -p "$WORK/seed"
cp "$HERE/nginx-config/seed/"* "$WORK/seed/"

# ---------------------------------------------------------------- cases
case_name="init + doctor"
if carry_json init >/dev/null && r="$(carry_json doctor)" \
   && [[ "$(jq -r '.reachable and .conditional_writes' <<<"$r")" == "true" ]]; then
  pass "$case_name"
else fail "$case_name"; fi

case_name="run without CARRY_REPO execs the app unchanged"
start_app e2e-norepo "$NGINX_IMG" "$WEB_PORT" 80
if wait_http "http://127.0.0.1:${WEB_PORT}/" 10 \
   && curl -fsS "http://127.0.0.1:${WEB_PORT}/" | grep -qi "welcome to nginx"; then
  pass "$case_name"
else fail "$case_name"; fi
docker rm -f e2e-norepo >/dev/null 2>&1 || true

case_name="put config from host CLI"
if carry_json put web-1:/etc/nginx/conf.d/default.conf /work/seed/default.conf >/dev/null \
   && carry_json put web-1:/usr/share/nginx/html/index.html /work/seed/index.html >/dev/null \
   && r="$(carry_json ls web-1)" \
   && [[ "$(jq '[.entries[].path] | index("/etc/nginx/conf.d/default.conf") != null' <<<"$r")" == "true" ]]; then
  pass "$case_name"
else fail "$case_name"; fi

WEB_ENV=("${CARRY_ENV[@]}" -e CARRY_STATE=web-1 -e CARRY_RELOAD_SIGNAL=HUP)

case_name="app starts with restored config"
docker volume create "carry-${RUN_ID}-web" >/dev/null
start_app e2e-web "$NGINX_IMG" "$WEB_PORT" 80 "${WEB_ENV[@]}" -v "carry-${RUN_ID}-web:/carry"
if wait_body "http://127.0.0.1:${WEB_PORT}/from-carry" "conf-from-carry" 15 \
   && wait_body "http://127.0.0.1:${WEB_PORT}/" "hello from carry v1" 5; then
  pass "$case_name"
else fail "$case_name" "$(docker logs --tail 20 e2e-web 2>&1 | tr '\n' ' ')"; fi

case_name="replace container (fresh /carry) keeps config"
start_app e2e-web2 "$NGINX_IMG" "$((WEB_PORT + 1))" 80 "${WEB_ENV[@]}"
if wait_body "http://127.0.0.1:$((WEB_PORT + 1))/from-carry" "conf-from-carry" 15; then
  pass "$case_name"
else fail "$case_name"; fi
docker rm -f e2e-web2 >/dev/null 2>&1 || true

case_name="status --json reports ok while store is up"
if out="$(docker exec e2e-web carry status --json 2>/dev/null)" \
   && [[ "$(jq -r '.result.mode' <<<"$out")" == "ok" ]] \
   && [[ "$(jq -r '.result.state' <<<"$out")" == "web-1" ]]; then
  pass "$case_name"
else fail "$case_name" "${out:-no output}"; fi

case_name="laptop update applied within poll interval"
printf '<!doctype html><title>carry</title><p>hello from carry v2</p>\n' >"$WORK/seed/index.html"
carry_json put web-1:/usr/share/nginx/html/index.html /work/seed/index.html >/dev/null || true
# one poll interval + restore budget + slack
if wait_body "http://127.0.0.1:${WEB_PORT}/" "hello from carry v2" $(( POLL + BUDGET_MS / 1000 + 3 )); then
  pass "$case_name"
else fail "$case_name"; fi

case_name="log shows the history"
if r="$(carry_json log web-1)" && (( $(jq '.versions | length' <<<"$r") >= 3 )); then
  pass "$case_name"
else fail "$case_name"; fi

# --- env defaults vs container override (node app, state tools-1)
NODE_ENV=("${CARRY_ENV[@]}" -e CARRY_STATE=tools-1)
carry_json env set tools-1 GREETING=hello-from-state LOG_LEVEL=info >/dev/null || true

case_name="env default from state"
start_app e2e-node "$NODE_IMG" "$NODE_PORT" 3000 "${NODE_ENV[@]}"
if wait_http "http://127.0.0.1:${NODE_PORT}/healthz" 15 \
   && r="$(curl -fsS "http://127.0.0.1:${NODE_PORT}/env")" \
   && [[ "$(jq -r '.GREETING + "," + .LOG_LEVEL' <<<"$r")" == "hello-from-state,info" ]]; then
  pass "$case_name"
else fail "$case_name" "${r:-}"; fi

case_name="container env overrides stored default"
start_app e2e-node "$NODE_IMG" "$NODE_PORT" 3000 "${NODE_ENV[@]}" -e LOG_LEVEL=debug
if wait_http "http://127.0.0.1:${NODE_PORT}/healthz" 15 \
   && r="$(curl -fsS "http://127.0.0.1:${NODE_PORT}/env")" \
   && [[ "$(jq -r '.GREETING + "," + .LOG_LEVEL' <<<"$r")" == "hello-from-state,debug" ]]; then
  pass "$case_name"
else fail "$case_name" "${r:-}"; fi

# --- tool store
log "fetch jq ${JQ_VERSION} for linux/${ARCH}"
curl -fsSL -o "$WORK/jq" "https://github.com/jqlang/jq/releases/download/jq-${JQ_VERSION}/jq-linux-${ARCH}"
chmod +x "$WORK/jq"

case_name="tool keygen writes a key file and prints only the public key"
PUBKEY=""
if r="$(carry_json tool keygen)" \
   && PUBKEY="$(jq -r '.public_key' <<<"$r")" && [[ -n "$PUBKEY" && "$PUBKEY" != null ]] \
   && kf="$(jq -r '.private_key_file' <<<"$r")" \
   && [[ "$kf" == /work/* && -s "$WORK/${kf#/work/}" ]] \
   && [[ "$(jq 'has("private_key")' <<<"$r")" == "false" ]]; then
  SIGNING_KEY_FILE="$kf"
  pass "$case_name"
else fail "$case_name" "${r:-}"; fi

case_name="tool publish + ls"
if carry_json tool publish "jq@${JQ_VERSION}" --file /work/jq --platform "linux/${ARCH}" >/dev/null \
   && r="$(carry_json tool ls jq)" \
   && [[ "$(jq --arg v "$JQ_VERSION" '[.tools[] | select(.name=="jq") | .versions[]] | index($v) != null' <<<"$r")" == "true" ]]; then
  pass "$case_name"
else fail "$case_name" "${r:-}"; fi

case_name="tool add + first use of jq inside node app"
carry_json tool add tools-1 "jq@${JQ_VERSION}" >/dev/null || true
start_app e2e-node "$NODE_IMG" "$NODE_PORT" 3000 "${NODE_ENV[@]}" -e CARRY_TOOL_PUBKEY="$PUBKEY"
if wait_http "http://127.0.0.1:${NODE_PORT}/healthz" 15 \
   && r="$(curl -fsS --max-time 30 "http://127.0.0.1:${NODE_PORT}/jq")" && [[ "$r" == "jq-42" ]] \
   && st="$(docker exec e2e-node carry status --json)" \
   && [[ "$(jq -r '.result.tools[] | select(.name=="jq") | .status' <<<"$st")" == "ready" ]]; then
  pass "$case_name"
else fail "$case_name" "${r:-} ${st:-}"; fi

case_name="tool signed by another key is refused"
other_pub="$(carry_json tool keygen --out /work/other-signing.key | jq -r '.public_key')" || other_pub=""
start_app e2e-node-bad "$NODE_IMG" "$((NODE_PORT + 1))" 3000 "${NODE_ENV[@]}" -e CARRY_TOOL_PUBKEY="$other_pub"
if [[ -n "$other_pub" ]] && wait_http "http://127.0.0.1:$((NODE_PORT + 1))/healthz" 15 \
   && code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 "http://127.0.0.1:$((NODE_PORT + 1))/jq")" \
   && [[ "$code" == "500" ]]; then
  pass "$case_name"
else fail "$case_name" "http ${code:-?}"; fi
docker rm -f e2e-node-bad >/dev/null 2>&1 || true

case_name="exit code of the child is propagated"
set +e
docker run --rm --network "$CARRY_NETWORK" "${NODE_ENV[@]}" --entrypoint carry "$NODE_IMG" run -- sh -c 'exit 7' >/dev/null 2>&1
rc=$?
set -e
if [[ $rc -eq 7 ]]; then pass "$case_name"; else fail "$case_name" "got $rc"; fi

case_name="SIGTERM is forwarded (graceful stop)"
docker stop -t 8 e2e-node >/dev/null
rc="$(docker inspect -f '{{.State.ExitCode}}' e2e-node)"
if [[ "$rc" == "0" ]]; then pass "$case_name"; else fail "$case_name" "exit $rc (137 = not forwarded)"; fi

# --- fail-open
log "stop MinIO"
"${COMPOSE[@]}" stop minio >/dev/null

case_name="store down: replaced container reuses last good local generation"
docker rm -f e2e-web >/dev/null
start_app e2e-web "$NGINX_IMG" "$WEB_PORT" 80 "${WEB_ENV[@]}" -v "carry-${RUN_ID}-web:/carry"
if wait_body "http://127.0.0.1:${WEB_PORT}/" "hello from carry v2" $(( BUDGET_MS / 1000 + 6 )); then
  pass "$case_name"
else fail "$case_name"; fi

case_name="store down: fresh container starts within budget + 1s"
t0="$(now_ms)"
start_app e2e-web3 "$NGINX_IMG" "$((WEB_PORT + 2))" 80 "${WEB_ENV[@]}"
if wait_http "http://127.0.0.1:$((WEB_PORT + 2))/" 15; then
  # measure from carry's own report; docker start overhead is not carry's budget
  st="$(docker exec e2e-web3 carry status --json 2>/dev/null || true)"
  ms="$(jq -r '.result.last_restore_ms // empty' <<<"$st" 2>/dev/null || true)"
  elapsed=$(( $(now_ms) - t0 ))
  if [[ -n "$ms" ]] && (( ms <= BUDGET_MS + 1000 )); then
    pass "$case_name (restore ${ms}ms, wall ${elapsed}ms)"
  else fail "$case_name" "last_restore_ms=${ms:-?} wall=${elapsed}ms"; fi
else fail "$case_name" "app never answered"; fi

case_name="store down: status --json says degraded"
if st="$(docker exec e2e-web3 carry status --json 2>/dev/null)" \
   && [[ "$(jq -r '.result.mode' <<<"$st")" == "degraded" ]]; then
  pass "$case_name"
else fail "$case_name" "${st:-no output}"; fi

case_name="store down: CLI exits 3 (unreachable)"
set +e
out="$(carry_cli --json --timeout 3 ls web-1 2>/dev/null)"; rc=$?
set -e
if [[ $rc -eq 3 && "$(jq -r '.error.code' <<<"$out" 2>/dev/null)" == "unreachable" ]]; then
  pass "$case_name"
else fail "$case_name" "rc=$rc out=$out"; fi

# ---------------------------------------------------------------- summary
echo
echo "passed: $PASSED  failed: $FAILED"
if (( FAILED > 0 )); then
  printf '  - %s\n' "${FAILED_NAMES[@]}"
  exit 1
fi
