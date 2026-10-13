# carry SDKs

Thin wrappers that run the `carry` binary with `--json`, parse the envelope and raise typed errors
(ADR 0009, contract in `docs/cli-contract.md`). None of them talk to S3 directly.

Binary lookup: constructor option, then `CARRY_BIN`, then `carry` on `PATH`.
Options: `profile`, `repo`, `endpoint`, `timeout` (seconds), extra `env` for the child process.

| exit | error.code | error |
|---|---|---|
| 2 | usage, readonly, not_found | UsageError |
| 3 | unreachable | UnreachableError |
| 4 | refused, invalid, bad_key | RefusedError |
| 5 | conflict | ConflictError |
| other | internal / unparseable output / missing binary | CarryError (base of all) |

Secrets never go in argv: `envSet` sends values over stdin as `carry env import <state> -`,
file content goes over stdin as `carry put <state:path> -`, and credentials belong in the `env` option.

## JavaScript (`sdk/js`, `@muthuishere/carry`)
```js
import { Carry, ConflictError } from "@muthuishere/carry";
const carry = new Carry({ profile: "dev" });
await carry.put("web-1:/etc/app.yaml", "x: 1");
const bytes = await carry.get("web-1:/etc/app.yaml"); // Buffer
```

## Python (`sdk/python`, `carry-sdk`)
```python
from carry_sdk import Carry, ConflictError
carry = Carry(profile="dev")
carry.put("web-1:/etc/app.yaml", b"x: 1")
assert carry.get("web-1:/etc/app.yaml") == b"x: 1"
```

## C# (`sdk/dotnet`, `Carry.Sdk`)
```csharp
var carry = new CarryClient(new CarryOptions { Profile = "dev" });
await carry.PutAsync("web-1:/etc/app.yaml", "x: 1");
byte[] bytes = await carry.GetAsync("web-1:/etc/app.yaml"); // ConflictException etc. on failure
```

## Java (`sdk/java`, `io.github.muthuishere.carry`)
```java
Carry carry = Carry.builder().profile("dev").build();
carry.put("web-1:/etc/app.yaml", "x: 1");
byte[] bytes = carry.get("web-1:/etc/app.yaml"); // throws CarryException.Conflict etc.
```

## Go (`sdk/go`, `github.com/muthuishere/carry/sdk/go`)
```go
c := carry.New(carry.Options{Profile: "dev"})
_, err := c.Put(ctx, "web-1:/etc/app.yaml", []byte("x: 1"), 0)
data, err := c.Get(ctx, "web-1:/etc/app.yaml")
if errors.Is(err, carry.ErrConflict) { /* retry later */ }
```

Every SDK also has `ls, rm, mv, log, diff, rollback, envSet, envUnset, envGet, toolAdd, status, states`
and a generic `run(args)`, plus an `examples/` program (set env, put a file, read it back, add a tool, print log).

## Tests

All suites run against the stub `sdk/testdata/fake-carry` (python3, state in `$FAKE_CARRY_DB`;
arguments containing `conflict`/`unreachable`/`badkey` produce exit 5/3/4). Set `CARRY_BIN` to
point them at a real binary instead.

```sh
cd sdk/js     && npm test
cd sdk/python && python3 -m unittest discover -s tests
cd sdk/go     && go test ./...
cd sdk/dotnet && dotnet test
cd sdk/java   && gradle test
```
