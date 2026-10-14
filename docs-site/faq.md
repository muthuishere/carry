# FAQ

**Which S3 providers work?**
Anything S3-compatible that honours conditional writes (`If-None-Match`, `If-Match`). AWS S3 does. MinIO is used in the repo's tests. For R2, Hetzner, Scaleway and others, run `carry doctor` and check `conditional_writes`.

**Does my app need to know about carry?**
No. It sees ordinary files (symlinks into `/carry/current/files`), env vars and tools on `PATH`.

**Why not mount S3 with FUSE?**
FUSE needs privileges, and S3 mounts lack atomic rename and symlinks that real programs need. carry restores to local disk instead ([ADR 0001](https://github.com/muthuishere/carry/blob/main/docs/adr/0001-snapshot-sync-not-filesystem-mount.md)).

**If I edit a file inside the container, is it saved?**
No. The CLI is the only write path. Local edits are not uploaded, and are lost on the next version flip or pod replacement. A shell `rm` never deletes anything in the bucket ([ADR 0002](https://github.com/muthuishere/carry/blob/main/docs/adr/0002-cli-is-the-only-write-path.md)).

**How fast do changes reach containers?**
Within `CARRY_POLL_SECS` (default 30 s). Files and tools update live; env changes need an app restart.

**What happens if S3 is down when my pod starts?**
The app starts after at most `CARRY_RESTORE_BUDGET_MS` (2 s) with the last good local generation, or with an empty tree on a brand-new pod. `carry status` reports `degraded`.

**Can two people commit at the same time?**
Yes. Commits are compare-and-swap on HEAD; the loser retries automatically (up to 40 attempts / 15 s) and only then exits 5 with `conflict`.

**Can I use it without encryption?**
Only for tests (`CARRY_NO_ENCRYPTION=1`). An encrypted repo refuses unencrypted writes.

**How do I rotate the master key?**
v0.1 has no rotation command. See [Security model → Key management](security.md#key-management-limits-in-v01).

**Can I share one bucket across many apps?**
Yes: one repository holds many states, and you can use separate prefixes (`s3://bucket/team-a`, `s3://bucket/team-b`) with separate keys for separate trust boundaries.

**Does it work on Mac/Windows?**
The runtime targets Linux containers (`linux/amd64`, `linux/arm64`). For the CLI on a laptop, run it from the image (see [Quickstart](quickstart.md#2-make-a-key-and-a-cli-alias)) or build from source with `cargo build --release` on a Unix system.

**Can it install apt/apk packages?**
No. Tools are single static binaries per platform. Bake packages into the image.

**Is there a size limit?**
No hard limit, but carry is built for config-sized trees. Large, frequently changing files grow the repo forever in v0.1 (no gc). See [Operations](operations.md#repository-growth).

**Can I delete a state?**
Not from the CLI in v0.1. `carry rm` removes files from the current version; history remains.

**How do I use it from CI?**
Install the static binary, export `CARRY_REPO`, credentials and `CARRY_KEY` from CI secrets, and run CLI commands with `--json`. Parse the envelope and act on exit codes.
