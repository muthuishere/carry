# Security model

This page says what carry protects, against whom, and where the limits are. It describes **v0.1.1** as implemented, not the roadmap.

## Assets

1. **Content**: your config files, env values and tool binaries.
2. **Integrity**: that a container runs the version you committed, and only tools you signed.
3. **Availability**: that your app starts even when the store is not there.

## What the bucket (and the provider) sees

| Visible | Not visible |
|---|---|
| That a repository exists under a prefix, its `config` (format version, cipher ids, *wrapped* data key) | Your master key or the data key |
| State names (`refs/<state>/HEAD` paths) | File names, paths, modes, env keys and values, tool lists (all inside encrypted manifests) |
| Number, sizes and timing of objects; HEAD `seq` numbers | File contents (XChaCha20-Poly1305 after zstd) |
| Tool names and versions in `tools/<name>/<version>/` paths, `channels.json` | Tool binaries' contents (encrypted) |
| Lease holder identity during a rollback | Whether two repos contain the same file: ids are keyed BLAKE3, keyed per repo |

!!! warning "Name things accordingly"
    State names and tool names appear in object keys in plaintext. Do not encode secrets or customer names in them. Sizes and change frequency are also visible; compression happens before encryption, so very small files may leak approximate length.

## Threats and mitigations

| Threat | Mitigation |
|---|---|
| Provider or bucket reader reads your data | Client-side envelope encryption; keys never leave the client |
| Attacker with bucket **write** access modifies a blob or manifest | AEAD authentication: tampered objects fail to decrypt; manifests are bound to their state (`carry manifest v2\0<state>`) and blobs/manifests are domain-separated |
| Attacker swaps one state's manifest into another | Manifest AAD includes the state name; loading checks the manifest's `state` field |
| Attacker replays an older HEAD (silent rollback) | HEAD `seq` must match the manifest's `seq`; containers keep a per-state high-water mark in `<root>/state-hwm.json` and refuse to go backwards (status `degraded`) |
| Attacker uploads a malicious tool | Tools run only if `tool.json` is signed by `CARRY_TOOL_PUBKEY` (Ed25519) and the binary's SHA-256 matches |
| Attacker replays an old `channels.json` or un-yanks a version | Channels are signed and carry a `seq`; containers refuse lower seqs and remember yanks |
| Concurrent writers overwrite each other | Conditional PUT on HEAD; losers retry, never clobber |
| Downgrade to unencrypted | An encrypted repo refuses unencrypted writes; `CARRY_NO_ENCRYPTION=1` is for tests only and recorded in `config` |
| A planted symlink in a restored tree redirects writes | Restore refuses to traverse anything that is not a real directory; only removes links that point into carry's own tree |
| S3 outage takes the app down | Fail-open with a time budget; never a readiness dependency |
| Credentials leak through the app's env, crash dumps, `/proc/<pid>/environ` | `carry run` strips `CARRY_KEY`, `CARRY_KEY_FILE`, `AWS_*` credentials and signing keys from the app's environment |

## What the app can see

Tools launched by the app need to reach the bucket, so carry writes `<root>/.launcher.env` (mode `0600`, owned by the user running carry) with the repo, endpoint, region, S3 credentials, `CARRY_KEY` and `CARRY_TOOL_PUBKEY`, and gives the app `CARRY_LAUNCHER_ENV` pointing at it. The file is ignored unless it is a regular file owned by the current user with no group/other permissions.

**Be clear about the limit:** the app runs as the same user and *can read this file*. Stripping keeps secrets out of `/proc/<pid>/environ`, crash dumps and child processes that log their env. It does **not** protect against a hostile or compromised app. Anyone who can exec into the container can read the master key and S3 credentials.

## Recommendations

- Use **one master key per trust boundary** (e.g. per environment), and separate S3 prefixes or buckets.
- Give containers **read-only S3 credentials** scoped to their prefix whenever they do not need to write. Only your CLI/CI needs write access.
- Enable bucket server-side encryption and versioning as a second layer.
- Keep the tool signing private key off servers; only `CARRY_TOOL_PUBKEY` goes to containers.
- Store the master key in a Secret / secret manager and mount it as a file (`CARRY_KEY_FILE`), not as a `docker run -e` value.

## Key management: limits in v0.1

!!! danger "Back up the master key"
    The data key is stored only wrapped by your master key. **Lose the master key and the data is unrecoverable.** There is no recovery path.

- **One master key per repository.** The format can hold several wrapped copies of the data key (the design calls for primary + escrow keys), but v0.1's `carry init` wraps exactly one and there is no command to add another.
- **No key rotation command.** The design makes master-key rotation cheap (rewrite only `config`), but there is no `carry key rotate` in v0.1. To rotate today: create a new repository (new prefix) with the new key, re-`put` the current state, and switch containers over.
- **No data-key rotation.** Rotating the data key would mean re-encrypting all objects; not supported.
- **Tool signing key rotation**: publish with a new key and update `CARRY_TOOL_PUBKEY`; containers trust exactly one public key at a time, so plan a cut-over.

## Other limits

- **No per-user authorization inside carry.** Anyone with S3 write credentials *and* the master key can commit. Authorization is your S3 IAM policy.
- **Audit `by` field is self-reported** by the writing client, not authenticated.
- **`env set K=V` puts values in argv** (shell history, `ps`). Use `env import <state> -` for secrets; the SDKs already do.
- **Lease for rollback has a 15 s TTL** with no renewal in v0.1.
- Not audited by a third party.

Report security issues privately via [GitHub security advisories](https://github.com/muthuishere/carry/security/advisories/new).
