#!/usr/bin/env bash
# carry stress test against MinIO, using only the published image. Cleans up on exit.
#   WRITERS concurrent writers x WRITES each   -> no lost commits, versions contiguous
#   REPLICAS polling replicas                  -> all converge to the final HEAD
#   a 256 MiB file under a 64 MiB memory limit -> restore succeeds, RSS bounded
#   S3 killed mid-write-storm                  -> writers fail cleanly, apps keep running
set -uo pipefail
IMG=${IMG:-ghcr.io/muthuishere/carry:0.1.1}
WRITERS=${WRITERS:-20}; WRITES=${WRITES:-10}; REPLICAS=${REPLICAS:-25}
N="carrystress$$"; NET="$N"; W="$(mktemp -d)"; U=stress
P="$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"
KEY="$(head -c 32 /dev/urandom | base64)"
PASS=0; FAIL=0
pass(){ echo "PASS  $*"; PASS=$((PASS+1)); }
fail(){ echo "FAIL  $*"; FAIL=$((FAIL+1)); }
check(){ local n="$1"; shift; if "$@"; then pass "$n"; else fail "$n"; fi; }
log(){ echo; echo "== $*"; }
now(){ date +%s.%N; }
cleanup(){
  docker rm -f $(docker ps -aq --filter "label=$N") >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
  docker rmi -f "$N-app" >/dev/null 2>&1
  rm -rf "$W"
}
trap cleanup EXIT

ENVS=(-e CARRY_REPO=s3://carry/stress -e "CARRY_S3_ENDPOINT=http://$N-minio:9000" -e CARRY_S3_REGION=us-east-1
      -e "AWS_ACCESS_KEY_ID=$U" -e "AWS_SECRET_ACCESS_KEY=$P" -e "CARRY_KEY=$KEY")
cli(){ docker run --rm -i --label "$N" --network "$NET" "${ENVS[@]}" -v "$W:/work" "$IMG" "$@"; }

log "MinIO"
docker network create "$NET" >/dev/null
docker run -d --label "$N" --name "$N-minio" --network "$NET" -u 0 \
  -e "MINIO_ROOT_USER=$U" -e "MINIO_ROOT_PASSWORD=$P" chainguard/minio:latest server /data >/dev/null
for i in $(seq 1 60); do
  code=$(docker run --rm --network "$NET" curlimages/curl:8.11.1 -s -o /dev/null -w '%{http_code}' -X PUT \
    --aws-sigv4 "aws:amz:us-east-1:s3" --user "$U:$P" "http://$N-minio:9000/carry" 2>/dev/null)
  [[ "$code" == 200 || "$code" == 409 ]] && break; sleep 1
done
printf 'seed\n' > "$W/seed"; cli put s:/etc/s/seed /work/seed --json >/dev/null

mkdir -p "$W/app"
cat > "$W/app/Dockerfile" <<EOF
FROM alpine:3.20
COPY --from=$IMG /carry /usr/local/bin/carry
ENTRYPOINT ["carry","run","--"]
CMD ["sleep","infinity"]
EOF
docker build -q -t "$N-app" "$W/app" >/dev/null

log "$REPLICAS polling replicas"
t0=$(now)
for i in $(seq 1 "$REPLICAS"); do
  docker run -d --label "$N" --name "$N-r$i" --network "$NET" "${ENVS[@]}" -e CARRY_STATE=s -e CARRY_POLL_SECS=2 "$N-app" >/dev/null &
done; wait
echo "started in $(echo "$(now) - $t0" | bc)s"
sleep 3
ok=0; for i in $(seq 1 "$REPLICAS"); do docker exec "$N-r$i" cat /etc/s/seed 2>/dev/null | grep -q seed && ok=$((ok+1)); done
check "all $REPLICAS replicas restored seed ($ok)" test "$ok" -eq "$REPLICAS"

log "$WRITERS writers x $WRITES writes, all at once (one-shot containers)"
t0=$(now)
for w in $(seq 1 "$WRITERS"); do
  ( for k in $(seq 1 "$WRITES"); do
      echo "w$w-$k" | cli put "s:/etc/s/w$w/f$k" - --json 2>/dev/null | jq -c '{ok, v: .result.version, e: .error.code}'
    done > "$W/writer$w.log" ) &
