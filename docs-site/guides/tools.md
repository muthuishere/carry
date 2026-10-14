# Tools

carry can ship CLI tools (`jq`, `kubectl`, `gh`, your own binaries) to every container without baking them into images. Tools live in a **shared, signed tool store** under `tools/` in the repository and are downloaded **on first use** ([ADR 0006](https://github.com/muthuishere/carry/blob/main/docs/adr/0006-tools-on-demand-from-a-shared-store.md)).

## The flow

```text
publisher (once)                         state owner                 container
carry tool keygen                        carry tool add web-1 jq     carry run -- app
carry tool publish jq@1.7.1 --file ...   (writes apps.lock)          /carry/bin/jq -> carry
carry tool promote jq@1.7.1 --channel stable                          app runs `jq` → carry resolves,
                                                                       downloads, verifies, caches, execs
```

## 1. Create a signing key

```sh
carry tool keygen
```

```json
{"public_key": "u3k...=", "private_key_file": "/home/me/.config/carry/tool-signing.key"}
```

- Writes the Ed25519 private key to `~/.config/carry/tool-signing.key` (or `--out <path>`) and **never prints it**. Refuses to overwrite an existing file.
- The `public_key` is what containers trust: set it as `CARRY_TOOL_PUBKEY` everywhere.
- Publishing, promoting and yanking need the private key via `CARRY_TOOL_SIGNING_KEY_FILE` (or `CARRY_TOOL_SIGNING_KEY`):

```sh
export CARRY_TOOL_SIGNING_KEY_FILE=~/.config/carry/tool-signing.key
```

## 2. Publish a version, per platform

```sh
# from a local file
carry tool publish jq@1.7.1 --file ./jq-linux-amd64 --platform linux/amd64
carry tool publish jq@1.7.1 --file ./jq-linux-arm64 --platform linux/arm64

# or let carry download it, optionally pinning the checksum
carry tool publish jq@1.7.1 \
  --url https://github.com/jqlang/jq/releases/download/jq-1.7.1/jq-linux-amd64 \
  --sha256 "$JQ_SHA256" \
  --platform linux/amd64
```

`--file` and `--url` are mutually exclusive; `--sha256` is only valid with `--url` and fails the publish on mismatch. The upload is encrypted like everything else, and a `tool.json` describing it is signed with your key.

!!! note
    Take `$JQ_SHA256` from the upstream project's release page.

## 3. Promote to a channel

```sh
carry tool promote jq@1.7.1 --channel stable
carry tool ls jq
```

```json
{"tools": [{"name": "jq", "versions": ["1.7.1"], "channels": {"stable": "1.7.1"}}]}
```

Channels are a signed `channels.json` per tool. Containers refuse unsigned or older (replayed) channel files.

## 4. Add tools to a state

```sh
carry tool add web-1 jq                       # follows channel "stable" (the default)
carry tool add web-1 jq --channel beta        # follows another channel
carry tool add web-1 jq@1.7.1                 # pinned version
carry tool add web-1 kubectl --prefetch       # download in the background right after start
carry tool add web-1 gh --required            # try to download BEFORE the app starts
carry tool rm  web-1 gh
```

| Option | When the binary is fetched |
|---|---|
| *(none)* | The first time the app runs it |
| `--prefetch` | In the background right after the app starts |
| `--required` | Before the app starts, within what is left of the restore budget. Still fail-open: if it cannot be fetched in time the app starts anyway and `carry status` shows the tool as `failed` |

## 5. Yank a bad version

```sh
carry tool yank jq@1.7.1
```

Containers refuse a yanked version, and remember yanks they have seen even if the bucket is later rolled back.

## On-demand launcher, in detail

At start, carry creates `/carry/bin/<tool> -> carry` for every tool in the state's `apps.lock` and prepends `/carry/bin` to the app's `PATH`. When the app runs `jq`:

1. The `carry` binary sees it was invoked as `jq` (busybox-style).
2. It resolves the version: pinned version, or the channel (`stable` by default) from the **signed** `channels.json`.
3. It looks in the local cache (`/carry/cache/tools/`), then the tool store in S3.
4. It verifies the **Ed25519 signature** of `tool.json` against `CARRY_TOOL_PUBKEY` and the **SHA-256** of the binary, picks the build for its own platform (`linux/amd64` or `linux/arm64`), and checks the version is not yanked.
5. It caches and `exec`s the real binary with the original arguments.

The launcher reaches the bucket using `/carry/.launcher.env` (see [Security](../security.md#what-the-app-can-see)), because carry strips the credentials from the app's own environment.

If any check fails, the tool does not run and an error explains why (e.g. `refused: jq channels.json is unsigned`). The app itself is unaffected.

## Limits (v0.1)

- One static binary per tool and platform. No archives, no dependency trees.
- `apt` / `apk` packages are out of scope.
- No deletion of published versions from the store (yank hides them from containers).
