# carry

Your container's tools and config, from any S3 bucket.

Containers lose everything added after the image was built. carry restores config files, env defaults and tools from an S3 bucket before your app starts, and every change you make with the `carry` CLI is committed back as a new encrypted version. It works in plain Docker and Kubernetes, needs no privileges, and never stops your app from starting.

- **One static binary**, under 10 MB, in a `FROM scratch` image.
- **Encrypted on the client** (XChaCha20-Poly1305): the bucket only ever sees ciphertext.
- **Versioned**: every change is a version you can `log`, `diff` and `rollback`.
- **Fail-open**: if S3 is down or slow, the app starts from the last good local copy within a 2 s budget.
- **Tools on demand**: `gh`, `kubectl`, `jq` and friends download from a signed tool store the first time they are run.
- **Many replicas**: commits use S3 conditional writes, so concurrent writers never overwrite each other.

## Add it to any image

```dockerfile
FROM nginx:1.27
COPY --from=ghcr.io/muthuishere/carry:0.1.1 /carry /usr/local/bin/carry
ENTRYPOINT ["carry", "run", "--"]
CMD ["nginx", "-g", "daemon off;"]
```

```sh
docker run \
  -e CARRY_REPO=s3://my-bucket/carry \
  -e CARRY_STATE=web-1 \
  -e CARRY_S3_ENDPOINT=https://s3.eu-central-1.amazonaws.com \
  -e AWS_ACCESS_KEY_ID -e AWS_SECRET_ACCESS_KEY \
  -e CARRY_KEY_FILE=/run/secrets/carry-key \
  my-nginx
```

The same image is on Docker Hub as `muthuishere/carry:0.1.1`.

Without `CARRY_REPO`, `carry run --` simply runs your command, so the image still works with no bucket.

## Change state from anywhere

```sh
carry put  web-1:/etc/nginx/conf.d/default.conf ./default.conf
carry env  set web-1 LOG_LEVEL=info
carry tool add web-1 jq@1.7.1
carry log  web-1
carry rollback web-1 3
```

Running containers pick up new versions on their next poll (`CARRY_POLL_SECS`, default 30) and get `CARRY_RELOAD_SIGNAL` if set. Deletes and renames happen only through `carry rm` / `carry mv`; deleting a file inside a container never touches the bucket.

Profiles work like kubectl contexts, in `~/.config/carry/config.yaml`:

```yaml
current-profile: prod
profiles:
  prod:
    repo: s3://my-bucket/carry
    endpoint: https://fsn1.your-objectstorage.com
    credentials: { env: PROD_S3 }      # reads PROD_S3_ACCESS_KEY_ID / PROD_S3_SECRET_ACCESS_KEY
    key: { file: ~/.config/carry/prod.key }
    readonly: false
```

## Publish a tool once, use it everywhere

```sh
carry tool keygen                          # signing key; prints the public key only
carry tool publish jq@1.7.1 --file ./jq-linux-amd64 --platform linux/amd64
carry tool promote jq@1.7.1 --channel stable
```

Containers verify every tool against `CARRY_TOOL_PUBKEY` before running it.

## From code

Thin wrappers over the CLI's JSON contract live in [`sdk/`](sdk): JavaScript, Python, C#, Java and Go.

```python
from carry_sdk import Carry
c = Carry(profile="prod")
c.put("web-1:/etc/app.yaml", b"x: 1")
print(c.get("web-1:/etc/app.yaml"))
```

## Learn more

- CLI contract (JSON output, exit codes, env vars): [`docs/cli-contract.md`](docs/cli-contract.md)
- Design decisions: [`docs/adr`](docs/adr)
- Spec: [`openspec/changes/carry-v0-1`](openspec/changes/carry-v0-1)
- End-to-end example with MinIO: [`examples/`](examples), run `examples/e2e.sh`
- Kubernetes: [`deploy/k8s`](deploy/k8s)

## License

Apache-2.0
