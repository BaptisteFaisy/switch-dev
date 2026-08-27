import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  readProjectVersions,
  releaseTagFromContext,
  validateProjectVersions,
} from "../scripts/verify-versions.mjs";

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

test("les metadonnees de version du projet restent alignees", async () => {
  const versions = await readProjectVersions(projectRoot);
  assert.equal(validateProjectVersions(versions), versions["package.json"]);
});

test("une version ou un tag incoherent bloque la verification", () => {
  assert.throws(
    () => validateProjectVersions({ "package.json": "0.1.0", "Cargo.toml": "0.2.0" }),
    /Versions incoherentes/,
  );
  assert.throws(
    () => validateProjectVersions({ "package.json": "0.1.0" }, "v0.2.0"),
    /ne correspond pas/,
  );
});

test("le tag de release vient de la ligne de commande ou de GitHub Actions", () => {
  assert.equal(releaseTagFromContext({ args: ["--tag", "v1.2.3"], env: {} }), "v1.2.3");
  assert.equal(
    releaseTagFromContext({ args: [], env: { GITHUB_REF_TYPE: "tag", GITHUB_REF_NAME: "v2.0.0" } }),
    "v2.0.0",
  );
});

test("les commandes de production passent par la barriere de verification", async () => {
  const packageJson = JSON.parse(await readFile(resolve(projectRoot, "package.json"), "utf8"));
  // La barriere de verification est precedee par la porte de provenance de la
  // source (prebuild:source-guard) : les deux restent obligatoires.
  assert.match(packageJson.scripts.prebuild, /npm run verify$/);
  assert.match(packageJson.scripts["prebuild:signed"], /npm run verify$/);
  assert.match(packageJson.scripts["prebuild:server"], /npm run verify$/);
  assert.match(packageJson.scripts.verify, /cargo test/);

  const localTauriConfig = JSON.parse(
    await readFile(resolve(projectRoot, "src-tauri", "tauri.local.conf.json"), "utf8"),
  );
  const releaseTauriConfig = JSON.parse(
    await readFile(resolve(projectRoot, "src-tauri", "tauri.conf.json"), "utf8"),
  );
  assert.equal(localTauriConfig.bundle.createUpdaterArtifacts, false);
  assert.equal(releaseTauriConfig.bundle.createUpdaterArtifacts, true);
  assert.match(packageJson.scripts.build, /tauri\.local\.conf\.json/);
  assert.doesNotMatch(packageJson.scripts["build:signed"], /tauri\.local\.conf\.json/);

  const releaseWorkflow = await readFile(
    resolve(projectRoot, ".github", "workflows", "release.yml"),
    "utf8",
  );
  assert.match(releaseWorkflow, /^\s{2}verify-release:\s*$/m);
  assert.match(releaseWorkflow, /^\s{4}needs: verify-release\s*$/m);
});
