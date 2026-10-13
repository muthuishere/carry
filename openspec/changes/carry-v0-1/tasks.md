## 1. Scaffold
- [x] 1.1 Cargo crate, module stubs, shared `model.rs`, CI workflow (fmt, clippy, test)
- [x] 1.2 `docs/cli-contract.md` with JSON envelope and exit codes

## 2. Repository core (store, crypto, repo)
- [x] 2.1 `store.rs` on object_store with conditional put (If-None-Match / If-Match) and tests against MinIO
- [x] 2.2 `crypto.rs`: master/data keys, XChaCha20-Poly1305 seal/open, keyed blob ids, Ed25519 sign/verify, unit tests
- [x] 2.3 `repo.rs`: init race-safe, blobs dedup, manifests, HEAD CAS, lease
- [x] 2.4 `carry doctor` conditional-write probe

## 3. State commands and CLI
- [x] 3.1 `settings.rs` profiles + precedence + readonly
- [x] 3.2 `ops.rs` put/get/ls/rm/mv/log/diff/rollback/env with conflict retry
- [x] 3.3 `cli.rs` clap, `--json`, `--timeout`, exit codes, profile subcommands

## 4. Container runtime
- [x] 4.1 `restore.rs` generation dirs, verify, atomic flip, fail-open budget
- [x] 4.2 `run.rs` supervisor, signal forwarding, env merge, path links, HEAD polling
- [x] 4.3 bounded memory streaming; RSS test with 1 GiB file

## 5. Tool store
- [x] 5.1 publish/promote/yank/ls/add
- [x] 5.2 argv0 launcher with verify + exec

## 6. Distribution and examples
- [x] 6.1 musl static builds amd64/arm64, scratch Dockerfile, buildx
- [x] 6.2 `examples/` compose with MinIO + nginx config app + node app using jq + e2e.sh covering spec scenarios
- [x] 6.3 Kubernetes manifests (init container + native sidecar)
- [x] 6.4 README quick start

## 7. Language wrappers
- [x] 7.1 JS/TS (`sdk/js`), 7.2 Python (`sdk/python`), 7.3 C# (`sdk/dotnet`), 7.4 Java (`sdk/java`), 7.5 Go (`sdk/go`): wrapper, example, tests with stub binary

## 8. Verification and release
- [x] 8.1 Adversarial review of failure modes and crypto
- [ ] 8.2 e2e green (done on Linux, 20/20), publish Docker Hub image
