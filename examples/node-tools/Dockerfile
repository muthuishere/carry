# Tiny node app that shells out to `jq`, provided on demand by carry (state `tools-1`).
ARG CARRY_IMAGE=carry:dev
FROM ${CARRY_IMAGE} AS carry

FROM node:22-alpine
COPY --from=carry /carry /usr/local/bin/carry
WORKDIR /app
COPY server.js .
EXPOSE 3000
ENTRYPOINT ["carry", "run", "--"]
CMD ["node", "server.js"]
