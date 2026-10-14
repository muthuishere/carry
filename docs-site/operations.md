# Operations

## Repository growth

carry never deletes from the bucket in v0.1. Every commit adds:

- one **manifest** (the whole tree's entries; files under 4 KiB inline),
- one **blob per new or changed file** of 4 KiB or more (unchanged content dedupes by id),
- an update to `refs/<state>/HEAD`.

Rough sizing: a state with 50 small config files and 100 changes a day adds about 100 manifests a day, each a few KiB. Large files changed often are what grows a repo; put those in tools (published once per version) or keep them out of carry.

## Garbage collection

**There is no `gc` / `prune` command in v0.1.** Removed files and old versions stay in the bucket forever, which is what makes any version restorable. Do not use S3 lifecycle rules to expire `blobs/` or `manifests/`: they cannot tell which objects are still referenced, and deleting a live blob breaks restores (containers will then degrade).

Locally, containers keep only the newest **2 generations** in `/carry/gens` and clean stale `*.tmp` dirs automatically.

## Rollback

```sh
carry log web-1 --limit 20
carry diff web-1 14 17          # what changed
carry rollback web-1 14         # new version 18 = tree of version 14
```

Rollback restores files, env and the tool list of that version, as a new version, so you can roll *forward* again the same way. Running containers apply it on their next poll. Env changes need an app restart to take effect.

To undo a single change, prefer the opposite command (`put` the old file, `env set` the old value) over a whole-tree rollback.

## Monitoring

Inside a container:

```sh
carry status --json
```

| Field | Watch for |
|---|---|
| `mode` | `degraded` = last restore or poll failed; app runs on last good or empty tree. `disabled` = no `CARRY_REPO` |
| `version` | Should match `carry states` for that state within one poll interval |
| `last_restore_ms` | Creeping towards `CARRY_RESTORE_BUDGET_MS` means the store is slow |
| `error` | Last error text |
| `tools[].status` | `failed` with a `reason` (signature, yank, download) |

From outside: `carry states --json` lists every state's current version. A simple drift check is to compare that with `kubectl exec ... carry status --json` across pods.

There is no Prometheus endpoint in v0.1. carry logs to stderr with a `carry:` prefix (`carry: poll: ...`, `carry: warning: ...`), so your normal container log pipeline picks them up.

## Store checks

Run `carry doctor` when you change provider, endpoint or credentials. `conditional_writes: false` means concurrent commits are not safe on that store.

## Backups

- The bucket *is* the history; enable bucket versioning / replication for disaster recovery.
- Back up the **master key** separately. Without it a bucket backup is useless.
- Back up the **tool signing key** if you want to keep publishing under the same trust root.

## Upgrades

Readers stay compatible with older objects (unmarked legacy blobs and manifests remain readable). Upgrade containers before CLIs when a format change is announced. Images are pinned by tag (`0.1.1`); change the tag in your `COPY --from` line and rebuild.
