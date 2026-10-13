export interface CarryOptions {
  /** Path to the carry binary. Defaults to $CARRY_BIN, then `carry` on PATH. */
  bin?: string;
  profile?: string;
  repo?: string;
  endpoint?: string;
  /** Seconds, passed as --timeout. */
  timeout?: number;
  /** Extra environment for the child (put secrets here, never in args). */
  env?: Record<string, string>;
}

export class CarryError extends Error {
  code: string;
  exitCode: number;
  constructor(code: string, message: string, exitCode: number);
}
export class UsageError extends CarryError {}
export class UnreachableError extends CarryError {}
export class RefusedError extends CarryError {}
export class ConflictError extends CarryError {}

export interface PutResult { state: string; path: string; version: number; size: number; uploaded: boolean }
export interface LsEntry { path: string; kind: "file" | "symlink"; size: number; mode: number }
export interface LsResult { state: string; version: number; entries: LsEntry[] }
export interface RmResult { state: string; path: string; version: number }
export interface MvResult { state: string; from: string; to: string; version: number }
export interface LogEntry { version: number; op: string; args: string[]; at: string; by: string }
export interface LogResult { state: string; versions: LogEntry[] }
export interface DiffResult { added: string[]; removed: string[]; changed: string[]; env_changed: string[]; apps_changed: string[] }
export interface RollbackResult { state: string; version: number; restored_from: number }
export interface EnvResult { state: string; version: number; keys: string[] }
export interface ToolResult { state: string; version: number; tool: string }
export interface StatusResult {
  state: string | null; mode: "ok" | "degraded" | "disabled"; version: number | null; root: string;
  last_restore_ms: number | null; error: string | null;
  tools: { name: string; status: "ready" | "missing" | "failed" }[];
}
export interface StateInfo { name: string; version: number }

export class Carry {
  constructor(opts?: CarryOptions);
  run<T = any>(args: string[], input?: string | Uint8Array): Promise<T>;
  put(path: string, data: string | Uint8Array, mode?: number | string): Promise<PutResult>;
  get(path: string): Promise<Buffer>;
  ls(target: string): Promise<LsResult>;
  rm(path: string): Promise<RmResult>;
  mv(from: string, to: string): Promise<MvResult>;
  log(state: string, limit?: number): Promise<LogResult>;
  diff(state: string, v1: number, v2: number): Promise<DiffResult>;
  rollback(state: string, version: number): Promise<RollbackResult>;
  envSet(state: string, vars: Record<string, string>): Promise<EnvResult>;
  envUnset(state: string, keys: string[]): Promise<EnvResult>;
  envGet(state: string, key?: string): Promise<Record<string, string>>;
  toolAdd(state: string, tool: string, opts?: { channel?: string; prefetch?: boolean; required?: boolean }): Promise<ToolResult>;
  status(): Promise<StatusResult>;
  states(): Promise<StateInfo[]>;
}
