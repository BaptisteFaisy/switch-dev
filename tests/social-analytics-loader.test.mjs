import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

test("le dashboard social reste masqué quand le sidecar développement est absent", async () => {
  const loader = await readFile(path.join(root, "public", "social-loader.js"), "utf8");
  const index = await readFile(path.join(root, "index.html"), "utf8");
  assert.match(index, /<script defer src="\/social-loader\.js"><\/script>/);
  assert.match(loader, /fetch\("\/api\/social\/availability"/);
  assert.match(loader, /payload\?\.available === true/);
  assert.match(loader, /if \(!available\) return/);
});

test("le dashboard social est isolé dans un dialogue et un iframe borné", async () => {
  const loader = await readFile(path.join(root, "public", "social-loader.js"), "utf8");
  assert.match(loader, /document\.createElement\("dialog"\)/);
  assert.match(loader, /frame\.src = initialFrameUrl/);
  assert.match(loader, /allow-top-navigation-by-user-activation/);
  assert.match(loader, /event\.origin === window\.location\.origin/);
  assert.match(loader, /MutationObserver\(scheduleInjection\)/);
});
