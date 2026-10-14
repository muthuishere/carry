# Kubernetes guide

v0.1 runs as **one container, no init container, no sidecar**: the app image contains carry and uses `carry run --` as its entrypoint. Manifests live in [`deploy/k8s/`](https://github.com/muthuishere/carry/tree/main/deploy/k8s).

## 1. Build the image

```dockerfile
FROM nginx:1.27-alpine
COPY --from=ghcr.io/muthuishere/carry:0.1.1 /carry /usr/local/bin/carry
ENTRYPOINT ["carry", "run", "--"]
CMD ["nginx", "-g", "daemon off;"]
```

## 2. Create the Secret

Generate the master key once, keep a copy offline (losing it loses the data), and put it in a Secret together with the S3 credentials:

```sh
openssl rand -base64 32 > carry.key      # back this up somewhere safe

kubectl create secret generic carry-web \
  --from-literal=CARRY_REPO=s3://YOUR_BUCKET/YOUR_PREFIX \
  --from-literal=CARRY_S3_REGION=us-east-1 \
  --from-literal=AWS_ACCESS_KEY_ID="$AWS_ACCESS_KEY_ID" \
  --from-literal=AWS_SECRET_ACCESS_KEY="$AWS_SECRET_ACCESS_KEY" \
  --from-literal=CARRY_KEY="$(cat carry.key)" \
  --from-literal=CARRY_TOOL_PUBKEY="$(cat tool-signing.pub)"   # if you use tools
```

Or edit [`deploy/k8s/secret.yaml`](https://github.com/muthuishere/carry/blob/main/deploy/k8s/secret.yaml), replacing every `YOUR_KEY_HERE`. Add `CARRY_S3_ENDPOINT` for non-AWS stores.

## 3. Deployment

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
spec:
  replicas: 2
  selector: {matchLabels: {app: web}}
  template:
    metadata: {labels: {app: web}}
    spec:
      terminationGracePeriodSeconds: 20   # carry forwards SIGTERM to the app
      containers:
        - name: web
          image: YOUR_REGISTRY/carry-example-nginx:0.1.0
          args: ["nginx", "-g", "daemon off;"]
          envFrom:
            - secretRef: {name: carry-web}
          env:
            - {name: CARRY_STATE, value: web-1}
            - {name: CARRY_ROOT, value: /carry}
            - {name: CARRY_POLL_SECS, value: "30"}
            - {name: CARRY_RESTORE_BUDGET_MS, value: "2000"}
            - {name: CARRY_RELOAD_SIGNAL, value: HUP}
          ports:
            - {containerPort: 80, name: http}
          readinessProbe:            # probe the APP, never carry
            httpGet: {path: /healthz, port: http}
            periodSeconds: 5
          resources:
            requests: {cpu: 50m, memory: 64Mi}
            limits: {memory: 128Mi}  # carry's 64 MiB cap + your app
          volumeMounts:
            - {name: carry, mountPath: /carry}
      volumes:
        - name: carry
          emptyDir: {sizeLimit: 512Mi}   # generations + tool cache
```

```sh
kubectl apply -f deploy/k8s/secret.yaml
kubectl apply -f deploy/k8s/deployment.yaml
```

!!! warning "Always set `CARRY_STATE`"
    Without it carry uses `$HOSTNAME`, which on Kubernetes is the pod name, so every pod would look for its own (empty) state.

## Why an `emptyDir`

`/carry` holds generations (`gens/<seq>`, `current`), tool links (`bin/`) and the tool cache. An `emptyDir` lives as long as the pod: a container restart (OOM, crash) reuses the last good generation even if S3 is down. A new pod starts empty and restores from S3. A PVC is not needed and would tie pods to a zone.

## Multi-replica

All replicas with the same `CARRY_STATE` restore the same tree and converge on new versions within one poll interval (`CARRY_POLL_SECS`, default 30 s). Replicas only *read*; writes come from your CLI or CI, and are safe even when several run at once because every commit is a compare-and-swap on HEAD (see [How it works](../how-it-works.md#compare-and-swap-head)).

Want per-replica state (e.g. a StatefulSet where each pod has its own config)? Set `CARRY_STATE` from the pod name with the downward API:

```yaml
env:
  - name: CARRY_STATE
    valueFrom: {fieldRef: {fieldPath: metadata.name}}
```

## Rolling updates

- **Config change** (`carry put`, `env set`, `tool add`): no rollout needed for files and tools; pods pick it up on the next poll and get `CARRY_RELOAD_SIGNAL`. Env changes only reach the app on restart: `kubectl rollout restart deploy/web`.
- **Image change**: a normal rolling update. Each new pod restores at start within the 2 s budget, then starts the app; readiness still comes only from the app.
- **Staged rollout of config**: use a second state (`web-canary`) for some pods, then copy the change to `web-1` once happy. There is no built-in canary in v0.1.
- **Bad config**: `carry rollback web-1 <n>`; every pod reverts on its next poll.

## Probes and fail-open

Probe only the app. If S3 is down, carry starts the app with the last good (or an empty) generation and reports `"mode": "degraded"`:

```sh
kubectl exec deploy/web -- carry status --json
```

Never add a readiness probe that checks carry: an S3 outage would then mark every replica unready and take the service down, which is exactly what fail-open exists to prevent. (When the agent later moves into a native sidecar, the same rule applies: own memory limit, no probes.)

## Signals and shutdown

carry forwards `SIGTERM` (and `SIGINT`, `SIGHUP`, `SIGQUIT`) to the app and exits with its exit code, so `terminationGracePeriodSeconds` behaves exactly as without carry.

## Resources

carry targets under 20 MiB idle and caps itself at 64 MiB. A `64Mi` request covers carry; add your app's needs to the limit. Size the `emptyDir` for two generations plus the tools you use.
