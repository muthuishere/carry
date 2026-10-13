// Thin wrapper over the `carry` CLI JSON interface (docs/cli-contract.md).
import { execFile } from "node:child_process";

export class CarryError extends Error {
  constructor(code, message, exitCode) {
    super(message);
    this.name = "CarryError";
    this.code = code;
    this.exitCode = exitCode;
  }
}
export class UsageError extends CarryError { constructor(...a) { super(...a); this.name = "UsageError"; } }
export class UnreachableError extends CarryError { constructor(...a) { super(...a); this.name = "UnreachableError"; } }
export class RefusedError extends CarryError { constructor(...a) { super(...a); this.name = "RefusedError"; } }
export class ConflictError extends CarryError { constructor(...a) { super(...a); this.name = "ConflictError"; } }

const BY_CODE = {
  usage: UsageError, readonly: UsageError, not_found: UsageError,
  unreachable: UnreachableError,
  refused: RefusedError, invalid: RefusedError, bad_key: RefusedError,
  conflict: ConflictError,
};
const BY_EXIT = { 2: UsageError, 3: UnreachableError, 4: RefusedError, 5: ConflictError };

function toError(code, message, exitCode) {
  const Cls = BY_CODE[code] || BY_EXIT[exitCode] || CarryError;
  return new Cls(code, message, exitCode);
}

function dotenv(vars) {
  return Object.entries(vars).map(([k, v]) => {
    if (/[=\n]/.test(k) || String(v).includes("\n")) throw new UsageError("usage", `invalid env entry ${k}`, 2);
    return `${k}=${v}\n`;
  }).join("");
}

export class Carry {
  /** @param {import('./index').CarryOptions} [opts] */
  constructor(opts = {}) {
    this.bin = opts.bin || process.env.CARRY_BIN || "carry";
    this.opts = opts;
  }

  _globalArgs() {
    const a = ["--json"];
    const o = this.opts;
    if (o.profile) a.push("--profile", o.profile);
    if (o.repo) a.push("--repo", o.repo);
    if (o.endpoint) a.push("--endpoint", o.endpoint);
    if (o.timeout != null) a.push("--timeout", String(o.timeout));
    return a;
  }

  /** Run any carry command with --json; resolves to the envelope's `result`. */
  run(args, input) {
    return new Promise((resolve, reject) => {
      const child = execFile(this.bin, [...this._globalArgs(), ...args], {
        env: { ...process.env, ...(this.opts.env || {}) },
        maxBuffer: 256 * 1024 * 1024,
        encoding: "utf8",
      }, (err, stdout, stderr) => {
        const exitCode = err ? (typeof err.code === "number" ? err.code : 1) : 0;
        let env;
        try { env = JSON.parse(stdout); } catch {
          const msg = err && typeof err.code === "string" ? err.message : (stderr || stdout || "no output").trim();
          return reject(new CarryError("internal", `carry: unparseable output: ${msg}`, exitCode));
        }
        if (env.ok) return resolve(env.result);
        const e = env.error || {};
        reject(toError(e.code || "internal", e.message || "unknown error", exitCode));
      });
      if (input != null) child.stdin.end(input); else child.stdin.end();
    });
  }

  put(path, data, mode) {
    const args = ["put", path, "-"];
    if (mode != null) args.push("--mode", typeof mode === "number" ? mode.toString(8) : String(mode));
    return this.run(args, typeof data === "string" ? Buffer.from(data) : data);
  }
  async get(path) {
    const r = await this.run(["get", path]);
    return Buffer.from(r.content_base64, "base64");
  }
  ls(target) { return this.run(["ls", target]); }
  rm(path) { return this.run(["rm", path]); }
  mv(from, to) { return this.run(["mv", from, to]); }
  log(state, limit) { return this.run(limit != null ? ["log", state, "--limit", String(limit)] : ["log", state]); }
  diff(state, v1, v2) { return this.run(["diff", state, String(v1), String(v2)]); }
  rollback(state, version) { return this.run(["rollback", state, String(version)]); }
  // Values go over stdin (`env import <state> -`), never argv.
  envSet(state, vars) { return this.run(["env", "import", state, "-"], dotenv(vars)); }
  envUnset(state, keys) { return this.run(["env", "unset", state, ...keys]); }
  async envGet(state, key) {
    const r = await this.run(key ? ["env", "get", state, key] : ["env", "get", state]);
    return r.env;
  }
  toolAdd(state, tool, opts = {}) {
    const a = ["tool", "add", state, tool];
    if (opts.channel) a.push("--channel", opts.channel);
    if (opts.prefetch) a.push("--prefetch");
    if (opts.required) a.push("--required");
    return this.run(a);
  }
  status() { return this.run(["status"]); }
  async states() { return (await this.run(["states"])).states; }
}
