import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  SCHEDULED_CHATS_STORAGE_KEY,
  addScheduledChatItem,
  cancelScheduledChatItem,
  claimScheduledChatItem,
  dueScheduledChatItems,
  loadScheduledChatItems,
  localDateTimeInputValue,
  markScheduledChatFailed,
  markScheduledChatLaunched,
  nextScheduledChatAt,
  normalizeScheduledChatItems,
  parseScheduledChatDateTime,
  persistScheduledChatItems,
  recoverInterruptedScheduledChats,
  renderScheduledChatsPanel,
  requestScheduledChatNow,
  rescheduleScheduledChatItem,
  scheduledChatPendingCount,
  scheduledChatTitle,
} from "../src/scheduled-chats.ts";

const memoryStorage = () => {
  const values = new Map();
  return {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, String(value)),
    values,
  };
};

const futureDraft = (scheduledFor = 20_000) => ({
  prompt: "  Vérifie la livraison\r\n puis lance les tests.  ",
  environmentPath: " C:\\Projects\\Produit\\ ",
  accountId: "account-1",
  mode: "build",
  scheduledFor,
});

test("normalise les chats planifiés et ignore les entrées invalides", () => {
  assert.deepEqual(normalizeScheduledChatItems([
    {
      id: "scheduled-1",
      prompt: "  Préparer la démo  ",
      environmentPath: "C:\\Projet\\",
      accountId: " account-1 ",
      mode: "plan",
      scheduledFor: 2_000,
      status: "scheduled",
      createdAt: 1_000,
      updatedAt: 1_100,
    },
    { id: "scheduled-1", prompt: "Doublon", environmentPath: "/tmp", scheduledFor: 3_000 },
    { id: "missing-prompt", prompt: "  ", environmentPath: "/tmp", scheduledFor: 3_000 },
    null,
  ], 500), [{
    id: "scheduled-1",
    prompt: "Préparer la démo",
    environmentPath: "C:\\Projet",
    accountId: "account-1",
    mode: "plan",
    scheduledFor: 2_000,
    status: "scheduled",
    createdAt: 1_000,
    updatedAt: 1_100,
    launchedAt: null,
    error: null,
  }]);
});

test("ajoute uniquement une programmation future et calcule la prochaine échéance", () => {
  const items = addScheduledChatItem([], futureDraft(), 10_000, "scheduled-1");
  assert.equal(items.length, 1);
  assert.equal(items[0].prompt, "Vérifie la livraison\n puis lance les tests.");
  assert.equal(items[0].environmentPath, "C:\\Projects\\Produit");
  assert.equal(items[0].status, "scheduled");
  assert.equal(scheduledChatPendingCount(items), 1);
  assert.equal(nextScheduledChatAt(items), 20_000);
  assert.deepEqual(dueScheduledChatItems(items, 19_999), []);
  assert.deepEqual(dueScheduledChatItems(items, 20_000).map((item) => item.id), ["scheduled-1"]);
  assert.deepEqual(addScheduledChatItem(items, futureDraft(9_000), 10_000, "past"), items);
});

test("fait évoluer une programmation du claim au résultat et permet le rattrapage", () => {
  const scheduled = addScheduledChatItem([], futureDraft(), 10_000, "scheduled-1");
  const claimed = claimScheduledChatItem(scheduled, "scheduled-1", 20_000);
  assert.equal(claimed[0].status, "launching");
  assert.equal(dueScheduledChatItems(claimed, 30_000).length, 0);

  const launched = markScheduledChatLaunched(claimed, "scheduled-1", 21_000);
  assert.equal(launched[0].status, "launched");
  assert.equal(launched[0].launchedAt, 21_000);
  assert.equal(scheduledChatTitle(launched[0]), "Vérifie la livraison");

  const retry = requestScheduledChatNow(launched, "scheduled-1", 22_000);
  assert.equal(retry[0].status, "scheduled");
  assert.equal(retry[0].scheduledFor, 21_999);
  assert.equal(dueScheduledChatItems(retry, 22_000).length, 1);

  const failed = markScheduledChatFailed(
    claimScheduledChatItem(retry, "scheduled-1", 22_000),
    "scheduled-1",
    "Compte indisponible",
    23_000,
  );
  assert.equal(failed[0].status, "failed");
  assert.equal(failed[0].error, "Compte indisponible");

  const rescheduled = rescheduleScheduledChatItem(failed, "scheduled-1", 40_000, 30_000);
  assert.equal(rescheduled[0].status, "scheduled");
  assert.equal(rescheduled[0].error, null);
  assert.equal(cancelScheduledChatItem(rescheduled, "scheduled-1", 31_000)[0].status, "cancelled");

  const interrupted = claimScheduledChatItem(rescheduled, "scheduled-1", 40_000);
  const recovered = recoverInterruptedScheduledChats(interrupted, 50_000, 5_000);
  assert.equal(recovered[0].status, "failed");
  assert.match(recovered[0].error, /interrompu/);
});

test("persiste les programmations et résiste à un stockage corrompu", () => {
  const storage = memoryStorage();
  const items = addScheduledChatItem([], futureDraft(), 10_000, "scheduled-1");
  assert.equal(persistScheduledChatItems(items, storage), true);
  assert.deepEqual(loadScheduledChatItems(storage), items);
  assert.ok(storage.values.has(SCHEDULED_CHATS_STORAGE_KEY));

  storage.values.set(SCHEDULED_CHATS_STORAGE_KEY, "{invalide");
  assert.deepEqual(loadScheduledChatItems(storage), []);
});

