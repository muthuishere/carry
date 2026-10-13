## ADDED Requirements

### Requirement: Run wrapper
`carry run -- <cmd...>` SHALL restore the state, apply env, place tool links on PATH, start `<cmd>` as a supervised child, forward SIGTERM/SIGINT/SIGHUP, and exit with the child's exit code.

#### Scenario: No repo configured
- **WHEN** `CARRY_REPO` is unset
- **THEN** `carry run -- <cmd>` execs `<cmd>` unchanged

### Requirement: Fail-open restore
Restore SHALL never prevent the app from starting. On timeout (default 2 s) or any error it SHALL use the last good local generation, or an empty one, and report status `degraded`.

#### Scenario: Store unreachable
- **WHEN** the S3 endpoint refuses connections
- **THEN** the app starts within the timeout plus 1 s and `carry status --json` shows `degraded`

#### Scenario: Agent panics
- **WHEN** background work panics
- **THEN** the app process keeps running and its exit code is unaffected

### Requirement: Atomic generations
Restore SHALL write into a new generation directory, verify all content, and switch the `current` symlink with a single rename.

#### Scenario: Restore interrupted
- **WHEN** the process is killed mid-restore
- **THEN** `current` still points to the previous complete generation

### Requirement: Follow new versions
A running `carry run` SHALL poll HEAD (default every 30 s) and apply newer versions with an atomic generation switch, then send the configured reload signal if any.

#### Scenario: Change from laptop
- **WHEN** a laptop commits a new config file to the state
- **THEN** the running container shows the new file within one poll interval

### Requirement: Bounded memory
Restore and download SHALL stream with bounded buffers and keep RSS under 64 MiB for any file size.

#### Scenario: Large file
- **WHEN** a 1 GiB file is restored
- **THEN** peak RSS of carry stays under 64 MiB
