## ADDED Requirements

### Requirement: Explicit write commands commit synchronously
`put`, `rm`, `mv`, `env set`, `env unset`, `env import` and `rollback` SHALL return success only after a new version is committed to HEAD.

#### Scenario: Put a file
- **WHEN** `carry put web-1:/etc/app.yaml ./app.yaml` succeeds
- **THEN** `carry get web-1:/etc/app.yaml` from any machine returns the same bytes

#### Scenario: Remove is explicit
- **WHEN** a file is deleted from local disk without `carry rm`
- **THEN** the repository is unchanged

### Requirement: Read and history commands
The system SHALL provide `get`, `ls`, `log`, `diff <v1> <v2>` and `status` for a state.

#### Scenario: Rollback
- **WHEN** `carry rollback web-1 v3` runs
- **THEN** a new version is committed whose tree equals v3 and `log` shows the op as `rollback v3`

### Requirement: Non-interactive contract
No command SHALL prompt. Every command SHALL accept `--json` and `--timeout`, read secrets only from env, files or profiles, and never print secret values.

#### Scenario: JSON output
- **WHEN** any command runs with `--json`
- **THEN** stdout is a single JSON object with `ok` and either `result` or `error {code, message}`

#### Scenario: Exit codes
- **WHEN** a command fails
- **THEN** it exits 2 for usage errors, 3 for an unreachable store, 4 for a refused or invalid store response (incl. bad key), 5 for an unresolved conflict

### Requirement: Env defaults
The system SHALL store env vars per state, encrypted, and `carry run` SHALL apply them to the app unless the variable is already set in the container environment.

#### Scenario: Container overrides stored default
- **WHEN** state has `LOG_LEVEL=info` and the container sets `LOG_LEVEL=debug`
- **THEN** the app sees `debug`
