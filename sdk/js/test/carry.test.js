import { test, beforeEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { Carry, ConflictError, UnreachableError, RefusedError, UsageError, CarryError } from "../index.js";

const here = dirname(fileURLToPath(import.meta.url));
process.env.CARRY_BIN ||= join(here, "..", "..", "testdata", "fake-carry");
let db;
beforeEach(() => { db = join(mkdtempSync(join(tmpdir(), "carry-js-")), "db.json"); process.env.FAKE_CARRY_DB = db; });
const lastCall = () => JSON.parse(readFileSync(db, "utf8")).last_call;

test("put/get round trip", async () => {
  const c = new Carry({ profile: "dev" });
  const r = await c.put("web-1:/etc/app.yaml", "x: 1");
  assert.equal(r.version, 1);
  assert.deepEqual(await c.get("web-1:/etc/app.yaml"), Buffer.from("x: 1"));
  assert.ok(lastCall().argv.includes("--profile"));
});

test("binary data and mode", async () => {
  const c = new Carry();
  await c.put("s:/bin", new Uint8Array([0, 255, 1]), 0o600);
  assert.deepEqual([...(await c.get("s:/bin"))], [0, 255, 1]);
  assert.equal((await c.ls("s")).entries[0].mode, 0o600);
});

test("ls rm mv log rollback diff states", async () => {
  const c = new Carry();
  await c.put("s:/a", "1");
  await c.mv("s:/a", "/b");
  assert.deepEqual((await c.ls("s")).entries.map((e) => e.path), ["/b"]);
  await c.rm("s:/b");
  assert.equal((await c.log("s", 2)).versions.length, 2);
  assert.equal((await c.rollback("s", 1)).restored_from, 1);
  assert.deepEqual((await c.diff("s", 1, 2)).added, []);
  assert.deepEqual(await c.states(), [{ name: "s", version: 4 }]);
});

test("env values never appear in argv", async () => {
  const c = new Carry();
  await c.envSet("s", { DB_PASSWORD: "hunter2", MODE: "prod" });
  assert.ok(!JSON.stringify(lastCall().argv).includes("hunter2"));
  assert.deepEqual(await c.envGet("s"), { DB_PASSWORD: "hunter2", MODE: "prod" });
  assert.deepEqual(await c.envGet("s", "MODE"), { MODE: "prod" });
  await c.envUnset("s", ["MODE"]);
  assert.deepEqual(await c.envGet("s"), { DB_PASSWORD: "hunter2" });
});

test("toolAdd and status with extra env", async () => {
  const c = new Carry({ env: { CARRY_STATE: "s" } });
  await c.toolAdd("s", "jq@1.7", { channel: "stable" });
  const st = await c.status();
  assert.equal(st.state, "s");
  assert.deepEqual(st.tools, [{ name: "jq", status: "ready" }]);
});

test("typed errors", async () => {
  const c = new Carry();
  await assert.rejects(c.put("conflict:/a", "x"), (e) => e instanceof ConflictError && e.exitCode === 5 && e.code === "conflict");
  await assert.rejects(c.ls("unreachable"), (e) => e instanceof UnreachableError && e.exitCode === 3);
  await assert.rejects(c.ls("badkey"), (e) => e instanceof RefusedError && e.code === "bad_key");
  await assert.rejects(c.get("s:/missing"), (e) => e instanceof UsageError && e.code === "not_found");
  await assert.rejects(c.get("s:/missing"), CarryError);
});

test("missing binary is a CarryError", async () => {
  await assert.rejects(new Carry({ bin: "/nonexistent/carry" }).states(), CarryError);
});

test("generic run", async () => {
  assert.deepEqual(await new Carry().run(["states"]), { states: [] });
});
