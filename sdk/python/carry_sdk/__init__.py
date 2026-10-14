"""Thin wrapper over the `carry` CLI JSON interface (docs/cli-contract.md)."""
from __future__ import annotations

import base64
import json
import os
import subprocess
from typing import Any, Dict, List, Mapping, Optional, Sequence, Union

__all__ = ["Carry", "CarryError", "UsageError", "UnreachableError", "RefusedError", "ConflictError"]
__version__ = "0.1.1"


class CarryError(Exception):
    def __init__(self, code: str, message: str, exit_code: int):
        super().__init__(f"{code}: {message}")
        self.code = code
        self.message = message
        self.exit_code = exit_code


class UsageError(CarryError):
    pass


class UnreachableError(CarryError):
    pass


class RefusedError(CarryError):
    pass


class ConflictError(CarryError):
    pass


_BY_CODE = {
    "usage": UsageError, "readonly": UsageError, "not_found": UsageError,
    "unreachable": UnreachableError,
    "refused": RefusedError, "invalid": RefusedError, "bad_key": RefusedError,
    "conflict": ConflictError,
}
_BY_EXIT = {2: UsageError, 3: UnreachableError, 4: RefusedError, 5: ConflictError}


def _error(code: str, message: str, exit_code: int) -> CarryError:
    cls = _BY_CODE.get(code) or _BY_EXIT.get(exit_code) or CarryError
    return cls(code, message, exit_code)


class Carry:
    def __init__(self, *, bin: Optional[str] = None, profile: Optional[str] = None,
                 repo: Optional[str] = None, endpoint: Optional[str] = None,
                 timeout: Optional[int] = None, env: Optional[Mapping[str, str]] = None):
        self.bin = bin or os.environ.get("CARRY_BIN") or "carry"
        self.profile, self.repo, self.endpoint, self.timeout = profile, repo, endpoint, timeout
        self.env = dict(env or {})

    def _global_args(self) -> List[str]:
        a = ["--json"]
        if self.profile:
            a += ["--profile", self.profile]
        if self.repo:
            a += ["--repo", self.repo]
        if self.endpoint:
            a += ["--endpoint", self.endpoint]
        if self.timeout is not None:
            a += ["--timeout", str(self.timeout)]
        return a

    def run(self, args: Sequence[str], input: Optional[bytes] = None) -> Any:
        """Run any carry command with --json and return the envelope's result."""
        try:
            p = subprocess.run([self.bin, *self._global_args(), *args], input=input or b"",
                               capture_output=True, env={**os.environ, **self.env})
        except OSError as e:
            raise CarryError("internal", f"cannot run {self.bin}: {e}", 1) from e
        try:
            envelope = json.loads(p.stdout)
        except ValueError:
            msg = (p.stderr or p.stdout).decode(errors="replace").strip()
            raise CarryError("internal", f"unparseable output: {msg}", p.returncode)
        if envelope.get("ok"):
            return envelope.get("result")
        err = envelope.get("error") or {}
        raise _error(err.get("code", "internal"), err.get("message", "unknown error"), p.returncode)

    def put(self, path: str, data: Union[bytes, str], mode: Optional[int] = None) -> Dict[str, Any]:
        args = ["put", path, "-"]
        if mode is not None:
            args += ["--mode", format(mode, "o")]
        return self.run(args, data.encode() if isinstance(data, str) else data)

    def get(self, path: str) -> bytes:
        return base64.b64decode(self.run(["get", path])["content_base64"])

    def ls(self, target: str) -> Dict[str, Any]:
        return self.run(["ls", target])

    def rm(self, path: str) -> Dict[str, Any]:
        return self.run(["rm", path])

    def mv(self, src: str, dest: str) -> Dict[str, Any]:
        return self.run(["mv", src, dest])

    def log(self, state: str, limit: Optional[int] = None) -> Dict[str, Any]:
        return self.run(["log", state] + (["--limit", str(limit)] if limit is not None else []))

    def diff(self, state: str, v1: int, v2: int) -> Dict[str, Any]:
        return self.run(["diff", state, str(v1), str(v2)])

    def rollback(self, state: str, version: int) -> Dict[str, Any]:
        return self.run(["rollback", state, str(version)])

    def env_set(self, state: str, values: Mapping[str, str]) -> Dict[str, Any]:
        """Values travel over stdin (`env import <state> -`), never argv."""
        lines = []
        for k, v in values.items():
            if "=" in k or "\n" in k or "\n" in str(v):
                raise UsageError("usage", f"invalid env entry {k}", 2)
            lines.append(f"{k}={v}\n")
        return self.run(["env", "import", state, "-"], "".join(lines).encode())

    def env_unset(self, state: str, keys: Sequence[str]) -> Dict[str, Any]:
        return self.run(["env", "unset", state, *keys])

    def env_get(self, state: str, key: Optional[str] = None) -> Dict[str, str]:
        return self.run(["env", "get", state] + ([key] if key else []))["env"]

    def tool_add(self, state: str, tool: str, channel: Optional[str] = None,
                 prefetch: bool = False, required: bool = False) -> Dict[str, Any]:
        args = ["tool", "add", state, tool]
        if channel:
            args += ["--channel", channel]
        if prefetch:
            args.append("--prefetch")
        if required:
            args.append("--required")
        return self.run(args)

    def status(self) -> Dict[str, Any]:
        return self.run(["status"])

    def states(self) -> List[Dict[str, Any]]:
        return self.run(["states"])["states"]

    # camelCase aliases matching the other SDKs
    envSet, envUnset, envGet, toolAdd = env_set, env_unset, env_get, tool_add
