import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");

const modalStart = main.indexOf("const renderProxyManagerModal =");
const modalEnd = main.indexOf("\nconst renderWorkspaceModal", modalStart);
const proxyModal = main.slice(modalStart, modalEnd);

const workflowStart = main.indexOf("const resetProxyManagerDraft =");
const workflowEnd = main.indexOf("\nconst openAgentsModal", workflowStart);
const proxyWorkflow = main.slice(workflowStart, workflowEnd);

test("les paramètres ouvrent une fenêtre Proxys dédiée", () => {
  assert.match(main, /id="settingsProxies"/);
  assert.match(main, /<strong>Proxys<\/strong>/);
  assert.match(
    main,
    /#settingsProxies"\)\?\.addEventListener\("click", \(\) => \{\s*openProxyManagerModal\(\);/,
  );
  assert.match(main, /\$\{renderProxyManagerModal\(\)\}/);
  assert.match(proxyModal, /id="proxyManagerBackdrop"/);
  assert.match(proxyModal, /role="dialog"[^>]*aria-labelledby="proxyManagerTitle"/);
});

test("la fenêtre permet d'ajouter, modifier et retirer des proxys", () => {
  assert.match(proxyModal, /id="addProxyDraft"/);
  assert.match(proxyModal, /data-proxy-draft-field="label"/);
  assert.match(proxyModal, /data-proxy-draft-field="proxyUrl"/);
  assert.match(proxyModal, /data-proxy-draft-field="note"/);
  assert.match(proxyModal, /data-remove-proxy-draft=/);
  assert.match(proxyModal, /data-toggle-proxy-secret=/);
  assert.match(main, /const addProxyManagerDraft = \(\) =>/);
  assert.match(main, /const removeProxyManagerDraft = \(proxyId: string\) =>/);
  assert.match(main, /const proxyUrlIsValid =/);
});

test("Annuler travaille sur un brouillon et ne modifie pas les paramètres actifs", () => {
  assert.match(proxyWorkflow, /settings\.proxies\.map\(\(proxy\) => \(\{ \.\.\.proxy \}\)\)/);
  assert.match(proxyWorkflow, /settings\.accounts\.map\(\(account\) => \[account\.id, account\.proxyId \?\? null\]\)/);
  assert.match(proxyWorkflow, /const closeProxyManagerModal = \(\) =>/);
  assert.match(proxyWorkflow, /resetProxyManagerDraft\(\);/);
  assert.match(proxyModal, /id="cancelProxyManager"/);
});

test("l'enregistrement persiste la liste et les associations par compte", () => {
  assert.match(proxyModal, /data-proxy-account-id=/);
  assert.match(proxyWorkflow, /proxyControlsEnabled: proxyManagerControlsEnabled/);
  assert.match(proxyWorkflow, /proxies: normalizedProxies/);
  assert.match(proxyWorkflow, /accounts: settings\.accounts\.map/);
  assert.match(proxyWorkflow, /proxyId: assignedProxyId && proxyIds\.has\(assignedProxyId\)/);
  assert.match(proxyWorkflow, /invoke<AppSettings>\("save_settings", \{ settings: candidate \}\)/);
  assert.match(proxyWorkflow, /settings\.accounts\.forEach\(syncSessionsForAccount\)/);
});

test("le sélecteur du nouveau chat consomme les proxys enregistrés", () => {
  const chatStart = main.indexOf("const renderNewChatModal =");
  const chatEnd = main.indexOf("\nconst openChatAccountIdsForQuotaSelection", chatStart);
  const newChat = main.slice(chatStart, chatEnd);
  assert.match(newChat, /\.\.\.settings\.proxies\.map/);
  assert.match(newChat, /id="newChatProxy"/);
  assert.match(newChat, /item\.id === chatProxyId \? "selected"/);
});

test("la fenêtre Proxys est utilisable sur ordinateur et mobile", () => {
  assert.match(style, /\.proxy-manager-modal \{/);
  assert.match(style, /\.proxy-manager-fields \{[^}]*grid-template-columns:/s);
  assert.match(style, /@media \(max-width: 720px\) \{[\s\S]*?\.proxy-manager-fields \{ grid-template-columns: minmax\(0, 1fr\); \}/);
});
