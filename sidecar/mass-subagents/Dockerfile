FROM node:22-bookworm-slim

ENV NODE_ENV=production \
    SWITCH_ENV=development \
    MASS_SUBAGENTS_BIND=0.0.0.0 \
    MASS_SUBAGENTS_DATA_DIR=/srv/switch-mass-subagents
WORKDIR /opt/switch-mass-subagents

RUN groupadd --gid 10002 mass-subagents \
    && useradd --uid 10002 --gid mass-subagents --create-home --shell /usr/sbin/nologin mass-subagents \
    && install -d -m 0700 -o mass-subagents -g mass-subagents /srv/switch-mass-subagents

COPY --chown=mass-subagents:mass-subagents package.json ./
COPY --chown=mass-subagents:mass-subagents src/ ./src/
COPY --chown=mass-subagents:mass-subagents agent_roles/ ./agent_roles/
COPY --chown=mass-subagents:mass-subagents contracts/ ./contracts/

USER mass-subagents
EXPOSE 18084

HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=6 \
  CMD node -e "fetch('http://127.0.0.1:18084/healthz').then(r=>{if(!r.ok)process.exit(1)}).catch(()=>process.exit(1))"

CMD ["node", "src/cli.mjs", "serve"]
