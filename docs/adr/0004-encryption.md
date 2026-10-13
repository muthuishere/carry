# 0004. Client-side envelope encryption on by default

- Status: Accepted
- Date: 2026-10-13

## Decision
- A random 256-bit data key per repo encrypts blobs and manifests with XChaCha20-Poly1305 after zstd compression.
- The data key is stored in `config` only wrapped by one or more master keys (primary + escrow), supplied via `CARRY_KEY` / `CARRY_KEY_FILE` or a profile. Never prompted, never printed.
- Bucket server-side encryption is recommended as a second layer.
- `--no-encryption` exists for testing only and is recorded in `config`.

## Consequences
The provider sees only ciphertext and opaque ids. Losing every master key loses the data; hence escrow keys. Rotating a master key rewrites only `config`.
