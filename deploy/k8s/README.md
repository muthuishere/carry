# carry on Kubernetes

```
kubectl apply -f secret.yaml       # after replacing every YOUR_KEY_HERE (or use kubectl create secret)
kubectl apply -f deployment.yaml
```

Prefer the Helm chart in [`../helm/carry`](../helm/carry): it can inject carry into an unmodified app image with an init container. These plain manifests are the same shape for an image that already contains carry.

## Shape (v0.1)

- **One container, no init container.** The CLI contract has no `restore` command; `carry run -- <app>` restores and then starts the app. Put it in the image (`COPY --from=$EMAIL_2/carry:<ver> /carry /usr/local/bin/carry`, `ENTRYPOINT ["carry","run","--"]`) and pass the app command as `args`.
- **`/carry` is an `emptyDir`.** It holds generations (`gens/<seq>`, `current`), tool links (`bin/`) and the tool cache. It lasts for the pod's life, so a container restart in the same pod reuses the last good generation if S3 is down. A new pod starts with an empty dir and restores from S3.
- **Key as a file, never an env var.** The master key Secret (`carry-web-key`) is mounted read-only and found through `CARRY_KEY_FILE`; S3 credentials come from `carry-web-s3` via `secretKeyRef`. `carry run` strips the credentials and `CARRY_KEY_FILE` from the app's environment. `CARRY_REPO`, `CARRY_TOOL_PUBKEY` (public) and `CARRY_STATE` are plain env.
- **Fail-open.** Restore has a 2 s budget (`CARRY_RESTORE_BUDGET_MS`). If S3 is down the app starts anyway with the last good or an empty generation, and `kubectl exec <pod> -- carry status --json` reports `"mode": "degraded"`.
- **Memory.** carry aims for under 20 MiB idle and caps itself at 64 MiB. The 64Mi request covers carry; add your app's needs to the limit.
- **Signals.** carry forwards SIGTERM to the app and exits with its code, so `terminationGracePeriodSeconds` behaves as it would without carry.

## Rule for later versions: no readiness probe on the sidecar

ADR 0005 plans to move the background agent into a native sidecar (`initContainers` with `restartPolicy: Always`). When that happens, the sidecar gets **its own memory limit and no readiness or liveness probe**. A sidecar's readiness counts toward the pod's, so an S3 outage would mark every replica unready and take the service down. That is what fail-open exists to prevent. Probe only the app. Report carry's health through `carry status` and metrics.
