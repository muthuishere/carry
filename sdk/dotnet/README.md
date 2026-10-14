# Carry.Sdk

.NET SDK for [carry](https://github.com/muthuishere/carry): your container's tools and config, from any S3 bucket.
A thin wrapper that runs the `carry` binary with `--json` and raises typed errors
(contract: [docs/cli-contract.md](https://github.com/muthuishere/carry/blob/main/docs/cli-contract.md)). It needs the `carry` CLI
installed (constructor option, then `CARRY_BIN`, then `carry` on `PATH`).

```sh
dotnet add package Carry.Sdk
```

```csharp
var carry = new CarryClient(new CarryOptions { Profile = "dev" });
await carry.PutAsync("web-1:/etc/app.yaml", "x: 1");
byte[] bytes = await carry.GetAsync("web-1:/etc/app.yaml");
```

Methods: `put, get, ls, rm, mv, log, diff, rollback, envSet, envUnset, envGet, toolAdd, status, states, run`.
Errors: UsageError (exit 2), UnreachableError (3), RefusedError (4), ConflictError (5), CarryError (base).
Secrets never go in argv: values and file contents are sent over stdin.

Full docs: [sdk/README.md](https://github.com/muthuishere/carry/blob/main/sdk/README.md). License: Apache-2.0.
