import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  BUG_REPORT_SOURCE_PREFIX,
  bugReportSeverityLabel,
  bugReportTitleFromAgent,
  buildBugReportObjective,
  createBugReportSourceKey,
  emptyBugReportDraft,
  isBugReportAgent,
} from "../src/bug-report.ts";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const types = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");

test("un signalement est identifié durablement parmi les agents autonomes", () => {
  const sourceChatKey = createBugReportSourceKey(" ticket 42 / interface ");
  assert.equal(sourceChatKey, `${BUG_REPORT_SOURCE_PREFIX}ticket-42-interface`);
  assert.equal(isBugReportAgent({ sourceChatKey }), true);
  assert.equal(isBugReportAgent({ sourceChatKey: "chat:42" }), false);
  assert.equal(bugReportTitleFromAgent({ name: "Bug · Bouton bloqué" }), "Bouton bloqué");
});

test("la mission demande une reproduction, un correctif minimal et une validation", () => {
  const draft = {
    ...emptyBugReportDraft(),
    title: "Le bouton Enregistrer ne répond plus",
    description: "Le clic ne produit aucun effet.",
    steps: "Ouvrir les paramètres puis cliquer sur Enregistrer.",
    expected: "Les paramètres sont enregistrés.",
    actual: "La page reste inchangée.",
    severity: "high",
    testCommand: "npm test",
    projectDir: "C:/projet",
    accountId: "account-1",
  };
  const objective = buildBugReportObjective(draft);

  assert.match(objective, /Le bouton Enregistrer ne répond plus/);
  assert.match(objective, /Gravité\nÉlevée/);
  assert.match(objective, /reproduis le bug/i);
  assert.match(objective, /plus petit correctif sûr/i);
  assert.match(objective, /test de non-régression/i);
  assert.match(objective, /`npm test`/);
  assert.match(objective, /ne pousse aucun commit/i);
  assert.equal(bugReportSeverityLabel("blocking"), "Bloquant");
});

test("l'onglet lance immédiatement un agent build et expose son suivi", () => {
  assert.match(types, /\| "bug-report"/);
  assert.match(main, /id="bugReportToggle"/);
  assert.match(main, /data-view="bug-report"/);
  assert.match(main, /id="bugReportForm"/);
  assert.match(main, /Signaler et lancer l’agent/);
  assert.match(main, /create_autonomous_agent/);
  assert.match(main, /sourceChatKey: createBugReportSourceKey/);
  assert.match(main, /mode: "build"/);
  assert.match(main, /deferFirstRun: false/);
  assert.match(main, /intervalSeconds: 15 \* 60/);
  assert.match(main, /data-bug-report-monitor/);
  assert.match(main, /activeView === "bug-report"/);
  assert.match(style, /\.bug-report-panel/);
  assert.match(style, /\.chat-side-bug-report-entry/);
  assert.match(style, /\.chat-app-layout\.is-bug-report/);
});
