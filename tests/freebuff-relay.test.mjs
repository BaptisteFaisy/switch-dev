import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  FREEBUFF_RELAY_MAX_EXCHANGES,
  FREEBUFF_RELAY_STALL_THRESHOLD_MS,
  FREEBUFF_RELAY_STORAGE_KEY,
  FREEBUFF_RELAY_STORED_OUTPUT_LIMIT,
  addFreebuffRelayExchange,
  appendFreebuffRelayOutput,
  cleanFreebuffRelayOutput,
  clearFreebuffRelayHistory,
  freebuffRelayPayload,
  latestFreebuffRelayExchange,
  loadFreebuffRelayState,
  markFreebuffRelayExchangeFailed,
  markFreebuffRelayExchangeSent,
  markFreebuffRelayExchangeStalled,
  normalizeFreebuffRelayState,
  persistFreebuffRelayState,
  renderFreebuffRelayPanel,
  renderRelayActiveModel,
  resolveFreebuffRelayStall,
  setFreebuffRelayTarget,
} from "../src/freebuff-relay.ts";

const memoryStorage = () => {
  const values = new Map();
  return {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, String(value)),
    values,
  };
};

const fakeSession = (key, overrides = {}) => ({
  key,
  title: "Freebuff · Projet",
  accountLabel: "Compte Freebuff",
  ptyId: 42,
  running: true,
  ...overrides,
});

test("normalise l'état du relais et ignore les échanges invalides", () => {
  const state = normalizeFreebuffRelayState({
    targetKey: " terminal-1 ",
    exchanges: [
      {
        id: "relay-1",
        userText: "  Prépare la démo\r\n puis lance les tests.  ",
        sentAt: 2_000,
        output: "Sortie\nmulti-lignes",
        status: "sent",
      },
      { id: "relay-1", userText: "Doublon", sentAt: 3_000, status: "sent" },
      { id: "relay-empty", userText: "   ", sentAt: 3_000, status: "sent" },
      null,
    ],
  }, 500);
  assert.equal(state.targetKey, "terminal-1");
  assert.equal(state.exchanges.length, 1);
  assert.equal(state.exchanges[0].userText, "Prépare la démo\n puis lance les tests.");
  assert.equal(state.exchanges[0].status, "sent");
  assert.equal(state.exchanges[0].error, null);
});

test("ajoute un échange, le fait passer à envoyé puis à échoué", () => {
  const base = { targetKey: "terminal-1", exchanges: [] };
  const withExchange = addFreebuffRelayExchange(base, "  Lance la vérification  ", 10_000, "relay-1");
  assert.equal(withExchange.exchanges.length, 1);
  assert.equal(withExchange.exchanges[0].status, "sending");
  assert.equal(withExchange.exchanges[0].userText, "Lance la vérification");
  assert.equal(withExchange.exchanges[0].withEnter, true);

  const sent = markFreebuffRelayExchangeSent(withExchange, "relay-1", 10_100);
  assert.equal(sent.exchanges[0].status, "sent");
  assert.equal(sent.exchanges[0].error, null);

  const failed = markFreebuffRelayExchangeFailed(sent, "relay-1", "Terminal fermé", 10_200);
  assert.equal(failed.exchanges[0].status, "failed");
  assert.equal(failed.exchanges[0].error, "Terminal fermé");
  assert.equal(latestFreebuffRelayExchange(failed)?.id, "relay-1");

  assert.deepEqual(addFreebuffRelayExchange(base, "   ", 10_000, "blank"), base);
});

test("le mode sans Entrée est conservé dans l'échange et la charge envoyée", () => {
  const base = { targetKey: "terminal-1", exchanges: [] };
  const raw = addFreebuffRelayExchange(base, "choix-3", 10_000, "raw-1", false);
  assert.equal(raw.exchanges[0].withEnter, false);
  assert.equal(freebuffRelayPayload("choix-3", false), "choix-3");
  assert.equal(freebuffRelayPayload("choix-3", true), "choix-3\r");
  assert.equal(freebuffRelayPayload("ligne 1\nligne 2", true), "ligne 1\rligne 2\r");
  assert.equal(freebuffRelayPayload("   ", true), "");

  const normalized = normalizeFreebuffRelayState({
    targetKey: "terminal-1",
    exchanges: [
      { id: "old", userText: "ancien", sentAt: 1_000, status: "sent" },
      { id: "raw", userText: "menu", sentAt: 2_000, status: "sent", withEnter: false },
    ],
  });
  assert.equal(normalized.exchanges[0].withEnter, true);
  assert.equal(normalized.exchanges[1].withEnter, false);
});

