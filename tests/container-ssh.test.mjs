import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const dockerfile = readFileSync(new URL("../Dockerfile", import.meta.url), "utf8");
const entrypoint = readFileSync(
  new URL("../deploy/docker-entrypoint.sh", import.meta.url),
  "utf8",
);
const compose = readFileSync(new URL("../compose.yaml", import.meta.url), "utf8");
const setup = readFileSync(
  new URL("../scripts/setup-container-ssh.ps1", import.meta.url),
  "utf8",
);
const launcher = readFileSync(
  new URL("../scripts/start-container.ps1", import.meta.url),
  "utf8",
);
const sshdHelper = readFileSync(
  new URL("../scripts/ensure-windows-sshd.ps1", import.meta.url),
  "utf8",
);

test("le conteneur embarque un serveur SSH a cle publique uniquement", () => {
  assert.match(dockerfile, /openssh-server/);
  assert.match(entrypoint, /\/usr\/sbin\/sshd/);
  assert.match(entrypoint, /PasswordAuthentication no/);
  assert.match(entrypoint, /HostKey \$ssh_dir\/ssh_host_ed25519_key/);
  assert.match(entrypoint, /AuthorizedKeysFile \$ssh_dir\/authorized_keys/);
  assert.match(entrypoint, /AllowUsers cst/);
});

test("les cles et authorized_keys du conteneur survivent aux rebuilds", () => {
  assert.match(entrypoint, /ssh_dir=\/srv\/cst\/ssh/);
  assert.match(entrypoint, /ssh-keygen -q -t ed25519 -N "" -f "\$ssh_dir\/ssh_host_ed25519_key"/);
});

test("compose publie le port SSH du conteneur uniquement sur le loopback", () => {
  assert.match(compose, /127\.0\.0\.1:\$\{CST_SSH_PORT:-2224\}:22/);
});

test("les cles SSH sont bind-montees depuis .cst-data/ssh, pas dans le volume", () => {
  assert.match(compose, /\$\{CST_SSH_DIR:-\.\/\.cst-data\/ssh\}:\/srv\/cst\/ssh/);
});

test("le conteneur peut joindre le poste hote en SSH (sens sortant)", () => {
  assert.match(entrypoint, /Host local pc pc-fixe pc-fixe-tailscale/);
  assert.match(
    entrypoint,
    /local_host=\$\{CST_SSH_LOCAL_HOST:-host\.docker\.internal\}/,
  );
  assert.match(entrypoint, /HostName \$local_host/);
  assert.match(entrypoint, /IdentityFile \$ssh_dir\/id_back/);
  assert.match(compose, /host\.docker\.internal:host-gateway/);
});

test("le pont conteneur -> poste est persistant et se retablit seul", () => {
  assert.match(dockerfile, /autossh/);
  assert.match(entrypoint, /autossh -M 0 -N/);
  assert.match(entrypoint, /ControlMaster auto/);
  assert.match(entrypoint, /ControlPersist 60/);
  assert.match(entrypoint, /ServerAliveInterval 30/);
});

test("le script de setup prepare les deux cles et les deux authorized_keys", () => {
  assert.match(setup, /ssh-keygen/);
  assert.match(setup, /authorized_keys/);
  assert.match(setup, /id_back/);
  assert.match(setup, /cst@127\.0\.0\.1/);
  assert.match(setup, /ssh local/);
});

test("le launcher enchaine cles + sshd + compose up sans etape manuelle", () => {
  assert.match(launcher, /setup-container-ssh\.ps1/);
  assert.match(launcher, /CST_SSH_LOCAL_USER = \$WindowsUser/);
  assert.match(launcher, /docker compose up -d/);
  assert.match(launcher, /ensure-windows-sshd\.ps1/);
  assert.match(launcher, /-Verb RunAs/);
});

test("sshd Windows s'ouvre automatiquement et reste actif au demarrage", () => {
  assert.match(sshdHelper, /Set-Service sshd -StartupType Automatic/);
  assert.match(sshdHelper, /Start-Service sshd/);
});
