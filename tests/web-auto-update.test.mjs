import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

test("le client web detecte un nouveau build et recharge sans cache", async () => {
  const source = await read("src/web-update.ts");
  const main = await read("src/main.ts");

  assert.match(source, /fetch\("\/"/);
  assert.match(source, /__CST_BUILD_ID__/);
  assert.match(source, /cache: "no-store"/);
  assert.match(source, /serviceWorker\?\.getRegistration\(\)/);
  assert.match(source, /window\.location\.reload\(\)/);
  assert.match(source, /WEB_UPDATE_POLL_INTERVAL_MS = 5_000/);
  assert.match(main, /initWebAutoUpdate\(\)/);
});

test("le client compare le frontend charge avec l'index servi, jamais avec le backend", async () => {
  const source = await read("src/web-update.ts");
  const vite = await read("vite.config.ts");

  assert.match(source, /let observedBuild: string \| null = __CST_BUILD_ID__\.trim\(\) \|\| null/);
  assert.match(source, /frontendBuildIdentity\(await response\.text\(\)\)/);
  assert.match(source, /identity !== observedBuild/);
  assert.doesNotMatch(source, /fetch\("\/healthz"/);
  assert.match(vite, /name: "cst-build-id", content: buildId/);
});

test("le commit embarque vient de CST_GIT_COMMIT et est declare pour tsc", async () => {
  const vite = await read("vite.config.ts");
  const env = await read("src/vite-env.d.ts");

  assert.match(vite, /process\.env\.CST_GIT_COMMIT/);
  assert.match(vite, /__CST_BUILD_COMMIT__: JSON\.stringify\(buildCommit\)/);
  assert.match(vite, /git rev-parse --short HEAD/);
  assert.match(env, /declare const __CST_BUILD_COMMIT__: string;/);
});
