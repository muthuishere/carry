# 0005. The agent may fail; the app must not

- Status: Accepted
- Date: 2026-10-13

## Decision
- Restore has a time budget (default 2 s for config/env). On any error or timeout the app starts from the last good local copy, or empty, and status reports `degraded`. Restore exit code is always 0 under `--fail-open` (the default for `carry run`).
- Restore writes into a new generation directory, verifies every blob, then flips a symlink atomically. A half restore is never visible.
- `carry run -- <app>` supervises the app as a child, forwards signals, and exits with the app's exit code. Agent work runs on its own tasks; a panic there is caught and logged, never propagated.
- Memory: streaming with bounded buffers; concurrency sized from cgroup `memory.max`. Target idle < 20 MiB, limit 64 MiB.
- On Kubernetes the background agent runs as a native sidecar with its own limit and no readiness probe.
