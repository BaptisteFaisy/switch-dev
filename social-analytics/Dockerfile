FROM node:22-bookworm-slim

WORKDIR /opt/switch-social

COPY --chown=node:node server.mjs ./server.mjs
COPY --chown=node:node time.mjs ./time.mjs
COPY --chown=node:node public ./public

RUN install -d -m 0750 -o node -g node /data

USER node

ENV PORT=8090 \
    SOCIAL_DATA_DIR=/data \
    SOCIAL_TIMEZONE=Europe/Paris \
    SOCIAL_DEMO_MODE=false \
    SOCIAL_DEMO_SEED=false

EXPOSE 8090

ENTRYPOINT ["node", "/opt/switch-social/server.mjs"]
