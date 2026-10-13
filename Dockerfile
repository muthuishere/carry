# syntax=docker/dockerfile:1.7
# carry: a FROM-scratch image containing only /carry (+ a CA bundle).
# Cross-compiles on the build host with cargo-zigbuild (no QEMU), so
#   docker buildx build --platform linux/amd64,linux/arm64 .
# is fast on any builder.

ARG ZIGBUILD_IMAGE=ghcr.io/rust-cross/cargo-zigbuild:0.20.1

FROM --platform=$BUILDPLATFORM ${ZIGBUILD_IMAGE} AS builder
ARG TARGETARCH
ARG RUST_VERSION=1.90.0
RUN rustup toolchain install "$RUST_VERSION" --profile minimal \
      --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl \
 && rustup default "$RUST_VERSION"
WORKDIR /src
# Dependency layer: build against a dummy main so deps are cached.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-musl ;; \
      arm64) echo aarch64-unknown-linux-musl ;; \
      *) echo "unsupported TARGETARCH=$TARGETARCH" >&2; exit 1 ;; \
    esac > /target-triple
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,id=carry-target-${TARGETARCH} \
    cargo zigbuild --release --locked --target "$(cat /target-triple)"
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,id=carry-target-${TARGETARCH} \
    touch src/main.rs \
 && cargo zigbuild --release --locked --target "$(cat /target-triple)" \
 && cp "target/$(cat /target-triple)/release/carry" /carry

FROM --platform=$BUILDPLATFORM alpine:3.20 AS certs
RUN apk add --no-cache ca-certificates

FROM scratch
COPY --from=certs /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /carry /carry
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
ENTRYPOINT ["/carry"]