test("détecte le blocage après le seuil sans activité et se réveille à la sortie", () => {
  const base = { targetKey: "terminal-1", exchanges: [] };
  const sent = markFreebuffRelayExchangeSent(
    addFreebuffRelayExchange(base, "lance la tâche", 10_000, "relay-1"),
    "relay-1",
    10_050,
  );

  // En dessous du seuil : rien ne change.
  const early = resolveFreebuffRelayStall(
    sent,
    "relay-1",
    10_050,
    10_050 + FREEBUFF_RELAY_STALL_THRESHOLD_MS - 1_000,
  );
  assert.equal(early.exchanges[0].status, "sent");

  // Aucune activite enregistree : rien ne change non plus.
  assert.equal(
    resolveFreebuffRelayStall(sent, "relay-1", null, 10_050 + FREEBUFF_RELAY_STALL_THRESHOLD_MS)
      .exchanges[0].status,
    "sent",
  );

  // Au-dela du seuil : l'echange passe en « bloque ».
  const stalled = resolveFreebuffRelayStall(
    sent,
    "relay-1",
    10_050,
    10_050 + FREEBUFF_RELAY_STALL_THRESHOLD_MS + 1_000,
  );
  assert.equal(stalled.exchanges[0].status, "stalled");
  assert.match(stalled.exchanges[0].error, /bloqué/);

  // Une sortie qui arrive ensuite re-active l'echange.
  const revived = markFreebuffRelayExchangeSent(stalled, "relay-1", 20_000);
  assert.equal(revived.exchanges[0].status, "sent");
  assert.equal(revived.exchanges[0].error, null);

  // Un echange deja bloque ou echoue ne bouge plus.
  assert.equal(markFreebuffRelayExchangeStalled(stalled, "relay-1").exchanges[0].status, "stalled");
  const failed = markFreebuffRelayExchangeFailed(sent, "relay-1", "boom", 20_000);
  assert.equal(markFreebuffRelayExchangeStalled(failed, "relay-1").exchanges[0].status, "failed");

  // Le statut « stalled » survit a la normalisation.
  const normalized = normalizeFreebuffRelayState({ targetKey: "terminal-1", exchanges: stalled.exchanges });
  assert.equal(normalized.exchanges[0].status, "stalled");
});

test("rend le sélecteur de modèle avec le modèle courant de la cible", () => {
  const storage = memoryStorage();
  const bridge = {
    sessions: () => [fakeSession("terminal-1", { model: "deepseek/deepseek-v4-pro", accountId: "acc-1" })],
    models: () => ["deepseek/deepseek-v4-flash", "deepseek/deepseek-v4-pro"],
    write: async () => true,
    subscribeOutput: () => () => undefined,
    applyModel: async () => ({ ok: true }),
  };
  const panel = renderFreebuffRelayPanel({ storage, bridge, pendingTargetKey: "terminal-1" });
  assert.match(panel, /data-freebuff-relay-model/);
  assert.match(panel, /value="deepseek\/deepseek-v4-pro" selected/);
  assert.match(panel, /deepseek\/deepseek-v4-flash/);
});

test("affiche le modèle actif du TUI et signale la normalisation", () => {
  const target = fakeSession("terminal-1", { model: "deepseek/deepseek-v4-pro" });

  // Modele actif conforme a celui demande : confirme.
  const ok = renderRelayActiveModel(target, "deepseek/deepseek-v4-pro", false, true);
  assert.match(ok, /freebuff-relay-active-model is-ok/);
  assert.match(ok, /deepseek\/deepseek-v4-pro/);
  assert.match(ok, /confirmé dans le TUI/);
  assert.doesNotMatch(ok, /a normalisé/);

  // Le binaire a normalise un modele different de celui demande : alerte.
  const normalized = renderRelayActiveModel(target, "deepseek/deepseek-v4-flash", false, true);
  assert.match(normalized, /freebuff-relay-active-model is-warning/);
  assert.match(normalized, /a normalisé le modèle demandé/);
  assert.match(normalized, /deepseek\/deepseek-v4-flash/);
  assert.match(normalized, /deepseek\/deepseek-v4-pro/);

  // Cle absente dans settings.json : le TUI applique son defaut.
  const unset = renderRelayActiveModel(target, null, false, true);
  assert.match(unset, /freebuff-relay-active-model is-off/);
  assert.match(unset, /défaut du TUI/);

  // Lecture en cours.
  const pending = renderRelayActiveModel(target, null, true, true);
  assert.match(pending, /freebuff-relay-active-model is-pending/);
  assert.match(pending, /Lecture du modèle actif/);

  // Pont sans lecture de modele actif : rien n'est affiche.
  assert.equal(renderRelayActiveModel(target, null, false, false), "");
  assert.equal(renderRelayActiveModel(null, null, false, true), "");
});

