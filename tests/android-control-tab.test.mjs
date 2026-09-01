import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const main = read("../src/main.ts");
const view = read("../src/android-control.ts");
const styles = read("../src/android-control.css");
const tiktok = read("../src/tiktok-accounts.ts");
const backend = read("../src-tauri/src/tiktok_messaging.rs");
const server = read("../src-tauri/src/server.rs");

test("Android possède son propre onglet lazy sur ordinateur et mobile", () => {
  assert.match(main, /\| "android"/);
  assert.match(main, /type AndroidControlModule = typeof import\("\.\/android-control"\)/);
  assert.match(main, /androidControlModulePromise = import\("\.\/android-control"\)/);
  assert.match(main, /if \(view === "android" && !androidControlModule\)/);
  assert.match(main, /id="androidToggle"[\s\S]*?<strong>Android<\/strong>/);
  assert.match(main, /data-view="android"[^>]*>[\s\S]*?<span>Android<\/span>/);
  assert.match(main, /case "android":[\s\S]*?renderAndroidControlPanel/);
  assert.match(main, /bindAndroidControlUi/);
  assert.match(main, /activateAndroidControlPanel\(render, isRemoteMode\(\)\)/);
  assert.match(main, /deactivateAndroidControlPanel\(\)/);
  assert.match(view, /id="androidControlPanel"/);
  assert.doesNotMatch(tiktok, /androidControlEmbedFrame|renderAndroidEmbed|ws-scrcpy-web/);
});

test("le module Android expose une API autonome pour le nouvel onglet", () => {
  assert.match(view, /import \{ invoke \} from "\.\/platform"/);
  assert.match(view, /import "\.\/android-control\.css"/);
  assert.match(view, /export const renderAndroidControlPanel/);
  assert.match(view, /export const bindAndroidControlUi/);
  assert.match(view, /export const activateAndroidControlPanel/);
  assert.match(view, /export const deactivateAndroidControlPanel/);
  assert.doesNotMatch(view, /from "\.\/tiktok-accounts"/);
});

test("les appareils ADB viennent du pont existant et scrcpy externe reste disponible en secours", () => {
  assert.match(view, /invoke<AndroidDevicesSnapshot>\("list_tiktok_sender_accounts"\)/);
  assert.match(view, /invoke<ScrcpySetupAction>\("manage_tiktok_sender_login", \{/);
  assert.match(view, /action: "open_scrcpy"/);
  assert.match(view, /deviceSerial/);
  assert.match(view, /id="androidControlDevice"/);
  assert.match(view, /Autorisation USB requise/);
  assert.match(view, /Téléphone USB/);
  assert.match(view, /ADB réseau/);
  assert.match(view, /id="androidControlOpenScrcpy"/);
  assert.match(view, /Ouvrir scrcpy externe · secours/);
});

test("ws-scrcpy-web possède un bouton de démarrage et un état distinct de l'iframe", () => {
  assert.match(view, /type WsScrcpyServiceState = "idle" \| "starting" \| "online" \| "error"/);
  assert.match(view, /type AndroidEmbedState = "closed" \| "loading" \| "ready" \| "error"/);
  assert.match(view, /wsScrcpyAvailable: boolean/);
  assert.match(view, /wsScrcpyOnline: boolean/);
  assert.match(view, /data-service-state="\$\{serviceState\}"/);
  assert.match(view, /id="androidControlStartWebScrcpy"/);
  assert.match(view, /Démarrer ws-scrcpy-web/);
  assert.match(view, /Démarrage…/);
  assert.match(view, /Service en ligne/);
  assert.match(view, /Réessayer le démarrage/);
  assert.match(view, /aria-busy="\$\{state === "starting" \? "true" : "false"\}"/);
  assert.match(styles, /\.android-control-service-button\.is-starting/);
  assert.match(styles, /\.android-control-service-button\.is-online/);
  assert.match(styles, /\.android-control-service-button\.is-error/);
});

test("le démarrage est mis en file puis confirmé par le heartbeat réel", () => {
  assert.match(view, /if \(wsScrcpyServiceState === "starting"\) return/);
  assert.match(view, /action: "start_ws_scrcpy"/);
  assert.match(view, /WS_SCRCPY_START_TIMEOUT_MS = 150_000/);
  assert.match(view, /scheduleWsScrcpyStartTimeout\(options\)/);
  assert.match(view, /if \(next\.bridgeOnline && next\.wsScrcpyOnline\)/);
  assert.match(view, /wsScrcpyServiceState = "online"/);
  assert.match(view, /queued\.status === "failed"/);
  assert.match(view, /Commande envoyée au PC Windows\. Détection de ws-scrcpy-web en cours/);
  assert.match(view, /role="alert"/);
  assert.match(view, /clearWsScrcpyStartTimer\(\)/);
  assert.doesNotMatch(view, /queued\.status === "queued"[\s\S]{0,120}wsScrcpyServiceState = "online"/);
});

test("l'affichage intégré attend le service et un appareil mais le démarrage n'attend pas le téléphone", () => {
  assert.match(view, /const canOpenSelected = serviceState === "online"[\s\S]*?!!selectedSerial/);
  assert.match(view, /Démarrez d’abord ws-scrcpy-web et attendez que le service soit détecté/);
  assert.match(view, /action: "start_ws_scrcpy",\s*\}\)/);
  assert.doesNotMatch(view, /action: "start_ws_scrcpy",\s*deviceSerial/);
});

