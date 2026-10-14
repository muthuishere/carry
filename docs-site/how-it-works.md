# How it works

carry has two halves sharing one repository format:

- **The CLI** (`carry put`, `env set`, `rollback`, ...) is the *only* write path. Each command commits a new version to S3 before it returns success.
- **The runtime** (`carry run -- <app>`) restores the latest version onto local disk, starts your app as a supervised child, and polls for new versions.

```text
 laptop / CI                      S3 bucket (ciphertext only)             container
 ───────────                      ───────────────────────────             ─────────
 carry put web-1:/etc/a ./a  ──►  blobs/  manifests/  refs/web-1/HEAD ──► carry run -- app
 carry env set web-1 K=V          (conditional PUT on HEAD)                 restore → /carry/gens/<seq>
                                                                            flip /carry/current
                                                                            symlink /etc/a → current/files/etc/a
                                                                            exec app, poll, reload
```

## Repository format

A repository lives under a prefix, e.g. `s3://bucket/prefix/` ([ADR 0003](https://github.com/muthuishere/carry/blob/main/docs/adr/0003-repository-format.md)):

```text
config                   plaintext: format version, kdf/cipher ids, wrapped data keys
blobs/<ab>/<id>          encrypted, compressed file contents (one blob per file in v0.1)
manifests/<id>           encrypted: parent, op, created_at, state, seq, tree of entries
refs/<state>/HEAD        {"manifest": id, "seq": n}, written with If-Match / If-None-Match
refs/<state>/lease       {"holder", "expires"}, conditional PUT (rollback only)
tools/...                shared, signed tool store
```

- A **state** is a named tree inside the repo (`web-1`, `tools-1`, ...). One bucket holds many apps.
- A state's tree contains **files** (with mode bits), **symlinks**, an **env** map and an **apps.lock** list of tools.
- Files smaller than **4 KiB** are stored inline in the manifest; larger files become blobs.
- Unchanged files are never re-uploaded: blob ids are content hashes, so the same content dedupes.

## Encryption

Envelope encryption, on by default ([ADR 0004](https://github.com/muthuishere/carry/blob/main/docs/adr/0004-encryption.md)):

1. `carry init` generates a random 256-bit **data key** for the repository.
2. The data key is stored in `config` only **wrapped** by your **master key** (32 random bytes, base64, from `CARRY_KEY`, `CARRY_KEY_FILE` or a profile). The master key is never written to the bucket, never prompted for, never printed.
3. Content is **zstd-compressed (level 3), then sealed with XChaCha20-Poly1305** (24-byte random nonce). Large files are sealed in streamed chunks so memory stays bounded.
4. Blobs and manifests are **domain-separated**: blobs use associated data `carry blob v2`; manifests use `carry manifest v2\0<state>`, binding each manifest to its state so one state's manifest cannot be replayed as another's.

The wrong master key fails with `bad_key` (exit 4). A repository created with encryption refuses to be written without it (no silent downgrade).

## Keyed BLAKE3 ids

Object ids are a **keyed BLAKE3** hash of the plaintext, with the hash key derived (HKDF-SHA256) from the repo data key. Consequences:

- Ids are stable, so identical content dedupes and unchanged files are skipped.
- Without the data key, an id reveals nothing: the bucket cannot test "is this file `/etc/passwd` from Debian?".
- Manifest ids use a separate derived key (label `carry manifest id`), so a manifest id can never collide with a blob id.

## Compare-and-swap HEAD

Every commit is:

1. Read `refs/<state>/HEAD` (and its ETag).
2. Load the current manifest, apply the operation (put/rm/mv/env/tool), upload new blobs and the new manifest.
3. Write HEAD with `If-Match: <etag>` (or `If-None-Match: *` for the first version).

If S3 answers **412 Precondition Failed**, someone else committed first: carry reloads HEAD and re-applies the op, with exponential backoff and jitter, up to **40 attempts or 15 s**. Only after that does it give up with `conflict` (exit 5). Two writers can never both win, so nothing is silently overwritten ([ADR 0007](https://github.com/muthuishere/carry/blob/main/docs/adr/0007-single-writer-lease.md)).

`carry rollback` additionally takes a short lease (`refs/<state>/lease`, TTL 15 s) so two whole-tree replacements never interleave.

!!! warning "Your store must support conditional writes"
    AWS S3 supports them. For MinIO, R2, Hetzner, Scaleway and others, run `carry doctor`: it actually tests `If-None-Match` and `If-Match` and reports `conditional_writes`.

Each manifest records a monotonically increasing `seq`, and loading checks HEAD's `seq` equals the manifest's. Containers remember the highest `seq` they have seen per state (`<root>/state-hwm.json`) and **refuse a HEAD that goes backwards**, so an attacker who can write to the bucket cannot quietly replay an old version onto running containers. (Rollback is a *new* version with a higher seq, so it is not affected.)

## Restore: generations and symlinks

`carry run -- <app>` restores into `CARRY_ROOT` (default `/carry`):

```text
/carry/
  gens/<seq>/files/...     one directory per restored version
  gens/<seq>/manifest.json
  current -> gens/<seq>    atomically flipped symlink
  bin/                     tool links (jq -> carry)
  cache/tools/             downloaded, verified tool binaries
  status.json              what `carry status` reports
  state-hwm.json           highest seq seen per state
  .launcher.env            0600, credentials for tool launchers
```

1. Fetch HEAD and the manifest, write every file into `gens/<seq>.tmp/`, verifying each blob.
2. Rename to `gens/<seq>` and atomically swap the `current` symlink. A half-restored tree is never visible.
3. Make every manifest path a symlink into the tree: `/etc/nginx/conf.d/default.conf -> /carry/current/files/etc/nginx/conf.d/default.conf`. Because they point through `current`, a later flip updates every file at once.
4. Keep the newest **2** generations (never deleting `current`); remove stale `.tmp` directories.
5. Paths that vanished from the new version are unlinked, but only if the link still points into carry's own tree. carry never deletes a file it did not create.

Then carry sets the state's env vars (your container's own env still wins over stored defaults), puts `/carry/bin` on `PATH`, strips its secrets from the app's environment, and starts the app.

## Fail-open

([ADR 0005](https://github.com/muthuishere/carry/blob/main/docs/adr/0005-fail-open-and-bounded-memory.md))

- Restore has a budget, `CARRY_RESTORE_BUDGET_MS` (default **2000**). On any error or timeout the app starts from the last good local generation, or from empty, and `carry status` reports `"mode": "degraded"`.
- Without `CARRY_REPO`, `carry run --` just runs your command (`"mode": "disabled"`), so an image with carry in it works with no bucket at all.
- After start, carry polls HEAD every `CARRY_POLL_SECS` (default 30). New version → restore into a new generation → flip → send `CARRY_RELOAD_SIGNAL` (e.g. `HUP`) to the app if set.
- carry forwards `SIGTERM`, `SIGINT`, `SIGHUP`, `SIGQUIT` to the app and exits with the app's exit code.
- Memory is bounded: about two blobs in flight regardless of tree size; target under 20 MiB idle, 64 MiB cap.

## Tools on demand

See [Tools](guides/tools.md). In short: `apps.lock` lists tools; at start carry only creates links (`/carry/bin/jq -> carry`). The first time the app runs `jq`, carry (invoked as `jq`) resolves the version, downloads from `tools/`, verifies the Ed25519 signature and SHA-256 against `CARRY_TOOL_PUBKEY`, caches, and `exec`s it. Start time does not depend on how many tools you list.