test("le panneau lit le modèle actif du TUI quand le pont le permet", () => {
  const storage = memoryStorage();
  const bridge = {
    sessions: () => [fakeSession("terminal-1", { model: "deepseek/deepseek-v4-pro" })],
    write: async () => true,
    subscribeOutput: () => () => undefined,
    activeModel: async () => "deepseek/deepseek-v4-pro",
  };
  const panel = renderFreebuffRelayPanel({ storage, bridge, pendingTargetKey: "terminal-1" });
  // Aucune valeur en cache pour cette cible au premier rendu : lecture attendue.
  assert.match(panel, /freebuff-relay-active-model is-pending/);
  assert.match(panel, /Lecture du modèle actif/);
});

test("rend le bandeau de blocage quand le dernier échange est silencieux", () => {
  const storage = memoryStorage();
  const base = { targetKey: "terminal-1", exchanges: [] };
  const sent = markFreebuffRelayExchangeSent(
    addFreebuffRelayExchange(base, "lance la tâche", 10_000, "relay-1"),
    "relay-1",
    10_050,
  );
  const stalled = markFreebuffRelayExchangeStalled(sent, "relay-1", 10_050);
  persistFreebuffRelayState(stalled, storage);
  const bridge = {
    sessions: () => [fakeSession("terminal-1")],
    write: async () => true,
    subscribeOutput: () => () => undefined,
  };
  const panel = renderFreebuffRelayPanel({ storage, bridge });
  assert.match(panel, /freebuff-relay-stall-banner/);
  assert.match(panel, /Bloqué · silencieux/);
  assert.match(panel, /Terminal sans activité/);
});

test("borne la sortie stockée, nettoie l'ANSI et tronque l'historique", () => {
  const base = { targetKey: "terminal-1", exchanges: [] };
  let state = addFreebuffRelayExchange(base, "message", 10_000, "relay-1");
  state = appendFreebuffRelayOutput(state, "relay-1", "  \x1b[32mok\x1b[0m \r\nsuite ");
  assert.equal(
    state.exchanges[0].output,
    "  \x1b[32mok\x1b[0m \nsuite ",
  );
  assert.equal(cleanFreebuffRelayOutput(state.exchanges[0].output), "  ok \nsuite ");

  const huge = "x".repeat(FREEBUFF_RELAY_STORED_OUTPUT_LIMIT + 500);
  state = appendFreebuffRelayOutput(state, "relay-1", huge);
  assert.ok(state.exchanges[0].output.length <= FREEBUFF_RELAY_STORED_OUTPUT_LIMIT);

  const overflow = { targetKey: "terminal-1", exchanges: [] };
  for (let index = 0; index < FREEBUFF_RELAY_MAX_EXCHANGES + 10; index += 1) {
    const next = addFreebuffRelayExchange(overflow, `message-${index}`, 10_000 + index, `relay-${index}`);
    overflow.exchanges = next.exchanges;
  }
  assert.equal(overflow.exchanges.length, FREEBUFF_RELAY_MAX_EXCHANGES);
  assert.equal(overflow.exchanges[0].userText, "message-10");
});

test("persiste l'état et résiste à un stockage corrompu", () => {
  const storage = memoryStorage();
  const base = { targetKey: "terminal-1", exchanges: [] };
  const state = setFreebuffRelayTarget(
    addFreebuffRelayExchange(base, "message", 10_000, "relay-1"),
    "terminal-1",
  );
  assert.equal(persistFreebuffRelayState(state, storage), true);
  assert.deepEqual(loadFreebuffRelayState(storage), state);
  assert.ok(storage.values.has(FREEBUFF_RELAY_STORAGE_KEY));

  storage.values.set(FREEBUFF_RELAY_STORAGE_KEY, "{invalide");
  assert.deepEqual(loadFreebuffRelayState(storage), { targetKey: null, exchanges: [] });
});

