## Why
A container loses everything added after its image was built (tools, config files, env defaults) whenever it restarts, moves or scales. Existing fixes are tied to one cluster (PersistentVolumes), need privileges (S3 FUSE mounts), or are batch backups. carry makes that state follow the container from one S3 bucket, in Docker or Kubernetes, with no privileges.

## What Changes
- New Rust binary `carry` (CLI, container entrypoint wrapper, tool launcher in one).
- Encrypted, versioned, content-addressed repository format on any S3-compatible store.
- `carry run -- <app>`: restore config/env fast and fail-open, then run and supervise the app.
- Explicit write commands (put, rm, mv, env, rollback) that commit synchronously; no file watching.
- Shared, signed tool store with on-demand download via busybox-style symlinks.
- kubectl-style profiles for using the same CLI against a bucket from a laptop.
- Scratch image on Docker Hub to `COPY --from`, Docker Compose and Kubernetes examples.
- Wrappers for JS, Python, C#, Java, Go over the CLI's JSON contract.

## Capabilities

### New Capabilities
- `repository`: on-S3 format, encryption, versions, HEAD compare-and-swap, lease.
- `state-commands`: put/get/ls/rm/mv/log/diff/rollback/env, non-interactive JSON contract and exit codes.
- `container-runtime`: `carry run`, fail-open restore, atomic generation flip, supervision, HEAD polling.
- `tool-store`: publish/promote/yank/ls, signatures, on-demand launch via symlinks.
- `profiles`: local config with named profiles and precedence rules.
- `distribution`: static binaries, scratch multi-arch image, examples.
- `language-wrappers`: JS, Python, C#, Java, Go wrappers and examples.

### Modified Capabilities
- none

## Impact
New repository (github.com/muthuishere/carry), new Docker Hub image `muthuishere/carry`. Dependencies: tokio, object_store, clap, serde, blake3, zstd, chacha20poly1305, ed25519-dalek.
