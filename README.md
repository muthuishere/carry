# carry

Your container's tools and config, from any S3 bucket.

Containers lose everything added after the image was built. carry restores config files, env defaults and tools from an S3 bucket before your app starts, and every change you make with the `carry` CLI is committed back as a new encrypted version. Works in plain Docker and Kubernetes, needs no privileges, and never stops your app from starting.

Status: v0.1 in development. Design: [`docs/adr`](docs/adr), spec: [`openspec/changes/carry-v0-1`](openspec/changes/carry-v0-1).
