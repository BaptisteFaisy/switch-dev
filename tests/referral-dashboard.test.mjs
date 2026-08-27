import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

async function source(path) {
  return readFile(new URL(path, root), "utf8");
}

test("le dashboard expose l'onglet Parrainage et sa vue", async () => {
  const main = await source("src/main.ts");

  assert.match(main, /data-stats-tab="referral"/);
  assert.match(main, /<span>Parrainage<\/span>/);
  assert.match(main, /renderReferralDashboardPanel/);
  assert.match(main, /renderReferralMetric/);
  assert.match(main, /refreshReferralDashboard = async/);
  assert.match(main, /invoke<ReferralSnapshot>\("referral_snapshot", \{ refresh: force \}\)/);
  assert.match(main, /createReferralCode = async/);
  assert.match(main, /deleteReferralCode = async/);
  assert.match(main, /data-copy-referral=/);
  assert.match(main, /data-delete-referral=/);
});

test("l'onglet Parrainage n'apparaît qu'en mode distant (VPS)", async () => {
  const main = await source("src/main.ts");

  assert.match(main, /\$\{isRemoteMode\(\) \? `\s*\n\s*<button[\s\S]*data-stats-tab="referral"/);
});

test("la plateforme distante mappe les routes de parrainage", async () => {
  const platform = await source("src/platform.ts");

  assert.match(platform, /case "referral_snapshot":[\s\S]*"\/api\/referral/);
  assert.match(platform, /case "create_referral_code":[\s\S]*POST", "\/api\/referral"/);
  assert.match(platform, /case "delete_referral_code":[\s\S]*\/api\/referral\//);
});

test("le serveur expose les routes de parrainage protégées", async () => {
  const server = await source("src-tauri/src/server.rs");

  assert.match(server, /route\("\/referral", get\(api_referral_snapshot\)\.post\(api_create_referral\)\)/);
  assert.match(server, /route\("\/referral\/:code", delete\(api_delete_referral\)\)/);
  assert.match(server, /ReferralManager::load\(/);
  assert.match(server, /CST_DUELLO_REFERRAL_API_URL/);
  assert.match(server, /CST_DUELLO_APP_URL/);
});

test("le gestionnaire crée un code par défaut et persiste", async () => {
  const referral = await source("src-tauri/src/referral.rs");

  assert.match(referral, /pub struct ReferralManager/);
  assert.match(referral, /ensure_initial_code/);
  assert.match(referral, /label: "Code principal"/);
  assert.match(referral, /fs_util::atomic_write/);
  assert.match(referral, /referral_count: Option<u64>/);
  assert.match(referral, /duello_connected/);
  assert.match(referral, /fetch_duello_counts/);
});

test("la vue Parrainage prépare le comptage par code via Duello", async () => {
  const view = await source("src/referral-view.css");
  const main = await source("src/main.ts");

  assert.match(main, /Personnes parrainées/);
  assert.match(main, /En attente de l['’]application Duello/);
  assert.match(main, /referralLinkFor/);
  assert.match(main, /\?ref=\$\{encodeURIComponent\(code\)\}/);
  assert.match(main, /referralCount === null/);
  assert.match(view, /\.referral-table/);
  assert.match(view, /\.referral-modal-backdrop/);
});
