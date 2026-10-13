"""python examples/basic.py  (uses $CARRY_BIN or `carry` on PATH)"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from carry_sdk import Carry  # noqa: E402

carry = Carry(profile=os.environ.get("CARRY_PROFILE"))
state = "web-1"

carry.env_set(state, {"LOG_LEVEL": "info", "PORT": "8080"})
carry.put(f"{state}:/etc/app.yaml", "listen: 8080\n", mode=0o644)
print(carry.get(f"{state}:/etc/app.yaml").decode())
carry.tool_add(state, "jq@1.7")
for v in carry.log(state)["versions"]:
    print(v["version"], v["op"], " ".join(v["args"]))
