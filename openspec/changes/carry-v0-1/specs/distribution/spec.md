## ADDED Requirements

### Requirement: Scratch image
The project SHALL publish `muthuishere/carry:<version>` and `:latest` for linux/amd64 and linux/arm64, containing only `/carry`.

#### Scenario: COPY into an app image
- **WHEN** a Dockerfile does `COPY --from=muthuishere/carry:0.1.0 /carry /usr/local/bin/carry` and `ENTRYPOINT ["carry","run","--"]`
- **THEN** the image starts its CMD with restored state

### Requirement: Runnable examples
The repo SHALL include a Docker Compose setup with MinIO and example apps proving: config persistence across container replacement, env defaults, on-demand tool, laptop-to-container update, fail-open with S3 down.

#### Scenario: End-to-end script
- **WHEN** `./examples/e2e.sh` runs on a machine with Docker
- **THEN** every case passes and the script exits 0
