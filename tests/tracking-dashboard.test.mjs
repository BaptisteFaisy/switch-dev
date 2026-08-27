import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

async function source(path) {
  return readFile(new URL(path, root), "utf8");
}

test("the Duello shell exposes the tracking view", async () => {
  const main = await source("src/main.ts");

  assert.match(main, /type AppView =[\s\S]*\| "tracking"/);
  assert.match(main, /data-view="tracking"/);
  assert.match(main, /id="trackingToggle"/);
  assert.match(main, /renderTrackingPanel/);
});

test("the remote platform maps tracking management endpoints", async () => {
  const platform = await source("src/platform.ts");

  assert.match(platform, /case "tracking_links"[\s\S]*GET", "\/api\/tracking-links"/);
  assert.match(platform, /case "create_tracking_link"[\s\S]*POST", "\/api\/tracking-links"/);
});

test("the tracking dashboard applies the five-click day rule", async () => {
  const view = await source("src/tracking-view.ts");

  assert.match(view, /Math\.floor\(link\.clickCount \/ clicksPerDay\)/);
  assert.match(view, /\$\{clicksPerDay\} clics = 1 jour/);
  assert.match(view, /\/t\/\$\{encodeURIComponent\(slug\)\}/);
});

test("the server exposes protected management and public redirect routes", async () => {
  const server = await source("src-tauri/src/server.rs");
  const tracking = await source("src-tauri/src/tracking.rs");

  assert.match(server, /route\([\s\S]*"\/tracking-links"/);
  assert.match(server, /route\("\/t\/:slug"/);
  assert.match(tracking, /const CLICKS_PER_DAY: u64 = 5/);
  assert.match(tracking, /click_count = store\.links\[index\]\.click_count\.saturating_add\(1\)/);
  assert.match(tracking, /fs_util::atomic_write/);
});
