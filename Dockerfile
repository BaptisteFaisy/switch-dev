# syntax=docker/dockerfile:1.7

FROM node:22-bookworm-slim AS frontend-build
WORKDIR /build

ARG TARGETARCH
COPY package.json package-lock.json ./
RUN --mount=type=cache,id=cst-frontend-npm-${TARGETARCH},target=/root/.npm,sharing=locked \
    npm ci

COPY index.html tsconfig.json vite.config.ts ./
COPY scripts ./scripts
COPY src ./src
COPY public ./public
# Le prebuild:frontend lance un garde-fou de source PowerShell (source-guard,
# WIP hote Windows) qui n'existe pas dans le conteneur Linux. On execute ici
# directement les etapes du build frontend ; le controle de source reste cote
# hote (npm run prebuild / build:frontend sur Windows).
RUN npm run clean:web && npx tsc && npx vite build && node scripts/precompress-frontend.mjs

FROM rust:1.88.0-bookworm AS server-build
WORKDIR /build

# Les plateformes Buildx compilent en parallele : chaque architecture doit
# disposer de ses propres caches pour eviter les courses d'extraction Cargo.
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends git libssl-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY src-tauri ./src-tauri
ARG CST_GIT_COMMIT=container
ENV CST_GIT_COMMIT=${CST_GIT_COMMIT}
RUN --mount=type=cache,id=cst-cargo-registry-${TARGETARCH},target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cst-cargo-git-${TARGETARCH},target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=cst-cargo-target-${TARGETARCH},target=/build/src-tauri/target,sharing=locked \
    cargo build \
      --locked \
      --manifest-path src-tauri/Cargo.toml \
      --profile server \
      --bin cst-server \
    && install -m 0755 src-tauri/target/server/cst-server /tmp/cst-server

FROM node:22-bookworm-slim AS runtime

ARG TARGETARCH
ARG RUST_VERSION=1.88.0

# DEBIAN_FRONTEND evite les questions interactives d'apt dans les logs de build.
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      autossh \
      bash \
      build-essential \
      ca-certificates \
      chromium \
      curl \
      file \
      ffmpeg \
      gzip \
      git \
      gosu \
      jq \
      libssl-dev \
      openssh-client \
      openssh-server \
      pkg-config \
      procps \
      python3 \
      ripgrep \
      sudo \
      tar \
      unzip \
      xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd --gid 10001 cst \
    && useradd --uid 10001 --gid cst --create-home --home-dir /home/cst --shell /bin/bash cst \
    # useradd cree un compte au mot de passe verrouille (":!"), et OpenSSH 9
    # refuse alors TOUTE authentification (clé publique comprise) : « User cst
    # not allowed because account is locked ». On debloque le compte avec un
    # hash SHA-512 aleatoire jetable — jamais utilisable (PasswordAuthentication
    # no), il sert uniquement a satisfaire le controle de statut d'OpenSSH.
    && usermod -p "$(python3 -c 'import crypt, os; print(crypt.crypt(os.urandom(16).hex(), crypt.mksalt(crypt.METHOD_SHA512)))')" cst \
    && install -d -o cst -g cst /srv/cst /srv/cst/workspaces /opt/codex-switch-terminal/dist

USER cst
ENV HOME=/home/cst \
    PATH=/home/cst/.local/bin:/home/cst/.cargo/bin:/usr/local/bin:/usr/bin:/bin
# Toolchain Rust du runtime : uniquement pour les agents qui compilent du Rust
# dans le conteneur (~550 Mo et plusieurs minutes de build). CST_RUNTIME_RUST=0
# la saute pour un rebuild rapide quand on n'en a pas besoin ; defaut = 1,
# comportement historique inchange.
ARG CST_RUNTIME_RUST=1
RUN if [ "$CST_RUNTIME_RUST" = "1" ]; then \
      curl --proto '=https' --tlsv1.2 -fsS https://sh.rustup.rs -o /tmp/rustup-init.sh \
      && sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain "${RUST_VERSION}" \
      && rm -f /tmp/rustup-init.sh; \
    fi
