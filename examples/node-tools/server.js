// GET /jq      -> runs `jq` (a carry PATH link, fetched on first use) on a fixed document
// GET /env     -> the env vars the app sees (carry applies state defaults)
// GET /healthz
const http = require("http");
const { execFile } = require("child_process");

const server = http.createServer((req, res) => {
  if (req.url === "/healthz") return res.end("ok\n");
  if (req.url === "/env") {
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({
      GREETING: process.env.GREETING ?? null,
      LOG_LEVEL: process.env.LOG_LEVEL ?? null,
    }) + "\n");
  }
  if (req.url === "/jq") {
    const child = execFile("jq", ["-r", '.tool + "-" + (.n|tostring)'], (err, stdout, stderr) => {
      if (err) { res.statusCode = 500; return res.end(`jq failed: ${err.message}\n${stderr}`); }
      res.end(stdout);
    });
    child.stdin.end(JSON.stringify({ tool: "jq", n: 42 }));
    return;
  }
  res.statusCode = 404;
  res.end("not found\n");
});

server.listen(3000, () => console.log("node-tools listening on :3000"));
for (const sig of ["SIGTERM", "SIGINT"]) {
  process.on(sig, () => server.close(() => process.exit(0)));
}
