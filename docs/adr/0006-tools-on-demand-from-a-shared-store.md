# 0006. Tools run through carry and download on first use from a shared, signed store

- Status: Accepted
- Date: 2026-10-13

## Decision
- A state lists tools in `apps.lock` (pinned version or channel). They are not downloaded at start.
- Start creates symlinks on PATH (`gh -> carry`). Invoked under another name, carry resolves that tool (local cache → S3 tool store → nothing), verifies its signature and hash, then execs it (busybox-style).
- `carry tool publish name@ver --file|--url --platform os/arch` uploads to `tools/`, signs `tool.json` with an Ed25519 publisher key. Containers verify against `CARRY_TOOL_PUBKEY`.
- `tool promote --channel`, `tool yank`.
- `prefetch: true` / `required: true` per tool change when it is fetched.

## Consequences
Start time is independent of the number of tools. First use pays one download. Tools are per OS/arch, so Intel and ARM replicas both work. apt/apk packages are out of scope for v0.1.
