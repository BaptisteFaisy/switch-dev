import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const entrypoint = readFileSync(
  new URL("../deploy/docker-entrypoint.sh", import.meta.url),
  "utf8",
);
const documentation = readFileSync(
  new URL("../docs/SSH_TAILSCALE_PC.md", import.meta.url),
  "utf8",
);

test("les terminaux exposent un alias explicite vers le PC fixe", () => {
  assert.match(entrypoint, /Host local pc pc-fixe pc-fixe-tailscale/);
  assert.match(entrypoint, /IdentityFile \$ssh_dir\/id_back/);
  // Le conteneur provisionne sa propre cle d'hote dans $ssh_dir et accepte la
  // cle du PC au premier contact (accept-new) : pas de prompt interactif.
  assert.match(entrypoint, /StrictHostKeyChecking accept-new/);
  assert.match(entrypoint, /ssh_dir=\/srv\/cst\/ssh/);
});

test("la cible du pont reste configurable pour la production", () => {
  assert.match(
    entrypoint,
    /local_host=\$\{CST_SSH_LOCAL_HOST:-host\.docker\.internal\}/,
  );
  assert.match(entrypoint, /local_port=\$\{CST_SSH_LOCAL_PORT:-22\}/);
  assert.match(entrypoint, /local_user=\$\{CST_SSH_LOCAL_USER:-jeanp\}/);
});

test("la documentation explique que Tailscale termine hors du conteneur", () => {
  assert.match(documentation, /ssh pc-fixe hostname/);
  assert.match(documentation, /aucun processus Tailscale ne tourne dans le conteneur/);
  assert.match(documentation, /ne doit jamais etre[\s\S]*affichee/);
});
