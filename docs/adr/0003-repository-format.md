# 0003. Content-addressed repository with a compare-and-swap HEAD

- Status: Accepted
- Date: 2026-10-13

## Decision
Layout under a prefix (`s3://bucket/prefix/`):

```
config                   plaintext: format version, kdf/cipher ids, wrapped data keys
blobs/<ab>/<id>          encrypted, compressed file contents (one blob per file in v0.1)
manifests/<id>           encrypted: parent, op, created_at, tree of entries
refs/<state>/HEAD        {"manifest": id, "seq": n}, written with If-Match / If-None-Match
refs/<state>/lease       {"holder", "expires"}, written with conditional PUT
tools/...                shared tool store (0006)
```

- `<state>` is a named state inside one repo (e.g. `web-1`), so one bucket holds many apps.
- Blob id = keyed BLAKE3 of the plaintext (key derived from the repo data key), so ids leak nothing.
- Small files (< 4 KiB) are stored inline in the manifest.
- HEAD only moves with a conditional PUT. 412 means someone else committed: reload and retry the op.
- v0.1 stores one blob per file; pack files (kopia-style, 8–32 MiB) are a planned format bump once measured.

## Consequences
Unchanged files are never re-uploaded. Any version can be restored. Requires S3 conditional writes (AWS confirmed; MinIO/R2/others tested by `carry doctor`).
