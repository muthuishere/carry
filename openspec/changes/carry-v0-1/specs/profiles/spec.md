## ADDED Requirements

### Requirement: Named profiles
The CLI SHALL read `~/.config/carry/config.yaml` (or `CARRY_CONFIG`) with `current-profile` and `profiles` entries holding repo, endpoint, region, credential references, key reference and `readonly`.

#### Scenario: Switch profile
- **WHEN** `carry profile use dev` runs
- **THEN** later commands without `--profile` use `dev`

### Requirement: Precedence
Settings SHALL resolve as flag, then `CARRY_*` env, then selected profile, then defaults.

#### Scenario: Env overrides profile
- **WHEN** profile sets a repo and `CARRY_REPO` is set
- **THEN** `CARRY_REPO` is used

### Requirement: Read-only profiles
A profile with `readonly: true` SHALL refuse write commands with exit code 2.

#### Scenario: Write on readonly
- **WHEN** `carry put` runs under a readonly profile
- **THEN** nothing is written and the error names the profile
