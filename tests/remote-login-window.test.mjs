import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
const popup = readFileSync(new URL("../src/remote-login-window.ts", import.meta.url), "utf8");

const compiledPopup = ts.transpileModule(popup, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const popupModule = await import(
  `data:text/javascript;base64,${Buffer.from(compiledPopup).toString("base64")}`
);

test("la connexion Codex distante ouvre directement OpenAI dans un nouvel onglet", () => {
  assert.match(main, /provider === "codex" && isRemoteMode\(\)/);
  assert.match(main, /openRemoteCodexLoginWindow\(account\.id, account\.label\)/);
  assert.match(
    main,
    /const popupPrepared =[\s\S]*?openRemoteCodexLoginWindow\(account\.id, account\.label\)[\s\S]*?await invoke<AppSettings>\("save_settings"/,
  );
  assert.match(
    popup,
    /window\.open\(CODEX_DEVICE_VERIFICATION_URL, "_blank"\)/,
  );
  assert.doesNotMatch(popup, /popup=yes|width=560|height=720/);
  assert.match(
    main,
    /href="https:\/\/auth\.openai\.com\/codex\/device" target="_blank" rel="noopener"/,
  );
  assert.match(main, /window\.setTimeout\(\(\) => void reloginAccount\(accountId, true\), 0\)/);
});

test("le mode mobile persiste le compte et demarre son terminal avant d'ouvrir OpenAI", () => {
  const flowStart = main.indexOf("const addAccountAndLogin = async () =>");
  const flowEnd = main.indexOf("const deleteSelectedAccount =", flowStart);
  const flow = main.slice(flowStart, flowEnd);

  assert.match(platform, /export const hasMobileExternalHttpsOpener/);
  assert.match(flow, /const nativeMobileLogin = remoteCodexLogin && hasMobileExternalHttpsOpener\(\)/);
  assert.match(
    flow,
    /const deferredMobileLogin =[\s\S]*?nativeMobileLogin \|\| window\.matchMedia\("\(max-width: 860px\)"\)\.matches/,
  );
  assert.match(
    flow,
    /deferredMobileLogin\s*\? prepareRemoteCodexLoginTab\(account\.id, account\.label\)\s*:\s*openRemoteCodexLoginWindow/,
  );
  const persistedAt = flow.indexOf('await invoke<AppSettings>("add_shared_account"');
  const terminalAt = flow.indexOf("await reloginAccount(account.id, deferredMobileLogin)");
  const browserAt = flow.indexOf("await openExternalHttpsUrl(CODEX_DEVICE_VERIFICATION_URL)");
  assert.ok(persistedAt >= 0, "la sauvegarde distante doit rester presente");
  assert.ok(terminalAt > persistedAt, "le terminal doit demarrer apres la sauvegarde");
  assert.ok(browserAt > terminalAt, "Android ne doit passer en arriere-plan qu'en dernier");
  assert.match(flow, /touche « Ouvrir OpenAI » pour terminer la connexion/);
});

test("la fenetre extrait uniquement le lien appareil OpenAI et son code", () => {
  assert.match(popup, /https:\\\/\\\/auth\\\.openai\\\.com\\\/codex\\\/device/);
  assert.match(popup, /\[A-Z0-9\]\{4\}-\[A-Z0-9\]\{4,5\}/);
  assert.match(popup, /stripRemoteLoginControlSequences/);
  assert.match(popup, /CODEX_DEVICE_VERIFICATION_URL/);
});

test("le parseur reconnait la sortie reelle du CLI meme avec des sequences terminal", () => {
  const parsed = popupModule.parseRemoteCodexLoginOutput(`
    \u001b[1mFollow these steps\u001b[0m\r
    https://auth.openai.com/codex/device\r
    Enter this one-time code: HT2W-2WYBH\r
  `);
  assert.equal(parsed.url, "https://auth.openai.com/codex/device");
  assert.equal(parsed.userCode, "HT2W-2WYBH");
  assert.equal(parsed.success, false);

  const completed = popupModule.parseRemoteCodexLoginOutput("Successfully logged in");
  assert.equal(completed.success, true);

  const cursorSeparated = popupModule.parseRemoteCodexLoginOutput(
    "Enter this one-time code: GALF-0YAWS\u001b[14;1HContinue in your browser",
  );
  assert.equal(cursorSeparated.userCode, "GALF-0YAWS");
});

test("le code est copie des qu'il est recu pendant que l'onglet OpenAI est ouvert", () => {
  assert.match(popup, /const renderReady =[^]*?copyDeviceCode\(state\.userCode\)/);
  assert.match(popup, /state\.popup\.focus\(\)/);
  assert.match(main, /code \$\{update\.userCode\} copié, collez-le pour continuer/);
});

test("le terminal de login reste en arriere-plan et est ferme apres validation", () => {
  assert.match(
    main,
    /const backgroundLogin =[\s\S]*?remoteCodexLoginWindowIsOpen\(accountId\)/,
  );
  assert.match(main, /if \(!backgroundLogin\) activeView = "terminal";/);
  assert.match(main, /applyRemoteCodexLoginOutput\(session, event\.payload\.data\)/);
  assert.match(main, /consumeRemoteCodexLoginOutput\(session\.accountId, data\)/);
  assert.match(main, /update\.type === "success"[\s\S]*?closeTerminalSession\(session\.key\)/);
});

test("un onglet OpenAI natif conserve le suivi du device-auth sans WindowProxy", () => {
  assert.match(popup, /export const prepareRemoteCodexLoginTab/);
  assert.match(popup, /popup: null/);
  assert.match(
    popup,
    /if \(!state \|\| \(state\.popup && !popupIsUsable\(state\.popup\)\)\) return false/,
  );
  assert.match(
    popup,
    /if \(!state \|\| \(state\.popup && !popupIsUsable\(state\.popup\)\)\) return \{ type: "none" \}/,
  );
  assert.match(main, /prepareRemoteCodexLoginTab\(account\.id, account\.label\)/);
});

test("un popup bloque conserve le terminal comme solution de repli", () => {
  assert.match(popup, /if \(!popupIsUsable\(popup\)\) return false;/);
  assert.match(
    main,
    /statusText = popupOpened[\s\S]*?: `Ouverture de la connexion/,
  );
});

test("la sortie initiale du terminal est tamponnee puis rejouee dans la fenetre", () => {
  assert.match(platform, /const remoteTerminalOutput = new Map<number, string>\(\)/);
  assert.match(platform, /emitRemoteTerminalData\(id, message\.data\)/);
  assert.match(platform, /case "terminal_output_snapshot":/);
  assert.match(main, /const replayRemoteCodexLoginOutput = async/);
  assert.match(main, /invoke<string>\("terminal_output_snapshot", \{ id: terminalId \}\)/);
  assert.match(main, /applyRemoteCodexLoginOutput\(session, delta\)/);
  assert.match(main, /remoteCodexLoginWindowNeedsCode\(session\.accountId\)/);
});

test("une connexion sans code ne peut plus charger indefiniment", () => {
  assert.match(main, /const deadline = Date\.now\(\) \+ 12_000;/);
  assert.match(main, /Le code OpenAI n’a pas été reçu/);
  assert.match(main, /failRemoteCodexLoginWindow\(session\.accountId, message\)/);
  assert.match(main, /await closeTerminalSession\(session\.key\)/);
});

test("le panneau du code Codex reste lisible et recopiable pendant la connexion", () => {
  const accountId = "acc-panel";
  popupModule.prepareRemoteCodexLoginTab(accountId, "Perso");
  const pending = popupModule.remoteCodexLoginPanel(accountId);
  assert.equal(pending?.phase, "preparing");
  assert.equal(pending?.userCode, null);
  assert.equal(pending?.verificationUrl, "https://auth.openai.com/codex/device");

  const update = popupModule.consumeRemoteCodexLoginOutput(
    accountId,
    "https://auth.openai.com/codex/device\r\nEnter this one-time code: HT2W-2WYBH\r\n",
  );
  assert.equal(update.type, "ready");

  const ready = popupModule.remoteCodexLoginPanel(accountId);
  assert.equal(ready?.phase, "ready");
  assert.equal(ready?.userCode, "HT2W-2WYBH");

  assert.equal(popupModule.copyRemoteCodexLoginCode(accountId), "HT2W-2WYBH");
  assert.equal(popupModule.copyRemoteCodexLoginCode("compte-inconnu"), null);
});

test("la page Comptes affiche un encadre permanent avec le code Codex", () => {
  assert.match(main, /const renderCodexLoginCodePanel = \(accountId: string\): string =>/);
  assert.match(main, /const panel = remoteCodexLoginPanel\(accountId\);/);
  assert.match(main, /renderCodexLoginCodePanel\(item\.id\)/);
  assert.match(main, /class="codex-login-code \$\{ready \? "ready" : "pending"\}"/);
  assert.match(main, /data-copy-codex-code="\$\{escapeAttr\(accountId\)\}"/);
  assert.match(main, /const code = copyRemoteCodexLoginCode\(accountId\);/);
});