# OpenCode porte tous les fournisseurs API annexes (Z.ai, MiniMax, DeepSeek,
# OpenRouter). Sans lui, `opencode auth login --provider <id>` echouait en
# « command not found » et le terminal de connexion restait ouvert sans fin.
RUN --mount=type=cache,id=cst-runtime-npm-${TARGETARCH},target=/home/cst/.npm,uid=10001,gid=10001,sharing=locked \
    # @anthropic-ai/claude-code est EPINGLE a 2.1.207 : la derniere version
    # publiee sur npm (août 2026) ne fournit plus son binaire natif dans
    # node:22-bookworm-slim (« Error: claude native binary not installed »
    # au premier claude --version). 2.1.207 est la version installee et
    # verifiee sur les postes de dev ; reepuiser la version apres audit.
    # tokscale est installe dans la meme couche : une seule resolution npm, une
    # seule couche image (usage + leaderboard tokscale.ai ;
    # scripts/tokscale-all.sh utilise le binaire local plutot que `npx`, faible
    # RAM). Le token/credentials vivent dans TOKSCALE_CONFIG_DIR=/srv/cst/.tokscale
    # (voir deploy/cst-container.env).
    npm install --global --prefix /home/cst/.local @openai/codex@0.150.1 @anthropic-ai/claude-code@2.1.207 opencode-ai freebuff tokscale \
    && command -v codex >/dev/null \
    && codex --version \
    && command -v claude >/dev/null \
    && claude --version \
    && command -v opencode >/dev/null \
    && opencode --version \
    && command -v freebuff >/dev/null \
    && command -v tokscale >/dev/null \
    && tokscale --version

# `opencode auth login` bootstrape son environnement AVANT d'afficher son invite :
# telechargement du catalogue models.dev (3,2 Mo) puis installation de
# `@opencode-ai/plugin` (~60 Mo de node_modules) dans son dossier de config.
# Mesure sur le VPS : ~8 minutes de terminal totalement muet, repayees par chaque
# nouveau compte tant que ces dossiers vivaient sous le home du compte. Le runtime
# est desormais mutualise (`provider::opencode_shared_runtime_dir`) et pre-chauffe
# ici une fois pour toutes. Aucun identifiant n'y transite : ils restent dans le
# `XDG_DATA_HOME` du compte, ici detourne vers un home jetable.
#
# Les deux etapes sont reproduites explicitement plutot que lancees via `auth
# login` : ce dernier attend une cle sur un TTY et n'a donc aucune fin
# exploitable dans un build (son invite s'affiche AVANT la fin de l'installation
# du plugin, la couper laisserait l'image a moitie chaude). `opencode models`
# ecrit le catalogue puis sort ; le plugin est installe a la version exacte de la
# CLI, dans le format que OpenCode ecrit lui-meme, et il le reutilise tel quel.
ENV CST_OPENCODE_RUNTIME_DIR=/home/cst/.cst-opencode-runtime
RUN --mount=type=cache,id=cst-runtime-npm-${TARGETARCH},target=/home/cst/.npm,uid=10001,gid=10001,sharing=locked \
    set -eu; \
    export XDG_CACHE_HOME="${CST_OPENCODE_RUNTIME_DIR}/cache" \
           XDG_CONFIG_HOME="${CST_OPENCODE_RUNTIME_DIR}/config" \
           OPENCODE_CONFIG_DIR="${CST_OPENCODE_RUNTIME_DIR}/config/opencode" \
           XDG_DATA_HOME=/tmp/opencode-warmup/data \
           XDG_STATE_HOME=/tmp/opencode-warmup/state; \
    mkdir -p "$XDG_CACHE_HOME" "$OPENCODE_CONFIG_DIR" "$XDG_DATA_HOME" "$XDG_STATE_HOME"; \
    opencode models >/dev/null; \
    printf '{\n  "dependencies": {\n    "@opencode-ai/plugin": "%s"\n  }\n}\n' \
      "$(opencode --version | tr -d '\r\n')" > "${OPENCODE_CONFIG_DIR}/package.json"; \
    npm install --prefix "${OPENCODE_CONFIG_DIR}" --no-audit --no-fund; \
    test -s "${CST_OPENCODE_RUNTIME_DIR}/cache/opencode/models.json"; \
    test -d "${CST_OPENCODE_RUNTIME_DIR}/config/opencode/node_modules/@opencode-ai/plugin"; \
    rm -rf /tmp/opencode-warmup