done; wait
dt=$(echo "$(now) - $t0" | bc)
total=$((WRITERS*WRITES)); okc=$(cat "$W"/writer*.log | jq -s 'map(select(.ok)) | length')
echo "commits ok: $okc / $total in ${dt}s; errors: $(cat "$W"/writer*.log | jq -r 'select(.ok|not) | .e' | sort | uniq -c | tr '\n' ' ')"
check "every write committed" test "$okc" -eq "$total"
uniq=$(cat "$W"/writer*.log | jq -r 'select(.ok) | .v' | sort -n | uniq | wc -l)
check "no two writes got the same version" test "$uniq" -eq "$okc"
head=$(cli log s --json | jq '.result.versions[0].version')
check "HEAD = 1 + $total (got $head)" test "$head" -eq $((1+total))
files=$(cli ls s --json | jq '.result.entries | length')
check "repo lists all $((total+1)) files (got $files)" test "$files" -eq $((total+1))

log "replicas converge to HEAD"
sleep 8
conv=0; for i in $(seq 1 "$REPLICAS"); do
  v=$(docker exec "$N-r$i" carry status --json 2>/dev/null | jq '.result.version'); [ "$v" = "$head" ] && conv=$((conv+1)); done
check "all $REPLICAS replicas at v$head ($conv)" test "$conv" -eq "$REPLICAS"
r=$(( (RANDOM % REPLICAS) + 1 ))
check "replica r$r has w$WRITERS/f$WRITES content" bash -c "docker exec $N-r$r cat /etc/s/w$WRITERS/f$WRITES | grep -qx w$WRITERS-$WRITES"
echo "replica RSS (KiB): $(for i in 1 2 3; do docker exec "$N-r$i" sh -c 'grep VmRSS /proc/1/status' | awk '{print $2}'; done | tr '\n' ' ')"

log "256 MiB file, restored by a replica capped at 64 MiB"
head -c $((256*1024*1024)) /dev/urandom > "$W/big"; sum=$(sha256sum "$W/big" | cut -c1-64)
t0=$(now); cli put big:/data/big /work/big --json | jq -c .result; echo "upload $(echo "$(now) - $t0" | bc)s"
docker run -d --label "$N" --name "$N-big" --network "$NET" --memory 64m --memory-swap 64m "${ENVS[@]}" \
  -e CARRY_STATE=big -e CARRY_RESTORE_BUDGET_MS=60000 "$N-app" >/dev/null
for i in $(seq 1 90); do docker exec "$N-big" test -e /data/big 2>/dev/null && break; sleep 1; done
check "big replica not OOM-killed" bash -c "[ \"\$(docker inspect -f '{{.State.OOMKilled}} {{.State.Running}}' $N-big)\" = 'false true' ]"
check "256 MiB file restored byte-exact" bash -c "[ \"\$(docker exec $N-big sha256sum /data/big | cut -c1-64)\" = $sum ]"

log "S3 killed in the middle of a write storm"
for w in 1 2 3 4 5; do
  ( for k in $(seq 1 20); do echo x | cli put "s:/etc/s/storm/$w-$k" - --json 2>/dev/null | jq -c '{ok, e: .error.code}'; done > "$W/storm$w.log" ) &
done
sleep 2; docker kill "$N-minio" >/dev/null; wait
bad=$(cat "$W"/storm*.log | jq -r 'select(.ok|not) | .e' | sort | uniq -c | tr '\n' ' ')
echo "after kill: $(cat "$W"/storm*.log | jq -s 'map(select(.ok))|length') ok, failures: $bad"
check "failures are clean error codes, never a crash" bash -c "! cat $W/storm*.log | jq -e 'select(.ok|not) | select(.e==null)' >/dev/null"
alive=0; for i in $(seq 1 "$REPLICAS"); do [ "$(docker inspect -f '{{.State.Running}}' "$N-r$i")" = true ] && alive=$((alive+1)); done
check "all replicas still running with S3 dead ($alive)" test "$alive" -eq "$REPLICAS"
docker start "$N-minio" >/dev/null; sleep 5
check "repo consistent after S3 returns (log readable)" bash -c "$(declare -f cli); $(declare -p ENVS N NET W IMG); cli log s --json | jq -e .ok >/dev/null"

echo; echo "passed: $PASS  failed: $FAIL"
