# 0001. Restore-and-commit to local disk, not a live S3 mount

- Status: Accepted
- Date: 2026-10-13

## Context
Containers lose everything added after the image was built: installed tools, config files, env defaults. Existing fixes are tied to one cluster (PersistentVolumes), need privileges (FUSE mounts such as JuiceFS, Mountpoint-S3, s3fs), or are batch backups (restic, kopia, Velero).
Mountpoint-S3 documents no support for directory rename or symlinks; s3fs renames by copy+delete. `apt install` needs atomic rename, chmod and symlinks.

## Decision
carry restores state from S3 onto ordinary local disk before the app starts and commits changes back as new immutable versions. It never mounts S3.

## Consequences
- Full POSIX and local-disk speed for the app; no privileges; works in plain Docker.
- State is only as fresh as the last commit (acceptable: changes go through the CLI, which commits synchronously, see 0002).
- Very large trees pay a restore cost; mitigated by on-demand tools (0006) and a future lazy mode.
