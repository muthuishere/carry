# 0002. The CLI is the only write path

- Status: Accepted
- Date: 2026-10-13

## Context
Watching a directory with inotify misses events on overflow, sees half-written files, and turns accidental `rm` into data loss across every replica.

## Decision
Every change (put, rm, mv, env set, tool install, rollback) is an explicit `carry` command that commits a new version to S3 before it returns success. A shell `rm` inside the container has no effect on S3. Deletes and renames happen only when asked for.
Folder watching (`watch: true`) and `carry commit <dir>` are optional escape hatches for apps that write their own files; not in v0.1.

## Consequences
- No watcher, no reconcile loop, no lost-event class of bugs.
- Every version records the operation that produced it: free audit log and per-op rollback.
- Apps that rewrite their own config need the escape hatch (later).
