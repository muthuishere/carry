# SDKs

Thin wrappers for **JavaScript/TypeScript, Python, C#, Java and Go** live in [`sdk/`](https://github.com/muthuishere/carry/tree/main/sdk). Each one runs the `carry` binary with `--json`, parses the envelope and raises typed errors ([ADR 0009](https://github.com/muthuishere/carry/blob/main/docs/adr/0009-language-wrappers-over-the-cli.md)). None of them talk to S3 or implement crypto: one implementation of the format, five thin clients.

## Requirements

The `carry` binary must be available. Lookup order: constructor option (`bin`) → `CARRY_BIN` → `carry` on `PATH`.

## Common options

`profile`, `repo`, `endpoint`, `timeout` (seconds) and extra `env` for the child process. Put credentials (`AWS_*`, `CARRY_KEY`) in `env` or the parent environment, never in arguments.

## Common methods

`put, get, ls, rm, mv, log, diff, rollback, envSet, envUnset, envGet, toolAdd, status, states`, plus a generic `run(args)` for anything else (e.g. `tool publish`). Naming follows each language's conventions (`env_set` in Python, `EnvSetAsync` in C#).

## Errors

| exit | `error.code` | Error type |
|---|---|---|
| 2 | `usage`, `readonly`, `not_found` | UsageError |
| 3 | `unreachable` | UnreachableError |
| 4 | `refused`, `invalid`, `bad_key` | RefusedError |
| 5 | `conflict` | ConflictError |
| other | internal / unparseable output / missing binary | CarryError (base of all) |

## Secrets never go in argv

- `envSet` sends values over stdin as `carry env import <state> -`.
- `put` sends file content over stdin as `carry put <state:path> -`.

## Install status

!!! note
    v0.1 SDKs are not yet published to npm / PyPI / NuGet / Maven Central. Install from the repository as shown on each page. Go works directly via `go get`.

## Pages

- [JavaScript / TypeScript](javascript.md)
- [Python](python.md)
- [C# / .NET](dotnet.md)
- [Java](java.md)
- [Go](go.md)
