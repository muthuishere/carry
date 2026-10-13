## ADDED Requirements

### Requirement: Wrappers for five languages
The repo SHALL ship wrappers under `sdk/` for JavaScript/TypeScript, Python, C#, Java and Go exposing put, get, ls, rm, mv, log, rollback, envSet, envGet, toolAdd, status, and a generic `run(args)`.

#### Scenario: Python put/get
- **WHEN** `Carry(profile="dev").put("web-1:/etc/app.yaml", b"x: 1")` then `get` is called
- **THEN** `get` returns `b"x: 1"`

### Requirement: Typed errors from exit codes
Each wrapper SHALL raise/return a language-native error carrying the CLI's `code`, `message` and exit status.

#### Scenario: Conflict
- **WHEN** the CLI exits 5
- **THEN** the wrapper raises its ConflictError equivalent

### Requirement: Examples
Each wrapper SHALL include a runnable example and tests that run against a real `carry` binary and MinIO when available, otherwise against a stub binary.

#### Scenario: Wrapper tests
- **WHEN** the wrapper's test command runs with `CARRY_BIN` pointing to a stub
- **THEN** tests pass
