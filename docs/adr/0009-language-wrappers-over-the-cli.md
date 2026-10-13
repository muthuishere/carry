# 0009. Language wrappers call the CLI's JSON interface

- Status: Accepted
- Date: 2026-10-13

## Decision
Thin wrappers for JavaScript/TypeScript, Python, C#, Java and Go run the `carry` binary with `--json`, parse the result, and map exit codes to typed errors. No wrapper reimplements the repository format or talks to S3 itself.

## Consequences
- One implementation of the format, crypto and locking; wrappers stay ~200 lines each.
- The JSON output and exit codes are a public contract (documented in `docs/cli-contract.md`) and versioned.
- Wrappers need the binary on PATH or `CARRY_BIN`.