test("convertit correctement une date locale pour le champ date/heure", () => {
  const date = new Date(2026, 6, 15, 14, 35, 0, 0);
  const value = localDateTimeInputValue(date.getTime());
  assert.equal(value, "2026-07-15T14:35");
  assert.equal(parseScheduledChatDateTime(value), date.getTime());
  assert.equal(parseScheduledChatDateTime("15/07/2026 14:35"), null);
});

test("rend un panneau échappé avec l'heure, l'environnement et les actions", () => {
  const storage = memoryStorage();
  const now = Date.now();
  const items = addScheduledChatItem([], {
    prompt: '<img src=x onerror="alert(1)">',
    environmentPath: "C:\\Projects\\Produit",
    accountId: "account-1",
    mode: "build",
    scheduledFor: now + 60 * 60_000,
  }, now, "unsafe");
  persistScheduledChatItems(items, storage);
  const panel = renderScheduledChatsPanel({
    storage,
    environments: [{ path: "C:\\Projects\\Produit", label: "Produit" }],
    accounts: [{ id: "account-1", label: "Compte principal" }],
    defaultEnvironmentPath: "C:\\Projects\\Produit",
    defaultAccountId: "account-1",
  });
  assert.match(panel, /id="scheduledChatsPanel"/);
  assert.match(panel, /id="scheduledChatCreateForm"/);
  assert.match(panel, /data-scheduled-chat-run="unsafe"/);
  assert.match(panel, /data-scheduled-chat-reschedule="unsafe"/);
  assert.match(panel, /Produit/);
  assert.match(panel, /Compte principal/);
  assert.doesNotMatch(panel, /<img src=x/);
  assert.match(panel, /&lt;img src=x onerror=&quot;alert\(1\)&quot;&gt;/);
  assert.match(panel, /class="scheduled-chat-info-trigger"[^>]*aria-describedby="scheduledChatLaunchInfo"/);
  assert.match(panel, /id="scheduledChatLaunchInfo" class="scheduled-chat-info-tooltip" role="tooltip"/);
  assert.doesNotMatch(panel, /Les tâches démarrent automatiquement à l’heure locale de cet appareil\./);
});

test("la vue Chat planifié est reliée aux navigations et au runtime", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const types = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const initialStyles = ["style.css", "theme.css"]
    .map((file) => readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8"))
    .join("\n");
  const view = readFileSync(new URL("../src/scheduled-chats-view.ts", import.meta.url), "utf8");
  const style = readFileSync(new URL("../src/scheduled-chats.css", import.meta.url), "utf8");

  assert.match(types, /\| "scheduled-chat"/);
  assert.match(main, /id="scheduledChatToggle"/);
  assert.match(main, /role="menuitem" data-view="scheduled-chat"/);
  assert.match(main, /type ScheduledChatsViewModule = typeof import\("\.\/scheduled-chats-view"\)/);
  assert.match(main, /scheduledChatsViewModulePromise = import\("\.\/scheduled-chats-view"\)/);
  assert.match(main, /if \(view === "scheduled-chat" && !scheduledChatsViewModule\)/);
  assert.match(main, /case "scheduled-chat":\s*return scheduledChatsViewModule\?\.renderScheduledChatsPanel/);
  assert.match(main, /scheduledChatsViewModule\?\.mountScheduledChatsPanel\(/);
  assert.match(main, /claimScheduledChatItem\(/);
  assert.match(main, /await executeScheduledChatItem\(claimed\)/);
  assert.match(main, /await startNewChatWithPrompt\(pane, item\.prompt\)/);
  assert.match(main, /startScheduledChatScheduler\(\)/);
  assert.match(main, /stopScheduledChatScheduler\(\)/);
  assert.match(view, /import "\.\/scheduled-chats\.css";/);
  assert.match(view, /mountScheduledChatsPanel/);
  assert.match(view, /renderScheduledChatsPanel/);
  assert.doesNotMatch(initialStyles, /scheduled-chat/);
  assert.match(style, /\.scheduled-chats-panel/);
  assert.match(style, /\.scheduled-chat-info-tooltip\s*{[\s\S]*?visibility:\s*hidden/);
  assert.match(style, /\.scheduled-chat-info-trigger:hover \+ \.scheduled-chat-info-tooltip/);
  assert.match(style, /\.scheduled-chat-empty h3[^}]*font:\s*620 11px/);
  assert.match(style, /\.scheduled-chat-empty p[^}]*font-size:\s*8px/);
  assert.match(style, /@media \(max-width: 560px\)[\s\S]*?\.scheduled-chat-create/);
  assert.match(
    style,
    /@media \(max-width: 560px\)[\s\S]*?\.scheduled-chat-info-tooltip\s*\{[^}]*left:\s*50%;[^}]*transform:\s*translate\(-50%, 3px\);[^}]*\}[\s\S]*?\.scheduled-chat-info-trigger:hover \+ \.scheduled-chat-info-tooltip,[\s\S]*?transform:\s*translate\(-50%, 0\);/,
  );
});
