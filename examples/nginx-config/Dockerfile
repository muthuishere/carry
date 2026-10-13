# nginx whose default.conf and index page come from carry state `web-1`.
ARG CARRY_IMAGE=carry:dev
FROM ${CARRY_IMAGE} AS carry

FROM nginx:1.27-alpine
COPY --from=carry /carry /usr/local/bin/carry
ENTRYPOINT ["carry", "run", "--"]
CMD ["nginx", "-g", "daemon off;"]
