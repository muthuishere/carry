## ADDED Requirements

### Requirement: Publish signed tools
`carry tool publish <name>@<ver> (--file <path> | --url <url> [--sha256 <hex>]) --platform <os/arch>` SHALL upload the binary to the shared tool store and write a `tool.json` signed with the Ed25519 key from `CARRY_TOOL_SIGNING_KEY`.

#### Scenario: Publish then list
- **WHEN** a tool is published
- **THEN** `carry tool ls --json` lists it with version and platforms

### Requirement: Channels and yank
The system SHALL support `tool promote <name>@<ver> --channel <c>` and `tool yank <name>@<ver>`.

#### Scenario: Yanked version on a channel
- **WHEN** the version a channel points to is yanked
- **THEN** launching via that channel fails with a clear error naming the yank

### Requirement: On-demand launch
Tools listed in a state's `apps.lock` SHALL be exposed as symlinks to carry; invoking one SHALL download (if missing), verify signature and hash, and exec the real binary with the original arguments, stdin, stdout and exit code.

#### Scenario: First use
- **WHEN** the app runs `jq --version` and jq is not on local disk
- **THEN** jq is fetched from the store, verified, cached, and its output returned

#### Scenario: Bad signature
- **WHEN** `tool.json` is not signed by `CARRY_TOOL_PUBKEY`
- **THEN** the tool is refused and nothing is executed

### Requirement: Add tool to a state
`carry tool add <state> <name>[@ver|--channel c] [--prefetch|--required]` SHALL update the state's `apps.lock` as a committed version.

#### Scenario: Add jq
- **WHEN** `carry tool add web-1 jq@1.7.1` runs
- **THEN** the next `carry run` exposes `jq` on PATH
