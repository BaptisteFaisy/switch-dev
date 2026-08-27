import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8").replace(/\r\n/g, "\n");
const maintenance = read("../src/maintenance.ts");
const main = read("../src/main.ts");
const platform = read("../src/platform.ts");

const block = (source, start, end) => {
  const from = source.indexOf(start);
  const to = source.indexOf(end, from + start.length);
  assert.ok(from >= 0, `debut introuvable: ${start}`);
  assert.ok(to > from, `fin introuvable: ${end}`);
  return source.slice(from, to);
};

test("les types de résultat de nettoyage sont complets", () => {
  assert.match(maintenance, /export type CleanupResultEntry = \{/);
  assert.match(maintenance, /label: string;/);
  assert.match(maintenance, /freedBytes: number;/);
  assert.match(maintenance, /export type CleanupResult = \{/);
  assert.match(maintenance, /startedAt\?: number;/);
  assert.match(maintenance, /finishedAt\?: number;/);
  assert.match(maintenance, /freedTotalBytes\?: number;/);
  assert.match(maintenance, /entries\?: CleanupResultEntry\[\];/);
  assert.match(maintenance, /error\?: string;/);
  assert.match(maintenance, /type CleanupStatus = \{/);
  assert.match(maintenance, /pending: boolean;/);
  assert.match(maintenance, /requestedAt: number;/);
  assert.match(maintenance, /lastResult: CleanupResult \| null;/);
});

test("l'état du module est local et initialisé", () => {
  assert.match(maintenance, /let status: CleanupStatus \| null = null;/);
  assert.match(maintenance, /let requesting = false;/);
  assert.match(maintenance, /let errorMessage = "";/);
  assert.match(maintenance, /let active = false;/);
  assert.match(maintenance, /let pollTimer: number \| null = null;/);
});

test("escapeHtml neutralise les 5 caractères dangereux", () => {
  const escape = block(maintenance, "const escapeHtml", "const formatBytes");
  assert.match(escape, /replaceAll\("&", "&amp;"\)/);
  assert.match(escape, /replaceAll\("<", "&lt;"\)/);
  assert.match(escape, /replaceAll\(">", "&gt;"\)/);
  assert.match(escape, /replaceAll\('"', "&quot;"\)/);
  assert.match(escape, /replaceAll\("'", "&#39;"\)/);
  assert.ok(escape.indexOf("&#39;") >= 0, "apostrophe");
});

test("formatBytes arrondit o, Ko, Mo et Go", () => {
  const format = block(maintenance, "const formatBytes", "const formatWhen");
  assert.match(format, /if \(!Number\.isFinite\(bytes\) \|\| bytes <= 0\) return "0 o";/);
  assert.match(format, /bytes < 1024 \* 1024/);
  assert.match(format, /`\$\{Math\.round\(bytes \/ 1024\)\} Ko`/);
  assert.match(format, /bytes < 1024 \* 1024 \* 1024/);
  assert.match(format, /\(bytes \/ \(1024 \* 1024\)\)\.toFixed\(1\)/);
  assert.match(format, /`\$\{\(bytes \/ \(1024 \* 1024 \* 1024\)\)\.toFixed\(2\)\} Go`/);
  assert.ok(format.indexOf("0 o") >= 0, "zéro octet");
  assert.ok(format.indexOf("Ko") >= 0, "kilo-octets");
  assert.ok(format.indexOf("Mo") >= 0, "méga-octets");
  assert.ok(format.indexOf("Go") >= 0, "giga-octets");
});

test("formatWhen formate en français et ignore les valeurs absentes", () => {
  const when = block(maintenance, "const formatWhen", "const resultSummary");
  assert.match(when, /if \(!unixSeconds\) return "";/);
  assert.match(when, /new Intl\.DateTimeFormat\("fr-FR", \{/);
  assert.match(when, /dateStyle: "short",/);
  assert.match(when, /timeStyle: "medium",/);
  assert.match(when, /unixSeconds \* 1_000/);
});

test("resultSummary concatène les entrées et signale les erreurs", () => {
  const summary = block(maintenance, "const resultSummary", "const stopPolling");
  assert.match(summary, /for \(const entry of result\.entries \?\? \[\]\) \{/);
  assert.match(summary, /`\$\{escapeHtml\(entry\.label\)\} : \$\{formatBytes\(entry\.freedBytes\)\}`/);
  assert.match(summary, /if \(result\.error\) parts\.push\(`Attention : \$\{escapeHtml\(result\.error\)\}`\);/);
  assert.match(summary, /parts\.join\(" · "\);/);
  assert.ok(summary.indexOf("Attention :") >= 0, "alerte d'erreur");
});

test("le polling est stoppable proprement", () => {
  const stop = block(maintenance, "const stopPolling", "export const refreshMaintenanceCard");
  assert.match(stop, /if \(pollTimer !== null\) \{/);
  assert.match(stop, /window\.clearInterval\(pollTimer\);/);
  assert.match(stop, /pollTimer = null;/);
});

test("refreshMaintenanceCard interroge le statut et masque les erreurs silencieuses", () => {
  const refresh = block(maintenance, "export const refreshMaintenanceCard", "const schedulePolling");
  assert.match(refresh, /invoke<CleanupStatus>\("cleanup_status"\)/);
  assert.match(refresh, /errorMessage = "";/);
  assert.match(refresh, /if \(!silent\) errorMessage = String\(error\);/);
  assert.match(refresh, /if \(active && !silent\) rerender\(\);/);
  assert.ok(refresh.indexOf("silent") >= 0, "mode silencieux");
});

test("schedulePolling relance toutes les 4 secondes et s'arrête à la fin", () => {
  const schedule = block(maintenance, "const schedulePolling", "export const renderMaintenanceCard");
  assert.match(schedule, /stopPolling\(\);/);
  assert.match(schedule, /window\.setInterval\(\(\) => \{/);
  assert.match(schedule, /await refreshMaintenanceCard\(rerender, true\);/);
  assert.match(schedule, /if \(status && !status\.pending\) \{/);
  assert.match(schedule, /stopPolling\(\);/);
  assert.match(schedule, /rerender\(\);/);
  assert.match(schedule, /\}, 4_000\);/);
  assert.ok(schedule.indexOf("4_000") >= 0, "intervalle 4 s");
});

test("la carte de maintenance décrit le périmètre du nettoyage", () => {
  const card = block(maintenance, "export const renderMaintenanceCard", "export const bindMaintenanceCard");
  assert.match(card, /maintenance-card/);
  assert.match(card, /Maintenance du PC/);
  assert.match(card, /Nettoie les fichiers inutiles de ce PC/);
  assert.match(card, /caches npm,/);
  assert.match(card, /caches Docker/);
  assert.match(card, /ne touche a aucun projet ni\s*aucune conversation/);
  assert.ok(card.indexOf("temporaires Windows") >= 0, "temporaires Windows");
});

test("la carte reflète l'état pending, l'erreur et le dernier résultat", () => {
  const card = block(maintenance, "export const renderMaintenanceCard", "export const bindMaintenanceCard");
  assert.match(card, /const pending = status\?\.pending \?\? false;/);
  assert.match(card, /const last = status\?\.lastResult \?\? null;/);
  assert.match(card, /maintenance-state-pending/);
  assert.match(card, /Nettoyage en cours sur le PC\.\.\./);
  assert.match(card, /Cette page se met a jour toute seule\./);
  assert.match(card, /maintenance-alert/);
  assert.match(card, /role="alert"/);
  assert.match(card, /maintenance-last-result/);
  assert.match(card, /Dernier nettoyage/);
  assert.match(card, /libérés/);
  assert.match(card, /maintenance-detail/);
  assert.ok(card.indexOf("aria-live=\"polite\"") >= 0, "live region");
});

test("le bouton de nettoyage est désactivé pendant l'activité", () => {
  const card = block(maintenance, "export const renderMaintenanceCard", "export const bindMaintenanceCard");
  assert.match(card, /id="maintenanceCleanButton"/);
  assert.match(card, /maintenance-clean-button/);
  assert.match(card, /pending \|\| requesting \? "disabled" : ""/);
  assert.match(card, /pending \|\| requesting \? "Nettoyage en cours\.\.\." : "Nettoyer mon PC maintenant"/);
});

test("bindMaintenanceCard lance la demande puis surveille", () => {
  const bind = block(maintenance, "export const bindMaintenanceCard", "export const activateMaintenanceCard");
  assert.match(bind, /#maintenanceCleanButton/);
  assert.match(bind, /if \(requesting \|\| status\?\.pending\) return;/);
  assert.match(bind, /requesting = true;/);
  assert.match(bind, /invoke<CleanupStatus>\("cleanup_request"\)/);
  assert.match(bind, /await refreshMaintenanceCard\(rerender, true\);/);
  assert.match(bind, /schedulePolling\(rerender\);/);
  assert.match(bind, /errorMessage = String\(error\);/);
  assert.match(bind, /finally \{/);
  assert.match(bind, /requesting = false;/);
  assert.match(bind, /rerender\(\);/);
});

test("activateMaintenanceCard et deactivate bornent le cycle de vie", () => {
  const activate = maintenance.slice(
    maintenance.indexOf("export const activateMaintenanceCard"),
    maintenance.indexOf("export const deactivateMaintenanceCard"),
  );
  assert.match(activate, /active = true;/);
  assert.match(activate, /void refreshMaintenanceCard\(rerender, true\)\.then\(\(\) => \{/);
  assert.match(activate, /if \(status\?\.pending\) schedulePolling\(rerender\);/);
  const deactivate = maintenance.slice(maintenance.indexOf("export const deactivateMaintenanceCard"));
  assert.match(deactivate, /active = false;/);
  assert.match(deactivate, /stopPolling\(\);/);
});

test("la carte est câblée dans main.ts et le mapping remoteInvoke existe", () => {
  assert.match(main, /renderMaintenanceCard\(\)/);
  assert.match(main, /bindMaintenanceCard\(/);
  assert.match(main, /activateMaintenanceCard\(/);
  assert.match(main, /deactivateMaintenanceCard\(\)/);
  assert.match(platform, /case "cleanup_status":/);
  assert.match(platform, /case "cleanup_request":/);
  assert.match(platform, /\/api\/cleanup\/status/);
  assert.match(platform, /\/api\/cleanup\/request/);
});
