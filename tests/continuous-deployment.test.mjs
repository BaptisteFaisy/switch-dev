import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

test("le push main deploie des artefacts precompiles et signes puis les clients detectent le commit", async () => {
  const workflow = await read(".github/workflows/deploy-web.yml");
  const updater = await read("deploy/update-node.sh");
  const webUpdate = await read("src/web-update.ts");

  assert.match(workflow, /branches: \[main\]/);
  assert.match(workflow, /update-node\.sh/);
  assert.match(workflow, /--commit "\$COMMIT_SHA"/);
  // La CI compile et signe ; le noeud ne compile plus sur l'hote.
  assert.match(workflow, /cargo build --manifest-path src-tauri\/Cargo\.toml --release --bin cst-server/);
  assert.match(workflow, /cst-server-linux-x86_64\.tar\.gz/);
  assert.match(workflow, /minisign -S -s minisign\.key/);
  // Frontend seul : pas de compilation, seul dist/ est transfere et bascule.
  assert.match(workflow, /git diff --quiet "\$BEFORE" "\$\{\{ github\.sha \}\}" -- src-tauri\//);
  assert.match(workflow, /tar -czf cst-frontend\.tar\.gz dist/);
  assert.match(workflow, /--prebuilt \/tmp\/cst-server-linux-x86_64\.tar\.gz/);
  assert.match(workflow, /--frontend \/tmp\/cst-frontend\.tar\.gz/);
  // Le noeud doit pouvoir basculer sans cle minisign (mode non signe explicite).
  assert.match(workflow, /--allow-unsigned/);

  assert.match(updater, /RELEASE_ID="\$VERSION-\$SAFE_COMMIT"/);
  assert.match(updater, /verify "\$VERSION" "\$COMMIT"/);
  // Modes precompile et frontend : verification signee puis bascule atomique.
  assert.match(updater, /--prebuilt\) MODE="prebuilt"/);
  assert.match(updater, /--frontend\) MODE="frontend"/);
  assert.match(updater, /verify_artifact/);
  assert.match(updater, /minisign -Vm "\$asset" -x "\$asset\.minisig" -P "\$MINISIGN_PUBKEY"/);
  assert.match(updater, /Mode frontend : binaire conserve \(\$BUILT_BIN\), seul dist\/ est remplace/);

  assert.match(webUpdate, /fetch\("\/"/);
  assert.match(webUpdate, /cst-build-id/);
  assert.match(webUpdate, /window\.location\.reload\(\)/);
});
