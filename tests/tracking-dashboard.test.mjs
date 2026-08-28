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

test("the tracking dashboard reads every wallet from the protected Duello backend", async () => {
  const view = await source("src/tracking-view.ts");

  assert.match(view, /invoke<DuelloTrackingSnapshot>\("duello_bank_snapshot"/);
  assert.match(view, /hasRemoteAuth\(\)/);
  assert.match(view, /wallets\.map\(\(wallet\)/);
  assert.match(view, /wallet\.clickCount/);
  assert.match(view, /formatMoney\(wallet\.availableMinor, wallet\.currency\)/);
  assert.match(view, /https:\/\/duello\.fr\/l\/\$\{encodeURIComponent\(referralCode\)\}/);
  assert.match(view, /Solde disponible/);
  assert.doesNotMatch(view, /create_tracking_link|5 clics = 1 jour/);
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
