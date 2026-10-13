import json
import os
import tempfile
import unittest

from carry_sdk import (Carry, CarryError, ConflictError, RefusedError,
                       UnreachableError, UsageError)

HERE = os.path.dirname(os.path.abspath(__file__))
os.environ.setdefault("CARRY_BIN", os.path.join(HERE, "..", "..", "testdata", "fake-carry"))


class CarryTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.db = os.path.join(self.tmp.name, "db.json")
        os.environ["FAKE_CARRY_DB"] = self.db

    def tearDown(self):
        self.tmp.cleanup()

    def last_call(self):
        with open(self.db) as f:
            return json.load(f)["last_call"]

    def test_put_get(self):
        c = Carry(profile="dev")
        c.put("web-1:/etc/app.yaml", b"x: 1")
        self.assertEqual(c.get("web-1:/etc/app.yaml"), b"x: 1")
        self.assertIn("--profile", self.last_call()["argv"])

    def test_binary_and_mode(self):
        c = Carry()
        c.put("s:/bin", bytes([0, 255, 1]), mode=0o600)
        self.assertEqual(c.get("s:/bin"), bytes([0, 255, 1]))
        self.assertEqual(c.ls("s")["entries"][0]["mode"], 0o600)

    def test_file_ops(self):
        c = Carry()
        c.put("s:/a", "1")
        c.mv("s:/a", "/b")
        self.assertEqual([e["path"] for e in c.ls("s")["entries"]], ["/b"])
        c.rm("s:/b")
        self.assertEqual(len(c.log("s", limit=2)["versions"]), 2)
        self.assertEqual(c.rollback("s", 1)["restored_from"], 1)
        self.assertEqual(c.diff("s", 1, 2)["added"], [])
        self.assertEqual(c.states(), [{"name": "s", "version": 4}])

    def test_env_not_in_argv(self):
        c = Carry()
        c.env_set("s", {"DB_PASSWORD": "hunter2", "MODE": "prod"})
        self.assertNotIn("hunter2", json.dumps(self.last_call()["argv"]))
        self.assertEqual(c.env_get("s"), {"DB_PASSWORD": "hunter2", "MODE": "prod"})
        self.assertEqual(c.envGet("s", "MODE"), {"MODE": "prod"})
        c.env_unset("s", ["MODE"])
        self.assertEqual(c.env_get("s"), {"DB_PASSWORD": "hunter2"})

    def test_tool_and_status(self):
        c = Carry(env={"CARRY_STATE": "s"})
        c.tool_add("s", "jq@1.7", channel="stable")
        st = c.status()
        self.assertEqual(st["state"], "s")
        self.assertEqual(st["tools"], [{"name": "jq", "status": "ready"}])

    def test_errors(self):
        c = Carry()
        with self.assertRaises(ConflictError) as e:
            c.put("conflict:/a", b"x")
        self.assertEqual((e.exception.exit_code, e.exception.code), (5, "conflict"))
        with self.assertRaises(UnreachableError):
            c.ls("unreachable")
        with self.assertRaises(RefusedError) as e:
            c.ls("badkey")
        self.assertEqual(e.exception.code, "bad_key")
        with self.assertRaises(UsageError) as e:
            c.get("s:/missing")
        self.assertEqual(e.exception.code, "not_found")
        with self.assertRaises(CarryError):
            Carry(bin="/nonexistent/carry").states()

    def test_run(self):
        self.assertEqual(Carry().run(["states"]), {"states": []})


if __name__ == "__main__":
    unittest.main()
