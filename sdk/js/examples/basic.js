// node examples/basic.js   (uses $CARRY_BIN or `carry` on PATH)
import { Carry } from "../index.js";

const carry = new Carry({ profile: process.env.CARRY_PROFILE });
const state = "web-1";

await carry.envSet(state, { LOG_LEVEL: "info", PORT: "8080" });
await carry.put(`${state}:/etc/app.yaml`, "listen: 8080\n", 0o644);
console.log((await carry.get(`${state}:/etc/app.yaml`)).toString());
await carry.toolAdd(state, "jq@1.7");
for (const v of (await carry.log(state)).versions) console.log(v.version, v.op, v.args.join(" "));
