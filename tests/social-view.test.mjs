import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

test("le dashboard social vit dans un onglet natif, plus dans un loader injecté", async () => {
  const index = await readFile(path.join(root, "index.html"), "utf8");
  const main = await readFile(path.join(root, "src", "main.ts"), "utf8");

  assert.doesNotMatch(index, /social-loader\.js/);
  assert.match(main, /type AppView =[^;]*\| "social"/s);
  assert.match(main, /import\("\.\/social-view"\)/);
  assert.match(main, /import\("\.\/social-view\.css"\)/);
  assert.match(main, /data-view="social"/);
  assert.match(main, /id="socialToggle"/);
  assert.match(main, /renderSocialPanel/);
  assert.match(main, /activateSocialPanel/);
  assert.match(main, /deactivateSocialPanel/);
});

test("l'onglet embarque le dashboard /social/ dans une iframe bornée", async () => {
  const view = await readFile(path.join(root, "src", "social-view.ts"), "utf8");

  assert.match(view, /frame\.src = frameUrl/);
  assert.match(view, /let frameUrl = "\/social\/"/);
  assert.match(view, /allow-top-navigation-by-user-activation/);
  assert.match(view, /fetch\("\/api\/social\/availability"/);
  assert.match(view, /payload\?\.available === true/);
  assert.match(view, /MutationObserver\(\(\) => adoptFrame\(\)\)/);
});

test("le retour OAuth ouvre l'onglet et prépare le toast du dashboard", async () => {
  const main = await readFile(path.join(root, "src", "main.ts"), "utf8");
  const view = await readFile(path.join(root, "src", "social-view.ts"), "utf8");

  assert.match(main, /switch_social/);
  assert.match(main, /setActiveView\("social"\)/);
  assert.match(view, /setSocialCallbackResult/);
  assert.match(view, /connect_error/);
  assert.match(view, /connected/);
});