# Le paquet npm `freebuff` n'est qu'un lanceur : le vrai CLI est un binaire Bun
# de ~140 Mo qu'il telecharge dans `~/.config/manicode` au premier demarrage
# (50 Mo compresses). Sans ce pre-amorcage, le premier onglet freebuff resterait
# muet le temps du telechargement — exactement le symptome traite juste au-dessus
# pour OpenCode, et que l'utilisateur lit comme un terminal casse.
#
# `--version` est la seule entree qui declenche la recuperation du binaire puis
# rend la main : le TUI, lui, ne se termine jamais de lui-meme et ne peut donc
# pas servir de pre-chauffage dans un build. Aucun identifiant n'y transite, la
# connexion se fait par `freebuff login` dans le home du compte.
RUN set -eu; \
    freebuff --version >/dev/null; \
    test -s /home/cst/.config/manicode/freebuff

USER root
# Le label d'image porte le commit git (build arg). Declare APRES les couches
# lourdes (apt, rustup, npm, pre-chauffages) : un nouveau CST_GIT_COMMIT (CI,
# deploiement) ne rejoue alors que les COPY/chmod finaux, au lieu d'invalider
# tout le stage runtime.
ARG CST_GIT_COMMIT=container
LABEL org.opencontainers.image.title="Codex Switch Terminal" \
      org.opencontainers.image.description="Runtime distant de chats Codex Switch Terminal" \
      org.opencontainers.image.revision="${CST_GIT_COMMIT}"

COPY --from=server-build /tmp/cst-server /usr/local/bin/cst-server
COPY --from=frontend-build /build/dist /opt/codex-switch-terminal/dist
COPY deploy/docker-entrypoint.sh /usr/local/bin/cst-container-entrypoint
COPY deploy/cst-memory-watchdog.sh /usr/local/bin/cst-memory-watchdog
COPY deploy/codex-rollout-guard.sh /usr/local/bin/codex-rollout-guard
COPY deploy/codex-rollout-ordinal-repair.py /usr/local/bin/codex-rollout-ordinal-repair
RUN mv /home/cst/.local/bin/codex /home/cst/.local/bin/codex-real \
    && ln -s /usr/local/bin/codex-rollout-guard /home/cst/.local/bin/codex \
    && chmod 0755 /usr/local/bin/codex-rollout-guard /usr/local/bin/codex-rollout-ordinal-repair \
      /usr/local/bin/cst-container-entrypoint /usr/local/bin/cst-memory-watchdog /usr/local/bin/cst-server \
    && chown -R cst:cst /opt/codex-switch-terminal

ENV CST_BIND=0.0.0.0:8080 \
    CST_DATA_DIR=/srv/cst \
    CST_WORKSPACES_ROOT=/srv/cst/workspaces \
    CST_STATIC_DIR=/opt/codex-switch-terminal/dist

EXPOSE 8080
VOLUME ["/srv/cst"]
HEALTHCHECK --interval=15s --timeout=4s --start-period=20s --retries=8 \
  CMD curl -fsS http://127.0.0.1:8080/healthz >/dev/null || exit 1

ENTRYPOINT ["/usr/local/bin/cst-container-entrypoint"]
CMD ["cst-server"]
