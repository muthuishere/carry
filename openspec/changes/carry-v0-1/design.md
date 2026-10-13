## Context
Decisions are recorded in `docs/adr/0001`–`0009`. This document fixes module boundaries and interfaces so several people (or agents) can build in parallel.

## Goals / Non-Goals

**Goals:**
- One static binary: CLI, `run` wrapper, tool launcher.
- Works against AWS S3 and MinIO; tested end-to-end with Docker Compose.
- Wrappers in five languages over the JSON contract.

**Non-Goals (v0.1):**
- Pack files (one blob per file for now), garbage collection beyond `gc --dry-run`.
- Leader push, request forwarding, Kubernetes Lease, Unix-socket API (CLI goes straight to S3; the in-container agent only restores and polls).
- Folder watching, apt/apk capture, lazy FUSE mode, admin web UI.

## Decisions

### Crate layout (single crate, `src/`)
| Module | Owns | Public surface |
|---|---|---|
| `store.rs` | S3 access via `object_store` | `Store::from_settings`, `get`, `get_opt`, `put`, `put_if_absent`, `put_if_match -> Result<ETag, Conflict>`, `head`, `list` |
| `crypto.rs` | keys, AEAD, keyed ids, signatures | `MasterKey::load`, `DataKey::{generate,wrap,unwrap}`, `seal`, `open`, `blob_id`, `sign`, `verify` |
| `repo.rs` | config, blobs, manifests, HEAD, lease | `Repo::open_or_init`, `read_head(state)`, `load_manifest`, `commit(state, parent, op, tree)`, `put_blob`, `get_blob_to(writer)`, `acquire_lease` |
| `model.rs` | shared types | `Manifest`, `Entry {path, kind, mode, size, blob|inline, target}`, `Op`, `Head`, `AppsLock`, `ToolRef` |
| `ops.rs` | state commands as pure functions over `Repo` | `put`, `get`, `ls`, `rm`, `mv`, `log`, `diff`, `rollback`, `env_*`, with retry-on-conflict loop |
| `restore.rs` | generation dirs, atomic flip, fail-open | `restore(repo, state, root, budget) -> RestoreReport` |
| `run.rs` | supervisor, env merge, PATH links, polling | `run(settings, cmd) -> exit code` |
| `tools.rs` | tool store, launcher, argv0 dispatch | `publish`, `promote`, `yank`, `ls`, `add`, `launch(name, args) -> !` |
| `settings.rs` | profiles, env, flags precedence | `Settings::resolve(cli)` |
| `cli.rs` | clap definitions, JSON output, exit codes | `main` entry, `Output::emit` |

### Key formats
- Manifest JSON (then zstd, then sealed): `{format:1, parent, seq, op:{kind,args,at,by}, entries:{path: Entry}, env:{K:V}, apps:{...}}`. Env and `apps.lock` live inside the manifest, so they version with everything else.
- Blob object: `nonce(24) || ciphertext`. Id: hex keyed BLAKE3 (key = HKDF(data key, "carry blob id")).
- HEAD: plaintext `{"manifest":"<id>","seq":N}`.
- Local layout under `CARRY_ROOT` (default `/carry`): `gens/<seq>/files/...`, `current -> gens/<seq>`, `bin/` (tool links), `cache/tools/<sha256>`, `status.json`.
- Files are materialised at their absolute paths by symlinking from the real path into `current/files/<path>`; `carry run` creates those links for every path in the manifest, parent dirs as needed. A path that already exists as a real file in the image is replaced only if it is listed in the state.

### Exit codes
0 ok, 1 internal, 2 usage/readonly, 3 store unreachable, 4 store refused/invalid/bad key, 5 conflict after retries. Documented in `docs/cli-contract.md`.

### Concurrency
Every write = load HEAD → apply op to tree → upload missing blobs → put manifest → `put_if_match(HEAD)`; on conflict retry up to 5 times with jitter. Lease used by `rollback` and `tool publish` only in v0.1.

## Risks / Trade-offs
- [Store lacks conditional writes] → `carry doctor` probes it; commands refuse to write with a clear error.
- [One object per file is slow for 10k+ files] → acceptable for config/tool state; pack format is next.
- [Symlinked config paths confuse apps that rename-replace their own config] → documented; such paths belong in the state root directly.
- [Lost master key] → escrow key support in `config` from day one.

## Open Questions
- Docker Hub org name: using `muthuishere/carry` unless told otherwise.
