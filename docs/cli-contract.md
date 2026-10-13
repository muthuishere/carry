# carry CLI contract (v0.1)

Wrappers and scripts depend on this file. Changing it is a breaking change.

## Global flags
`--profile <name>`, `--repo s3://bucket/prefix`, `--endpoint <url>`, `--region <r>`, `--json`, `--timeout <secs>` (default 30).
Env: `CARRY_PROFILE`, `CARRY_TIMEOUT`, `CARRY_REPO`, `CARRY_S3_ENDPOINT`, `CARRY_S3_REGION`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `CARRY_KEY` (base64 32 bytes) or `CARRY_KEY_FILE`, `CARRY_CONFIG`, `CARRY_ROOT` (default `/carry`), `CARRY_STATE` (state name used by `run`), `CARRY_TOOL_PUBKEY`, `CARRY_TOOL_SIGNING_KEY` / `CARRY_TOOL_SIGNING_KEY_FILE`, `CARRY_POLL_SECS` (default 30), `CARRY_RESTORE_BUDGET_MS` (default 2000), `CARRY_RELOAD_SIGNAL` (e.g. `HUP`), `CARRY_NO_ENCRYPTION=1` (tests only).
Precedence: flag > env > profile > default. No command ever prompts.

`run` strips `CARRY_KEY`, `CARRY_KEY_FILE`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `CARRY_TOOL_SIGNING_KEY` and `CARRY_TOOL_SIGNING_KEY_FILE` from the app's environment. Tool launchers (children of the app) still reach the repo through `<root>/.launcher.env`: a `KEY=value` file (mode 0600, owned by the user running carry) holding `CARRY_REPO`, `CARRY_S3_ENDPOINT`, `CARRY_S3_REGION`, the AWS credentials, `CARRY_KEY` and `CARRY_TOOL_PUBKEY`; the app gets `CARRY_LAUNCHER_ENV=<root>/.launcher.env`. When `CARRY_LAUNCHER_ENV` is set, carry reads that file after the real env and before the profile (flag > env > launcher file > profile > default); the file is ignored unless it is a regular file owned by the current user with no group/other permissions. Note that the app runs as the same user and can read the file: this keeps secrets out of `/proc/<pid>/environ`, crash dumps and child processes that log their env, not out of a hostile app.

## JSON envelope (`--json`)
Success: `{"ok": true, "result": <command-specific>}`
Failure: `{"ok": false, "error": {"code": "<code>", "message": "<text>"}}`
Exactly one JSON object on stdout. Logs go to stderr.

## Exit codes
| code | meaning | error.code values |
|---|---|---|
| 0 | ok | |
| 1 | internal error | internal |
| 2 | usage, read-only profile, not found | usage, readonly, not_found |
| 3 | store unreachable | unreachable |
| 4 | store refused / invalid data / bad key | refused, invalid, bad_key |
| 5 | conflict not resolved after retries | conflict |

## Paths
A state path is `<state>:<absolute path>`, e.g. `web-1:/etc/app.yaml`.

## Commands and `result` shapes
| command | result |
|---|---|
| `init` | `{"repo": str, "created": bool, "encrypted": bool}` |
| `doctor` | `{"reachable": bool, "conditional_writes": bool, "encrypted": bool|null, "checks": [{"name": str, "ok": bool, "detail": str}]}` |
| `put <state:path> <file|-> [--mode 0644]` | `{"state": str, "path": str, "version": int, "size": int, "uploaded": bool}` |
| `get <state:path> [dest|-]` | to stdout as raw bytes without `--json`; with `--json`: `{"path": str, "size": int, "mode": int, "content_base64": str}` |
| `ls <state>[:<dir>]` | `{"state": str, "version": int, "entries": [{"path": str, "kind": "file"|"symlink", "size": int, "mode": int}]}` |
| `rm <state:path>` | `{"state": str, "path": str, "version": int}` |
| `mv <state:from> <to>` | `{"state": str, "from": str, "to": str, "version": int}` |
| `log <state> [--limit n]` | `{"state": str, "versions": [{"version": int, "op": str, "args": [str], "at": str, "by": str}]}` |
| `diff <state> <v1> <v2>` | `{"added": [str], "removed": [str], "changed": [str], "env_changed": [str], "apps_changed": [str]}` |
| `rollback <state> <version>` | `{"state": str, "version": int, "restored_from": int}` |
| `env set <state> K=V...` / `env unset <state> K...` / `env import <state> <file|->` | `{"state": str, "version": int, "keys": [str]}` |
| `env get <state> [K]` | `{"state": str, "env": {K: V}}` |
| `states` | `{"states": [{"name": str, "version": int}]}` |
| `status` | `{"state": str|null, "mode": "ok"|"degraded"|"disabled", "version": int|null, "root": str, "last_restore_ms": int|null, "error": str|null, "tools": [{"name": str, "status": "ready"|"missing"|"failed", "reason"?: str}]}` (`failed` comes from `<root>/cache/tools/status.json`, written by tool prefetch) |
| `run -- <cmd...>` | not JSON; exits with the child's exit code |
| `tool publish <name>@<ver> (--file p | --url u [--sha256 h]) --platform os/arch` | `{"name": str, "version": str, "platform": str, "sha256": str}` |
| `tool promote <name>@<ver> --channel c` | `{"name": str, "channel": str, "version": str}` |
| `tool yank <name>@<ver>` | `{"name": str, "version": str, "yanked": true}` |
| `tool ls [name]` | `{"tools": [{"name": str, "versions": [str], "channels": {c: ver}}]}` |
| `tool add <state> <name>[@ver] [--channel c] [--prefetch] [--required]` / `tool rm <state> <name>` | `{"state": str, "version": int, "tool": str}` |
| `tool keygen` | `{"public_key": str, "private_key_file": str}` (writes key file, never prints private key) |
| `profile list` | `{"current": str|null, "profiles": [str]}` |
| `profile use <name>` / `profile current` | `{"current": str}` |
