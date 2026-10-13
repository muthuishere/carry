# 0008. Distribution: a scratch image to COPY from, plus static binaries

- Status: Accepted
- Date: 2026-10-13

## Decision
- One static Rust binary (`x86_64`/`aarch64` musl, rustls), Apache-2.0.
- Image `docker.io/muthuishere/carry:<ver>` built FROM scratch containing only `/carry`, multi-arch. Use: `COPY --from=muthuishere/carry:0.1 /carry /usr/local/bin/carry` and `ENTRYPOINT ["carry","run","--"]`.
- Without `CARRY_REPO`, `carry run` just execs the app, so images stay usable with no bucket.
- Local CLI uses kubectl-style profiles in `~/.config/carry/config.yaml`.