test("le connecteur Windows lance et confirme réellement ws-scrcpy-web", () => {
  assert.match(backend, /StartWsScrcpy/);
  assert.match(backend, /ws_scrcpy_available/);
  assert.match(backend, /ws_scrcpy_online/);
  assert.match(backend, /CST_WS_SCRCPY_PATH/);
  assert.match(backend, /WS_SCRCPY_WEB_PORT", "8000"/);
  assert.match(backend, /WS_SCRCPY_NO_BROWSER", "1"/);
  assert.match(backend, /CREATE_NO_WINDOW|0x08000000/);
  assert.match(backend, /body\.contains\("ws-scrcpy-web stream"\)/);
  assert.match(backend, /body\.contains\("data-embed-entry"\)/);
  assert.match(server, /"wsScrcpyAvailable": ws_scrcpy_available/);
  assert.match(server, /"wsScrcpyOnline": ws_scrcpy_online/);
  assert.match(server, /let ws_scrcpy_online = bridge_online/);
});

test("le connecteur provisionne ws-scrcpy-web automatiquement quand il manque", () => {
  // Le zip portable officiel, piné, est téléchargé et vérifié par SHA-256.
  assert.match(backend, /WsScrcpyWeb-beta-Portable\.zip/);
  assert.match(backend, /SHA256SUMS/);
  assert.match(backend, /bilbospocketses\/ws-scrcpy-web\/releases\/download/);
  assert.match(backend, /v0\.1\.30-beta\.82/);
  // Le correctif de port 8000 est appliqué à l'entrée du bundle.
  assert.match(backend, /i\.listen\(8000,\(\)=>\{Qr\.printListeningMsg\(n,8000,ts\)\}\)/);
  // L'extraction passe par PowerShell et l'installation est gérée par l'app.
  assert.match(backend, /Expand-Archive/);
  assert.match(backend, /runtime_data_path\("ws-scrcpy-web"\)/);
  assert.match(backend, /managed_ws_scrcpy_is_valid/);
  // Le job de premier lancement est traité en arrière-plan pour ne pas couper
  // les heartbeats pendant le téléchargement (~80 Mo).
  assert.match(backend, /tokio::task::spawn\(async move \{\s*let report = process_tiktok_sender_setup/);
  // Plus de refus serveur quand ws-scrcpy-web n'est pas encore installé :
  // l'action est mise en file et le connecteur provisionne au besoin.
  assert.doesNotMatch(backend, /StartWsScrcpy[\s\S]{0,200}introuvable sur le poste Windows/);
  // Le heartbeat considère l'installation gérée comme disponible.
  assert.match(backend, /managed_ws_scrcpy_root\(\)/);
  assert.match(backend, /ws_scrcpy_available = resolve_ws_scrcpy_path\(\)\.is_some\(\)/);
  // L'interface ne bloque plus sur « non installé » : elle prévient puis laisse
  // le connecteur installer automatiquement.
  assert.match(view, /téléchargement et installation automatiques au premier démarrage/);
  assert.match(view, /il sera téléchargé et installé automatiquement au premier démarrage du service/);
  assert.doesNotMatch(view, /ws-scrcpy-web est introuvable sur le PC Windows/);
});

test("l'iframe n'accepte que l'URL ws-scrcpy-web locale attendue", () => {
  assert.match(view, /const SCRCPY_EMBED_ORIGIN = "http:\/\/127\.0\.0\.1:8000"/);
  assert.match(view, /const SCRCPY_EMBED_PATH = "\/embed\.html"/);
  assert.match(view, /const SCRCPY_EMBED_URL_PREFIX = "http:\/\/127\.0\.0\.1:8000\/embed\.html\?device="/);
  assert.match(view, /new URL\(SCRCPY_EMBED_PATH, `\$\{SCRCPY_EMBED_ORIGIN\}\/`\)/);
  assert.match(view, /url\.searchParams\.set\("device", serial\)/);
  assert.match(view, /url\.origin !== SCRCPY_EMBED_ORIGIN/);
  assert.match(view, /url\.protocol !== "http:"/);
  assert.match(view, /url\.hostname !== "127\.0\.0\.1"/);
  assert.match(view, /url\.port !== "8000"/);
  assert.match(view, /url\.pathname !== SCRCPY_EMBED_PATH/);
  assert.match(view, /queryKeys\.length !== 1/);
  assert.match(view, /!url\.href\.startsWith\(SCRCPY_EMBED_URL_PREFIX\)/);
  assert.match(view, /class="android-control-embed-frame"/);
  assert.match(view, /data-android-embed-frame/);
  assert.match(view, /sandbox="allow-scripts allow-same-origin"/);
  assert.match(view, /allow="fullscreen"/);
  assert.match(view, /referrerpolicy="no-referrer"/);
  assert.doesNotMatch(view, /sandbox="[^"]*(?:allow-forms|allow-popups|allow-top-navigation)/);
  assert.doesNotMatch(view, /allow="[^"]*(?:camera|microphone|clipboard)/);
});

test("le handshake vérifie strictement l'origine et la fenêtre source", () => {
  assert.match(view, /event\.origin !== SCRCPY_EMBED_ORIGIN/);
  assert.match(view, /find\(\(candidate\) => event\.source === candidate\.contentWindow\)/);
  assert.match(view, /!androidEmbedSessions\.has\(deviceSerial\)/);
  assert.match(view, /ws-scrcpy-web:theme-ready/);
  assert.match(view, /ws-scrcpy-web:theme-request/);
  assert.match(view, /ws-scrcpy-web:theme/);
  assert.match(view, /postMessage\([\s\S]*?SCRCPY_EMBED_ORIGIN/);
});

test("le panneau représente les quatre états et garde une erreur actionnable", () => {
  assert.match(view, /type AndroidEmbedState = "closed" \| "loading" \| "ready" \| "error"/);
  assert.match(view, /const overallAndroidEmbedState/);
  assert.match(view, /SCRCPY_EMBED_LOAD_TIMEOUT_MS = 12_000/);
  assert.match(view, /frame\.addEventListener\("load"/);
  assert.match(view, /frame\.addEventListener\("error"/);
  assert.match(view, /Le service local ne répond pas/);
  assert.match(view, /bouton scrcpy externe de secours/);
  assert.match(styles, /\.android-control-screen\[data-state="loading"\]/);
  assert.match(styles, /\.android-control-screen\[data-state="ready"\]/);
  assert.match(styles, /\.android-control-screen\[data-state="error"\]/);
});

test("le mur supervise jusqu'à quarante écrans Android indépendants", () => {
  assert.match(view, /const MAX_ANDROID_LIVE_SCREENS = 40/);
  assert.match(view, /const androidEmbedSessions = new Map<string, AndroidEmbedSession>\(\)/);
  assert.match(view, /const androidEmbedLoadTimers = new Map<string, number>\(\)/);
  assert.match(view, /\.slice\(0, availableSlots\)/);
  assert.match(view, /id="androidControlOpenAllEmbeds"/);
  assert.match(view, /id="androidControlCloseAllEmbeds"/);
  assert.match(view, /data-android-screen-grid/);
  assert.match(view, /data-android-reload-screen/);
  assert.match(view, /data-android-close-screen/);
  assert.match(view, /for \(const frame of frames\)/);
  assert.match(styles, /\.android-control-screen-grid\s*\{/);
  assert.match(styles, /repeat\(auto-fit, minmax\(min\(100%, 260px\), 1fr\)\)/);
  assert.match(styles, /content-visibility: auto/);
  assert.match(styles, /contain-intrinsic-size: 52px 540px/);
});

test("le polling existe uniquement pendant que la vue Android est active", () => {
  assert.match(view, /let visible = false/);
  assert.match(view, /export const activateAndroidControlPanel[\s\S]*?visible = true/);
  assert.match(view, /if \(visible && panelRerender\)/);
  assert.match(view, /window\.setInterval/);
  assert.match(view, /export const deactivateAndroidControlPanel[\s\S]*?visible = false/);
  assert.match(view, /window\.clearInterval\(pollTimer\)/);
  assert.match(view, /unbindAndroidEmbedMessages\(\)/);
});

test("la vue précise le rôle du PC Windows et reste responsive", () => {
  assert.match(view, /Le contrôle USB local est disponible depuis le PC Windows connecté au VPS/);
  assert.match(styles, /\.android-control-panel\s*\{/);
  assert.match(styles, /\.android-control-embed-frame\s*\{/);
  assert.match(styles, /@media \(max-width: 900px\)/);
  assert.match(styles, /@media \(max-width: 640px\)/);
  assert.match(styles, /height: min\(62dvh, 520px\)/);
});
