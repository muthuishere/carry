# The published carry image is FROM scratch (no shell, no cp), so an init container
# cannot copy /carry out of it on its own. This 2 MB image adds busybox so it can.
#   docker build -t YOUR_REGISTRY/carry-installer:0.1.1 deploy/helm/carry/installer
ARG CARRY_IMAGE=docker.io/muthuishere/carry:0.1.1
FROM ${CARRY_IMAGE} AS carry
FROM busybox:1.36.1-musl
COPY --from=carry /carry /carry
ENTRYPOINT ["/bin/sh", "-c", "cp /carry \"$1\"/carry && chmod 0755 \"$1\"/carry", "install"]
CMD ["/opt/carry"]
