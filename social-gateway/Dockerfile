FROM node:22-bookworm-slim

WORKDIR /opt/switch-development-gateway
COPY --chown=node:node proxy.mjs ./proxy.mjs

USER node

ENV LISTEN_PORT=8081 \
    MAIN_UPSTREAM_URL=http://cst:8080 \
    SOCIAL_UPSTREAM_URL=http://social:8090 \
    FORWARDED_PROTO=https

EXPOSE 8081

ENTRYPOINT ["node", "/opt/switch-development-gateway/proxy.mjs"]