test("rend un panneau échappé avec la cible, le fil et le composeur", () => {
  const storage = memoryStorage();
  const base = { targetKey: "terminal-1", exchanges: [] };
  const state = addFreebuffRelayExchange(
    base,
    '<img src=x onerror="alert(1)">',
    10_000,
    "unsafe",
  );
  persistFreebuffRelayState(state, storage);
  const bridge = {
    sessions: () => [fakeSession("terminal-1"), fakeSession("terminal-2", { ptyId: null, running: false })],
    write: async () => true,
    subscribeOutput: () => () => undefined,
    onOpenTerminalView: () => undefined,
  };
  const panel = renderFreebuffRelayPanel({ storage, bridge, pendingTargetKey: "terminal-1" });
  assert.match(panel, /id="freebuffRelayPanel"/);
  assert.match(panel, /id="freebuffRelayComposer"/);
  assert.match(panel, /id="freebuffRelayTarget"/);
  assert.match(panel, /id="freebuffRelayEnterMode"/);
  assert.match(panel, /data-freebuff-relay-enter/);
  assert.match(panel, /data-freebuff-relay-exchange="unsafe"/);
  assert.doesNotMatch(panel, /<img src=x/);
  assert.match(panel, /&lt;img src=x onerror=&quot;alert\(1\)&quot;&gt;/);
  assert.match(panel, /terminal-2/);
  assert.match(panel, /en démarrage/);
  assert.match(panel, /PTY 42/);

  const rawStorage = memoryStorage();
  const rawState = addFreebuffRelayExchange(
    { targetKey: "terminal-1", exchanges: [] },
    "choix-2",
    10_000,
    "raw-ex",
    false,
  );
  persistFreebuffRelayState(rawState, rawStorage);
  const rawPanel = renderFreebuffRelayPanel({ storage: rawStorage, bridge, enterMode: false });
  assert.match(rawPanel, /sans Entrée/);
  assert.match(rawPanel, /class="freebuff-relay-composer is-raw"/);
  assert.doesNotMatch(rawPanel, /id="freebuffRelayEnterMode" checked/);
});

test("affiche l'état vide quand aucun terminal Freebuff n'est ouvert", () => {
  const storage = memoryStorage();
  const panel = renderFreebuffRelayPanel({
    storage,
    bridge: { sessions: () => [], write: async () => false, subscribeOutput: () => () => undefined },
  });
  assert.match(panel, /Aucun terminal Freebuff ouvert/);
  assert.match(panel, /data-freebuff-relay-open-terminal/);
  assert.match(panel, /freebuffRelayMessage/);
});

test("la vue Relais Freebuff est reliée aux navigations et au runtime", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const initialStyles = ["style.css", "theme.css"]
    .map((file) => readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8"))
    .join("\n");
  const view = readFileSync(new URL("../src/freebuff-relay-view.ts", import.meta.url), "utf8");
  const style = readFileSync(new URL("../src/freebuff-relay.css", import.meta.url), "utf8");

  assert.match(main, /\| "freebuff-relay"/);
  assert.match(main, /id="freebuffRelayToggle"/);
  assert.match(main, /type FreebuffRelayViewModule = typeof import\("\.\/freebuff-relay-view"\)/);
  assert.match(main, /freebuffRelayViewModulePromise = import\("\.\/freebuff-relay-view"\)/);
  assert.match(main, /if \(view === "freebuff-relay" && !freebuffRelayViewModule\)/);
  assert.match(main, /case "freebuff-relay":\s*return freebuffRelayViewModule\?\.renderFreebuffRelayPanel/);
  assert.match(main, /freebuffRelayViewModule\?\.mountFreebuffRelayPanel\(/);
  assert.match(main, /freebuffRelayOutputListeners\.forEach/);
  assert.match(main, /data-relay-terminal=/);
  assert.match(main, /openFreebuffRelayForTerminal\(session\.key\)/);
  assert.match(main, /applyModel: async \(sessionKey: string, model: string\)/);
  assert.match(main, /await applyFreebuffTerminalModel\(session, model\)/);
  assert.match(main, /activeModel: async \(sessionKey: string\)/);
  assert.match(main, /invoke<string \| null>\("freebuff_active_model"/);
  assert.match(view, /import "\.\/freebuff-relay\.css";/);
  assert.match(view, /mountFreebuffRelayPanel/);
  assert.match(view, /renderFreebuffRelayPanel/);
  assert.doesNotMatch(initialStyles, /freebuff-relay/);
  assert.match(style, /\.freebuff-relay-panel/);
  assert.match(style, /\.freebuff-relay-composer textarea/);
  assert.match(style, /\.freebuff-relay-status\.is-failed/);
  assert.match(style, /\.freebuff-relay-enter-toggle/);
  assert.match(style, /\.freebuff-relay-enter-key/);
  assert.match(style, /\.freebuff-relay-raw-badge/);
  assert.match(style, /\.freebuff-relay-status\.is-stalled/);
  assert.match(style, /\.freebuff-relay-stall-banner/);
  assert.match(style, /\.freebuff-relay-active-model/);
});
