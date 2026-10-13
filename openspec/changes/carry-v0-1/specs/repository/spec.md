## ADDED Requirements

### Requirement: Initialise repository on first use
The system SHALL create `config` with a wrapped data key when the target prefix is empty and a master key is supplied.

#### Scenario: Empty bucket
- **WHEN** any write command runs against an empty prefix with `CARRY_KEY` set
- **THEN** `config` is created with `If-None-Match: *` and the command proceeds

#### Scenario: Two initialisers race
- **WHEN** two processes initialise the same empty prefix at once
- **THEN** exactly one `config` is written and the other process reloads and uses it

### Requirement: Encrypted content
The system SHALL store blobs and manifests only as XChaCha20-Poly1305 ciphertext of zstd-compressed data, named by keyed BLAKE3 ids, unless the repo was created with `--no-encryption`.

#### Scenario: Provider reads the bucket
- **WHEN** an object under `blobs/` or `manifests/` is downloaded without the key
- **THEN** it contains no plaintext file names or contents

#### Scenario: Wrong key
- **WHEN** a command runs with a master key that does not unwrap the data key
- **THEN** it exits with code 4 and a `bad_key` error without modifying the repo

### Requirement: Versions advance by compare-and-swap
The system SHALL move `refs/<state>/HEAD` only with a conditional PUT, so two concurrent commits cannot both succeed.

#### Scenario: Concurrent commits
- **WHEN** two writers commit to the same state from the same parent
- **THEN** one succeeds and the other gets a conflict, reloads HEAD, reapplies its operation and commits as the next version

### Requirement: Deduplicated uploads
The system SHALL NOT upload a blob whose id already exists in the repo.

#### Scenario: Unchanged file
- **WHEN** a file is put with identical content to an existing blob
- **THEN** no blob PUT is issued and only a new manifest and HEAD are written

### Requirement: Integrity verification
The system SHALL verify each blob's authentication tag and id after download and reject mismatches.

#### Scenario: Tampered blob
- **WHEN** a blob's bytes are modified in S3
- **THEN** restore rejects it and falls back as defined by container-runtime
