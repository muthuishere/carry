# JavaScript / TypeScript

Package `@muthuishere/carry` in [`sdk/js`](https://github.com/muthuishere/carry/tree/main/sdk/js). ESM, Node 18+, zero dependencies, TypeScript types included (`index.d.ts`).

## Install

```sh
git clone https://github.com/muthuishere/carry
npm install ./carry/sdk/js
```

## Example

```js
import { Carry, ConflictError } from "@muthuishere/carry";

const carry = new Carry({ profile: process.env.CARRY_PROFILE });
const state = "web-1";

await carry.envSet(state, { LOG_LEVEL: "info", PORT: "8080" });
await carry.put(`${state}:/etc/app.yaml`, "listen: 8080\n", 0o644);
console.log((await carry.get(`${state}:/etc/app.yaml`)).toString());
await carry.toolAdd(state, "jq@1.7");
for (const v of (await carry.log(state)).versions)
  console.log(v.version, v.op, v.args.join(" "));

try {
  await carry.rollback(state, 3);
} catch (e) {
  if (e instanceof ConflictError) { /* someone else is rolling back; retry later */ }
  else throw e;
}
```

## API

```ts
new Carry({ bin?, profile?, repo?, endpoint?, timeout?, env? })
run(args: string[], input?): Promise<any>
put(path, data: string | Uint8Array, mode?): Promise<PutResult>
get(path): Promise<Buffer>
ls(target) / rm(path) / mv(from, to)
log(state, limit?) / diff(state, v1, v2) / rollback(state, version)
envSet(state, vars) / envUnset(state, keys) / envGet(state, key?)
toolAdd(state, tool, { channel?, prefetch?, required? })
status() / states()
```

Errors: `CarryError` (base, with `code`, `exitCode`), `UsageError`, `UnreachableError`, `RefusedError`, `ConflictError`.

Tests: `cd sdk/js && npm test`. Full example: [`sdk/js/examples/basic.js`](https://github.com/muthuishere/carry/blob/main/sdk/js/examples/basic.js).
