# Python

Package `carry-sdk` (module `carry_sdk`) in [`sdk/python`](https://github.com/muthuishere/carry/tree/main/sdk/python). Python 3.8+, no dependencies.

## Install

```sh
pip install "git+https://github.com/muthuishere/carry#subdirectory=sdk/python"
```

## Example

```python
import os
from carry_sdk import Carry, ConflictError

carry = Carry(profile=os.environ.get("CARRY_PROFILE"))
state = "web-1"

carry.env_set(state, {"LOG_LEVEL": "info", "PORT": "8080"})
carry.put(f"{state}:/etc/app.yaml", "listen: 8080\n", mode=0o644)
print(carry.get(f"{state}:/etc/app.yaml").decode())
carry.tool_add(state, "jq@1.7")
for v in carry.log(state)["versions"]:
    print(v["version"], v["op"], " ".join(v["args"]))

try:
    carry.rollback(state, 3)
except ConflictError:
    pass  # retry later
```

## API

```python
Carry(*, bin=None, profile=None, repo=None, endpoint=None, timeout=None, env=None)
run(args, input=None)
put(path, data, mode=None) -> dict
get(path) -> bytes
ls(target) / rm(path) / mv(src, dest)
log(state, limit=None) / diff(state, v1, v2) / rollback(state, version)
env_set(state, values) / env_unset(state, keys) / env_get(state, key=None) -> dict
tool_add(state, tool, channel=None, prefetch=False, required=False)
status() -> dict
states() -> list
```

Errors: `CarryError` (`.code`, `.exit_code`), `UsageError`, `UnreachableError`, `RefusedError`, `ConflictError`.

Tests: `cd sdk/python && python3 -m unittest discover -s tests`.
