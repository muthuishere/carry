# CLI reference

Every command and flag below is taken from `src/cli.rs` and the versioned contract in [`docs/cli-contract.md`](https://github.com/muthuishere/carry/blob/main/docs/cli-contract.md). Changing any JSON shape or exit code is a breaking change.

## Global flags

Accepted by every command, before or after the subcommand.

| Flag | Env | Default | Meaning |
|---|---|---|---|
| `--profile <name>` | `CARRY_PROFILE` | `current-profile` | Named profile from the config file |
| `--repo <url>` | `CARRY_REPO` | | `s3://bucket/prefix` |
| `--endpoint <url>` | `CARRY_S3_ENDPOINT` | AWS | S3 endpoint URL |
| `--region <r>` | `CARRY_S3_REGION` | | S3 region |
| `--json` | | off | Print exactly one JSON envelope on stdout |
| `--timeout <secs>` | `CARRY_TIMEOUT` | `30` | Store timeout |
| `--help`, `--version` | | | |

Other environment: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `CARRY_KEY` (base64 of 32 bytes) or `CARRY_KEY_FILE`, `CARRY_CONFIG`, `CARRY_ROOT` (default `/carry`), `CARRY_STATE`, `CARRY_TOOL_PUBKEY`, `CARRY_TOOL_SIGNING_KEY` / `CARRY_TOOL_SIGNING_KEY_FILE`, `CARRY_POLL_SECS` (30), `CARRY_RESTORE_BUDGET_MS` (2000), `CARRY_RELOAD_SIGNAL`, `CARRY_NO_ENCRYPTION=1` (tests only).

Precedence: **flag > env > launcher file > profile > default**. No command ever prompts.

## Output

Without `--json`, results are printed as `key: value` lines and aligned tables. With `--json`:

```json
{"ok": true, "result": { ... }}
{"ok": false, "error": {"code": "conflict", "message": "..."}}
```

Exactly one JSON object on stdout; logs go to stderr.

## Exit codes

| Exit | Meaning | `error.code` |
|---|---|---|
| 0 | ok | |
| 1 | internal error | `internal` |
| 2 | usage, read-only profile, not found | `usage`, `readonly`, `not_found` |
| 3 | store unreachable | `unreachable` |
| 4 | store refused / invalid data / bad key | `refused`, `invalid`, `bad_key` |
| 5 | conflict not resolved after retries (40 attempts or 15 s) | `conflict` |

`run` is the exception: it exits with the child's exit code. See [Troubleshooting](troubleshooting.md) for what to do with each.

## Paths

A state path is `<state>:<absolute path>`, e.g. `web-1:/etc/app.yaml`. The path must start with `/`; the state name must be non-empty.

---

## Repository

### `carry init`

Create the repository (idempotent). Generates and wraps the data key with your master key.

```sh
carry init --json
```
```json
{"ok": true, "result": {"repo": "s3://carry/demo", "created": true, "encrypted": true}}
```

### `carry doctor`

Check reachability, conditional-write support, and whether the repo exists and your key opens it. Read-only.

```sh
carry doctor --json
```
```json
{"ok": true, "result": {"reachable": true, "conditional_writes": true, "encrypted": true,
  "checks": [{"name": "reachable", "ok": true, "detail": "store answered"},
             {"name": "conditional_writes", "ok": true, "detail": "If-None-Match and If-Match honoured"}]}}
```

`encrypted` is `null` when it cannot be determined (e.g. not initialised).

### `carry states`

```sh
carry states --json
```
```json
{"ok": true, "result": {"states": [{"name": "web-1", "version": 7}, {"name": "tools-1", "version": 3}]}}
```

---

## Files

### `carry put <state:path> <file|-> [--mode <octal>]`

Upload a file (or stdin with `-`). Mode defaults to the source file's permission bits, or `0644` for stdin.

```sh
carry put web-1:/etc/nginx/conf.d/default.conf ./default.conf
echo 'x: 1' | carry put web-1:/etc/app.yaml - --mode 0640 --json
```
```json
{"ok": true, "result": {"state": "web-1", "path": "/etc/app.yaml", "version": 8, "size": 5, "uploaded": false}}
```

`uploaded` is `false` when no blob was written (content stored inline, under 4 KiB, or already present).

### `carry get <state:path> [dest|-]`

Without `--json`: raw bytes to stdout or to `dest` (with the stored mode applied).

```sh
carry get web-1:/etc/app.yaml
carry get web-1:/etc/app.yaml ./app.yaml
carry get web-1:/etc/app.yaml --json
```
```json
{"ok": true, "result": {"path": "/etc/app.yaml", "size": 5, "mode": 416, "content_base64": "eDogMQo="}}
```

(`mode` is decimal: 416 = 0o640.)

### `carry ls <state>[:<dir>]`

```sh
carry ls web-1 --json
carry ls web-1:/etc/nginx
```
```json
{"ok": true, "result": {"state": "web-1", "version": 8, "entries": [
  {"path": "/etc/app.yaml", "kind": "file", "size": 5, "mode": 416},
  {"path": "/etc/nginx/conf.d/default.conf", "kind": "file", "size": 812, "mode": 420}]}}
```

### `carry rm <state:path>`

The only way to delete from the bucket's current version. Old versions keep the file.

```sh
carry rm web-1:/etc/app.yaml --json
```
```json
{"ok": true, "result": {"state": "web-1", "path": "/etc/app.yaml", "version": 9}}
```

### `carry mv <state:from> <to>`

`to` is an absolute path or the same `state:path`. Moving between states is refused (`usage`).

```sh
carry mv web-1:/etc/app.yaml /etc/app/config.yaml --json
```
```json
{"ok": true, "result": {"state": "web-1", "from": "/etc/app.yaml", "to": "/etc/app/config.yaml", "version": 10}}
```

---

## History

### `carry log <state> [--limit n]`

```sh
carry log web-1 --limit 3 --json
```
```json
{"ok": true, "result": {"state": "web-1", "versions": [
  {"version": 10, "op": "mv", "args": ["/etc/app.yaml", "/etc/app/config.yaml"], "at": "2026-10-14T09:12:03Z", "by": "me@laptop"},
  {"version": 9, "op": "rm", "args": ["/etc/app.yaml"], "at": "...", "by": "..."}]}}
```

### `carry diff <state> <v1> <v2>`

```sh
carry diff web-1 3 7 --json
```
```json
{"ok": true, "result": {"added": ["/etc/app.yaml"], "removed": [], "changed": ["/etc/nginx/conf.d/default.conf"],
  "env_changed": ["LOG_LEVEL"], "apps_changed": ["jq"]}}
```

### `carry rollback <state> <version>`

Make an old version's tree current again, **as a new version** (history is never rewritten). Takes the state lease for up to 15 s.

```sh
carry rollback web-1 3 --json
```
```json
{"ok": true, "result": {"state": "web-1", "version": 11, "restored_from": 3}}
```

---

## Env

All write commands return `{"state", "version", "keys"}`.

### `carry env set <state> K=V...`

```sh
carry env set web-1 LOG_LEVEL=info PORT=8080 --json
```
```json
{"ok": true, "result": {"state": "web-1", "version": 12, "keys": ["LOG_LEVEL", "PORT"]}}
```

!!! warning
    Values in argv are visible in shell history and `ps`. For secrets use `env import` from stdin.

### `carry env unset <state> K...`

```sh
carry env unset web-1 PORT
```

### `carry env import <state> <file|->`

Import a dotenv-style file (UTF-8, `KEY=value` lines) or stdin.

```sh
carry env import web-1 ./prod.env
printf 'DB_PASSWORD=%s\n' "$DB_PASSWORD" | carry env import web-1 -
```

### `carry env get <state> [K]`

```sh
carry env get web-1 --json
carry env get web-1 LOG_LEVEL
```
```json
{"ok": true, "result": {"state": "web-1", "env": {"LOG_LEVEL": "info"}}}
```

---

## Runtime

### `carry run -- <cmd...>`

Restore `CARRY_STATE`, then run and supervise `<cmd>`. Not JSON; exits with the child's exit code. Without `CARRY_REPO` it just runs the command. See [How it works](how-it-works.md#restore-generations-and-symlinks).

```sh
carry run -- nginx -g 'daemon off;'
```

### `carry status`

Local restore status, read from `<root>/status.json`. Does not contact S3.

```sh
carry status --json
```
```json
{"ok": true, "result": {"state": "web-1", "mode": "ok", "version": 12, "root": "/carry",
  "last_restore_ms": 143, "error": null,
  "tools": [{"name": "jq", "status": "ready"}, {"name": "gh", "status": "failed", "reason": "..."}]}}
```

`mode` is `ok`, `degraded` (S3 failed; running on last good or empty) or `disabled` (no `CARRY_REPO`). Tool `status` is `ready`, `missing` (not downloaded yet) or `failed`.

---

## Tools

See the [Tools guide](guides/tools.md) for the workflow.

### `carry tool keygen [--out <path>]`

```sh
carry tool keygen --json
```
```json
{"ok": true, "result": {"public_key": "...", "private_key_file": "/home/me/.config/carry/tool-signing.key"}}
```

### `carry tool publish <name>@<ver> (--file <path> | --url <url> [--sha256 <hex>]) --platform <os/arch>`

```sh
carry tool publish jq@1.7.1 --file ./jq-linux-amd64 --platform linux/amd64 --json
```
```json
{"ok": true, "result": {"name": "jq", "version": "1.7.1", "platform": "linux/amd64", "sha256": "..."}}
```

### `carry tool promote <name>@<ver> --channel <c>`

```sh
carry tool promote jq@1.7.1 --channel stable --json
```
```json
{"ok": true, "result": {"name": "jq", "channel": "stable", "version": "1.7.1"}}
```

### `carry tool yank <name>@<ver>`

```sh
carry tool yank jq@1.7.1 --json
```
```json
{"ok": true, "result": {"name": "jq", "version": "1.7.1", "yanked": true}}
```

### `carry tool ls [name]`

```sh
carry tool ls --json
```
```json
{"ok": true, "result": {"tools": [{"name": "jq", "versions": ["1.7.1"], "channels": {"stable": "1.7.1"}}]}}
```

### `carry tool add <state> <name>[@ver] [--channel <c>] [--prefetch] [--required]`

```sh
carry tool add web-1 jq@1.7.1 --json
carry tool add web-1 kubectl --channel stable --prefetch
```
```json
{"ok": true, "result": {"state": "web-1", "version": 13, "tool": "jq"}}
```

### `carry tool rm <state> <name>`

```sh
carry tool rm web-1 kubectl
```

Same result shape as `tool add`.

---

## Profiles

Work without a resolvable store. See [Profiles](guides/profiles.md).

| Command | Result |
|---|---|
| `carry profile list` | `{"current": str|null, "profiles": [str]}` |
| `carry profile use <name>` | `{"current": str}` |
| `carry profile current` | `{"current": str}` |
