import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8").replace(/\r\n/g, "\n");
const web = read("../src/web-update.ts");
const pwa = read("../src/pwa.ts");
const updater = read("../src/updater.ts");
const main = read("../src/main.ts");

const block = (source, start, end) => {
  const from = source.indexOf(start);
  const to = source.indexOf(end, from + start.length);
  assert.ok(from >= 0, `debut introuvable: ${start}`);
  assert.ok(to > from, `fin introuvable: ${end}`);
  return source.slice(from, to);
};

test("la sonde web connaît son intervalle et le marqueur du frontend", () => {
  assert.match(web, /const WEB_UPDATE_POLL_INTERVAL_MS = 5_000;/);
  assert.match(web, /export const frontendBuildIdentity = \(html: string\): string \| null =>/);
  assert.match(web, /cst-build-id/);
  assert.match(web, /__CST_BUILD_ID__\.trim\(\) \|\| null/);
  assert.ok(web.includes("5_000"), "intervalle de poll");
  assert.ok(web.includes("cst-build-id"), "marqueur de build frontend");
});

test("frontendBuildIdentity extrait et nettoie le marqueur meta", () => {
  const identity = block(web, "export const frontendBuildIdentity", "let observedBuild");
  assert.match(identity, /html\.match\(/);
  assert.match(identity, /name\\s\*=\\s\*\["'\]cst-build-id/);
  assert.match(identity, /content\\s\*=\\s\*\["'\]\(\[\^"'\]\+\)/);
  assert.match(identity, /\?\.\[1\]\?\.trim\(\) \?\? ""/);
  assert.match(identity, /return identity \|\| null;/);
});

test("refreshToLatestBuild met à jour le worker puis recharge", () => {
  const refresh = block(web, "const refreshToLatestBuild", "export const checkForWebUpdate");
  assert.match(refresh, /if \(reloading\) return;/);
  assert.match(refresh, /reloading = true;/);
  assert.match(refresh, /navigator\.serviceWorker\?\.getRegistration\(\)/);
  assert.match(refresh, /registration\?\.update\(\)/);
  assert.match(refresh, /caches\.keys\(\)/);
  assert.match(refresh, /cache\.delete\("\/"\)/);
  assert.match(refresh, /cache\.delete\(new Request\(window\.location\.origin \+ "\/"\)\)/);
  assert.match(refresh, /window\.location\.reload\(\);/);
});

test("checkForWebUpdate ne re-vérifie jamais en vol ni pendant un rechargement", () => {
  const check = block(web, "export const checkForWebUpdate", "export const initWebAutoUpdate");
  assert.match(check, /if \(checkInFlight \|\| reloading\) return;/);
  assert.match(check, /checkInFlight = true;/);
  assert.match(check, /fetch\("\/", \{/);
  assert.match(check, /cache: "no-store"/);
  assert.match(check, /Accept: "text\/html"/);
  assert.match(check, /if \(!response\.ok\) return;/);
  assert.match(check, /frontendBuildIdentity\(await response\.text\(\)\)/);
  assert.match(check, /if \(!identity\) return;/);
  assert.match(check, /if \(observedBuild === null\) \{/);
  assert.match(check, /observedBuild = identity;/);
  assert.match(check, /identity !== observedBuild/);
  assert.match(check, /await refreshToLatestBuild\(\);/);
  assert.match(check, /finally \{/);
  assert.match(check, /checkInFlight = false;/);
});

test("initWebAutoUpdate ne tourne que sur http/https et écoute la visibilité", () => {
  const init = web.slice(web.indexOf("export const initWebAutoUpdate"));
  assert.match(init, /\["http:", "https:"\] as string\[\]\)\.includes\(window\.location\.protocol\)/);
  assert.match(init, /void checkForWebUpdate\(\);/);
  assert.match(init, /if \(poll === null\) \{/);
  assert.match(init, /window\.setInterval\(\(\) => void checkForWebUpdate\(\), WEB_UPDATE_POLL_INTERVAL_MS\)/);
  assert.match(init, /visibilitychange/);
  assert.match(init, /document\.visibilityState === "visible"/);
  assert.ok((init.match(/checkForWebUpdate\(\)/g) ?? []).length >= 3, "vérification au lancement + intervalle + visibilité");
});

test("le module web-update est importé et initialisé dans main.ts", () => {
  assert.match(main, /import \{[^}]*initWebAutoUpdate[^}]*\} from "\.\/web-update";/);
  assert.match(main, /initWebAutoUpdate\(\)/);
});

test("la PWA définit ses constantes d'installation et de service worker", () => {
  assert.match(pwa, /const IOS_INSTALL_HINT_DISMISSED = "codex-switch-terminal\.ios-install-hint-dismissed";/);
  assert.match(pwa, /const SERVICE_WORKER_URL = `\/service-worker\.js\?build=\$\{encodeURIComponent\(__CST_BUILD_ID__\)\}`;/);
  assert.ok(pwa.includes("service-worker.js"), "URL du worker");
  assert.ok(pwa.includes("ios-install-hint-dismissed"), "clé localStorage");
});

test("isAppleTouchDevice couvre iPad, iPhone, iPod et Mac tactile", () => {
  const apple = block(pwa, "const isAppleTouchDevice", "const isInstalledWebApp");
  assert.match(apple, /\/iPad\|iPhone\|iPod\/i\.test\(userAgent\)/);
  assert.match(apple, /\/Macintosh\/i\.test\(userAgent\) && navigator\.maxTouchPoints > 1/);
  assert.ok(apple.indexOf("navigator.userAgent") >= 0, "user agent");
});

test("isInstalledWebApp reconnaît standalone et display-mode", () => {
  const installed = block(pwa, "const isInstalledWebApp", "const hasNativeMobileBridge");
  assert.match(installed, /standalone === true/);
  assert.match(installed, /window\.matchMedia\("\(display-mode: standalone\)"\)\.matches/);
});

test("hasNativeMobileBridge détecte les bridges natifs et iOS", () => {
  const bridge = block(pwa, "const hasNativeMobileBridge", "const wasInstallHintDismissed");
  assert.match(bridge, /nativeWindow\.CstIOS \|\| nativeWindow\.CstAndroid/);
  assert.match(bridge, /\/CodexTerminaliOS\/i\.test\(navigator\.userAgent\)/);
  assert.ok(bridge.indexOf("CstIOS") >= 0, "bridge iOS");
  assert.ok(bridge.indexOf("CstAndroid") >= 0, "bridge Android");
});

test("le conseil d'installation iOS est un aside dialog dismissable", () => {
  const hint = block(pwa, "const dismissInstallHint", "const registerServiceWorker");
  assert.match(hint, /hint\.remove\(\);/);
  assert.match(hint, /localStorage\.setItem\(IOS_INSTALL_HINT_DISMISSED, "1"\)/);
  assert.match(hint, /catch \{/);
  const show = block(pwa, "const showIosInstallHint", "const registerServiceWorker");
  assert.match(show, /!isAppleTouchDevice\(\)/);
  assert.match(show, /isInstalledWebApp\(\)/);
  assert.match(show, /hasNativeMobileBridge\(\)/);
  assert.match(show, /wasInstallHintDismissed\(\)/);
  assert.match(show, /document\.querySelector\("\.ios-install-hint"\)/);
  assert.match(show, /document\.createElement\("aside"\)/);
  assert.match(show, /hint\.className = "ios-install-hint";/);
  assert.match(show, /hint\.setAttribute\("role", "dialog"\);/);
  assert.match(show, /Installer Codex Terminal sur cet iPad/);
  assert.match(show, /Sur l'ecran d'accueil/);
  assert.match(show, /hint\.querySelector\("button"\)\?\.addEventListener\("click", \(\) => dismissInstallHint\(hint\)\);/);
  assert.match(show, /document\.body\.appendChild\(hint\);/);
  assert.ok(show.indexOf("&times;") >= 0, "bouton fermer");
});

test("le service worker ne s'enregistre que sur origine web sécurisée", () => {
  const register = block(pwa, "const registerServiceWorker", "const scheduleServiceWorkerRegistration");
  assert.match(register, /!\("serviceWorker" in navigator\)/);
  assert.match(register, /!isWebOrigin/);
  assert.match(register, /!window\.isSecureContext/);
  assert.match(register, /hasNativeMobileBridge\(\)/);
  assert.match(register, /window\.location\.protocol === "https:" \|\| window\.location\.protocol === "http:"/);
  assert.match(register, /navigator\.serviceWorker\.register\(SERVICE_WORKER_URL, \{/);
  assert.match(register, /scope: "\/",/);
  assert.match(register, /updateViaCache: "none",/);
  assert.match(register, /void registration\.update\(\)\.catch\(\(\) => undefined\);/);
  assert.ok(register.indexOf("throttle de 24 h") >= 0, "commentaire throttle");
});

test("l'enregistrement différé utilise requestIdleCallback puis setTimeout", () => {
  const schedule = block(pwa, "const scheduleServiceWorkerRegistration", "export const initPwaSupport");
  assert.match(schedule, /requestIdleCallback\(registerServiceWorker, \{ timeout: 4_000 \}\)/);
  assert.match(schedule, /window\.setTimeout\(registerServiceWorker, 1_000\);/);
  assert.ok(schedule.indexOf("requestIdleCallback") >= 0, "idle callback");
});

test("initPwaSupport montre le conseil puis enregistre au load", () => {
  const init = pwa.slice(pwa.indexOf("export const initPwaSupport"));
  assert.match(init, /showIosInstallHint\(\);/);
  assert.match(init, /document\.readyState === "complete"/);
  assert.match(init, /scheduleServiceWorkerRegistration\(\);/);
  assert.match(init, /window\.addEventListener\("load", scheduleServiceWorkerRegistration, \{ once: true \}\)/);
  assert.ok(init.indexOf("once: true") >= 0, "écouteur one-shot");
});

test("main.ts initialise la PWA", () => {
  assert.match(main, /initPwaSupport\(\)/);
  assert.match(main, /import \{[^}]*initPwaSupport[^}]*\} from "\.\/pwa";/);
});

test("l'updater desktop est un no-op hors Tauri et vérifie la signature", () => {
  assert.match(updater, /import \{ check \} from "@tauri-apps\/plugin-updater";/);
  assert.match(updater, /import \{ relaunch \} from "@tauri-apps\/plugin-process";/);
  assert.match(updater, /export async function initDesktopUpdater\(\): Promise<void>/);
  assert.match(updater, /!\("__TAURI_INTERNALS__" in window\)/);
  assert.match(updater, /await check\(\);/);
  assert.match(updater, /if \(!update\) return;/);
  assert.match(updater, /window\.confirm\(/);
  assert.match(updater, /update\.version/);
  assert.match(updater, /update\.currentVersion/);
  assert.match(updater, /update\.body/);
  assert.match(updater, /await update\.downloadAndInstall\(\);/);
  assert.match(updater, /await relaunch\(\);/);
  assert.match(updater, /console\.error\("\[updater\] échec de la vérification\/installation :", err\);/);
  assert.ok(updater.indexOf("Ed25519") >= 0, "commentaire signature");
  assert.ok(updater.indexOf("downloadAndInstall") >= 0, "installation");
});
