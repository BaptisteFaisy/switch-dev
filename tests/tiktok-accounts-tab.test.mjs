import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const main = read("../src/main.ts");
const types = read("../src/main.ts");
const view = read("../src/tiktok-accounts.ts");
const styles = read("../src/tiktok-accounts.css");
const platform = read("../src/platform.ts");
const server = read("../src-tauri/src/server.rs");

test("un onglet TikTok est chargé à la demande sur ordinateur et mobile", () => {
  assert.match(types, /\| "tiktok"/);
  assert.match(main, /type TikTokAccountsModule = typeof import\("\.\/tiktok-accounts"\)/);
  assert.match(main, /tiktokAccountsModulePromise = import\("\.\/tiktok-accounts"\)/);
  assert.match(main, /if \(view === "tiktok" && !tiktokAccountsModule\)/);
  assert.match(main, /id="tiktokToggle"[\s\S]*?<strong>TikTok<\/strong>/);
  assert.match(main, /data-view="tiktok"[^>]*>[\s\S]*?<span>TikTok<\/span>/);
  assert.match(main, /case "tiktok":[\s\S]*?renderTikTokAccountsPanel/);
  assert.match(main, /bindTikTokAccountsUi/);
  assert.match(main, /activateTikTokAccountsPanel\(render, isRemoteMode\(\)\)/);
});

test("le parcours TikTok ouvre la connexion, détecte la session et sélectionne l'émetteur", () => {
  assert.match(view, /id="tiktokOpenScrcpy"/);
  assert.match(view, /runSetupAction\("open_scrcpy"/);
  assert.match(view, /id="tiktokOpenLogin"/);
  assert.match(view, /runSetupAction\("open_login"/);
  assert.match(view, /id="tiktokMatchAccounts"/);
  assert.match(view, /runSetupAction\("match_accounts"/);
  assert.match(view, /data-tiktok-select=/);
  assert.match(view, /select_tiktok_sender_account/);
  assert.match(view, /list_tiktok_sender_accounts/);
  assert.match(view, /manage_tiktok_sender_login/);
  assert.match(platform, /case "manage_tiktok_sender_login"/);
  assert.match(server, /"devices": devices/);
  assert.match(server, /"deviceDetails": device_details/);
  assert.match(server, /"scrcpyAvailable": scrcpy_available/);
});

test("les appareils Android USB, émulateur et réseau sont distingués", () => {
  assert.match(view, /transport: "usb" \| "emulator" \| "network" \| "unknown"/);
  assert.match(view, /Autorisation USB requise/);
  assert.match(view, /Téléphone USB/);
  assert.match(view, /ADB réseau/);
  assert.match(view, /snapshot\?\.adbError/);
  assert.match(server, /"bridgeOnline": bridge_online/);
});

test("le client Cloud recupere l'URL et le token depuis le noeud de demarrage", () => {
  assert.match(platform, /const startupPrimaryNode = startupNodes/);
  assert.match(platform, /config\.baseUrl\?\.trim\(\) \|\| startupPrimaryNode\?\.baseUrl/);
  assert.match(platform, /config\.token\?\.trim\(\) \|\| startupPrimaryNode\?\.token/);
});

test("les secrets TikTok restent hors du formulaire et du VPS", () => {
  assert.doesNotMatch(view, /<input[^>]+type=["']password["']/i);
  assert.doesNotMatch(view, /name=["'](?:password|token|cookie|captcha|twoFactor)["']/i);
  assert.match(view, /Aucun mot de passe, captcha, code 2FA ou token n’est envoyé au VPS/);
  assert.match(view, /Identifiants et vérifications locales/);
});

test("la vue TikTok est responsive et utilise les icônes enregistrées", () => {
  assert.match(styles, /\.tiktok-accounts-panel\s*\{/);
  assert.match(styles, /\.tiktok-setup-grid\s*\{/);
  assert.match(styles, /\.tiktok-account-card\s*\{/);
  assert.match(styles, /@media \(max-width: 640px\)/);

  const registryStart = main.indexOf("const lucideIcons = {");
  const registryEnd = main.indexOf("};", registryStart);
  const registry = main.slice(registryStart, registryEnd);
  const iconNames = [...view.matchAll(/data-lucide="([a-z0-9-]+)"/g)]
    .map((match) => match[1]);
  for (const iconName of iconNames) {
    const component = iconName
      .split("-")
      .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
      .join("");
    assert.match(registry, new RegExp(`\\b${component}\\b`), `${iconName} n'est pas enregistré`);
  }
});
