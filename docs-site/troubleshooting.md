# Troubleshooting

Start with:

```sh
carry doctor --json            # from where you run the CLI
carry status --json            # inside the container
```

Every CLI failure has an exit code and, with `--json`, an `error.code`.

## Exit 1: `internal`

A bug or an unexpected local condition (e.g. cannot start the runtime, cannot read stdin). Re-run with `--json` and open an issue with the message.

## Exit 2: `usage`, `readonly`, `not_found`

| Message | Fix |
|---|---|
| `expected <state>:<path>, got '...'` | Use `web-1:/abs/path` |
| `path must be absolute in '...'` | Paths start with `/` |
| `empty state name` | Put the state before the colon |
| `expected name@version, got '...'` | `tool publish/promote/yank` need `name@version` |
| `expected K=V, got '...'` | `env set` takes `KEY=value` pairs |
| `invalid --mode '...' (expected octal like 0644)` | Use octal digits |
| `mv cannot move between states` | Copy with `get` + `put` instead |
| `unknown profile '...'` | Check `carry profile list` and `~/.config/carry/config.yaml` |
| `readonly` | The selected profile has `readonly: true`; use another profile |
| `not_found` | The local source file, the state, the path or the version does not exist |
| `... already exists; refusing to overwrite` (keygen) | Pass `--out` or move the old key |
| `no signing key (set CARRY_TOOL_SIGNING_KEY or CARRY_TOOL_SIGNING_KEY_FILE)` | Export the signing key file path for `tool publish/promote/yank` |

## Exit 3: `unreachable`

The store did not answer within `--timeout` / `CARRY_TIMEOUT` (default 30 s), DNS failed, or the endpoint is wrong.

- Check `CARRY_S3_ENDPOINT` (scheme and port: `http://minio:9000`), `CARRY_S3_REGION`.
- From inside Docker, `localhost` is the container. Use the service name.
- Try a longer `--timeout`.

## Exit 4: `refused`, `invalid`, `bad_key`

| Code | Likely cause |
|---|---|
| `bad_key` | Wrong `CARRY_KEY` / `CARRY_KEY_FILE` for this repo (`master key does not unwrap the data key`), or a key that is not base64 of 32 bytes. Check precedence: `CARRY_KEY` beats `CARRY_KEY_FILE` beats the profile |
| `refused` | The store rejected the request: wrong credentials (403), bucket missing, or policy denies the prefix |
| `invalid` | Data failed to parse or authenticate (tampered or corrupted object, non-UTF-8 env file, bad tool signature) |

## Exit 5: `conflict`

Another writer kept winning for 40 attempts or 15 s, or a `rollback` found another live holder of the state lease. Wait and retry. If it happens constantly, check that your store really supports conditional writes (`carry doctor`) and that you do not have a runaway loop committing.

## `carry run` / container problems

| Symptom | Cause / fix |
|---|---|
| App starts but files are missing; `status.mode` = `degraded` | Restore failed or exceeded `CARRY_RESTORE_BUDGET_MS`. Read `status.error`. Check network, credentials, key |
| `status.mode` = `disabled` | `CARRY_REPO` is not set in the container |
| Files restored for the wrong state | `CARRY_STATE` not set; carry fell back to `$HOSTNAME` |
| `HEAD of '...' went back to version N after version M was seen` | Someone restored an older HEAD object in the bucket. carry keeps the current generation. Fix the bucket, or commit a new version (rollback creates a higher one) |
| Changes never arrive | Check `CARRY_POLL_SECS`, `carry: poll:` lines in logs, and that the state name matches |
| App does not reload | Set `CARRY_RELOAD_SIGNAL` (e.g. `HUP`); `carry: warning: unknown CARRY_RELOAD_SIGNAL, reload disabled` means the name is wrong |
| Env change not visible | Env is applied at start; restart the container |
| A path is not linked (warning in logs) | Permissions (non-root user) or an existing non-symlink file in the way |
| Tool fails with `refused: ... unsigned` / `bad signature` / `untrusted key` | `CARRY_TOOL_PUBKEY` does not match the publisher key, or channels were written by an old version. Re-promote with the current key |
| Tool fails with `... is yanked` | Pick another version or channel |
| Tool `missing` in status | Normal: on-demand tools download on first run. Use `--prefetch` or `--required` to fetch earlier |
| `required tool prefetch exceeded the restore budget; starting anyway` | Required tool did not download in time; raise the budget or prefetch instead |
| Pod not ready during S3 outage | Remove any probe that checks carry; probe the app only |
