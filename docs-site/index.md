# carry

**Your container's tools and config, from any S3 bucket.**

Containers lose everything added after the image was built. carry restores config files, env defaults and tools from an S3-compatible bucket *before your app starts*, and every change you make with the `carry` CLI is committed back as a new, encrypted, immutable version. It works in plain Docker and Kubernetes, needs no privileges, and never stops your app from starting.

<div class="grid" markdown>

- **One static binary**, under 10 MB, shipped in a `FROM scratch` image (`linux/amd64`, `linux/arm64`).
- **Encrypted on the client** with XChaCha20-Poly1305: the bucket only ever sees ciphertext and opaque ids.
- **Versioned**: every change is a version you can `log`, `diff` and `rollback`.
- **Fail-open**: if S3 is down or slow, the app starts from the last good local copy within a 2 s budget.
- **Tools on demand**: `gh`, `kubectl`, `jq` and friends download from a signed tool store the first time they are run.
- **Many replicas**: commits use S3 conditional writes, so concurrent writers never overwrite each other.

</div>

## The 30-second version

```dockerfile
FROM nginx:1.27
COPY --from=ghcr.io/muthuishere/carry:0.1.1 /carry /usr/local/bin/carry
ENTRYPOINT ["carry", "run", "--"]
CMD ["nginx", "-g", "daemon off;"]
```

```sh
# from your laptop (or CI)
carry put  web-1:/etc/nginx/conf.d/default.conf ./default.conf
carry env  set web-1 LOG_LEVEL=info
carry tool add web-1 jq@1.7.1
carry log  web-1
carry rollback web-1 3
```

Every running container whose `CARRY_STATE=web-1` picks up the new version on its next poll (default 30 s) and, if you set `CARRY_RELOAD_SIGNAL=HUP`, nginx reloads.

## What carry is not

- **Not a filesystem mount.** It restores to ordinary local disk; no FUSE, no privileges ([ADR 0001](https://github.com/muthuishere/carry/blob/main/docs/adr/0001-snapshot-sync-not-filesystem-mount.md)).
- **Not a directory watcher.** The CLI is the only write path. A shell `rm` inside a container never touches the bucket ([ADR 0002](https://github.com/muthuishere/carry/blob/main/docs/adr/0002-cli-is-the-only-write-path.md)).
- **Not a database or a secrets manager.** It is built for config files, env defaults and tool binaries: small trees that change by hand or by CI.

## Where to go next

| I want to... | Read |
|---|---|
| Try it in 5 minutes on my laptop | [Quickstart](quickstart.md) |
| Understand the repo format and crypto | [How it works](how-it-works.md) |
| Add it to an image | [Docker guide](guides/docker.md) |
| Run it on a cluster | [Kubernetes guide](guides/kubernetes.md) |
| Ship `jq`/`kubectl` to every container | [Tools](guides/tools.md) |
| Look up a command or its JSON | [CLI reference](cli.md) |
| Drive it from code | [SDKs](sdk/index.md) |
| Know what it protects against | [Security model](security.md) |

Version documented: **v0.1.1**. Images: `ghcr.io/muthuishere/carry:0.1.1` and `docker.io/muthuishere/carry:0.1.1`. Static binaries: [GitHub releases](https://github.com/muthuishere/carry/releases). License: Apache-2.0.
