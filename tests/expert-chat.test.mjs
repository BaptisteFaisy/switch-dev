import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  DEFAULT_EXPERT_CHAT_DISPLAY_MODE,
  DEFAULT_EXPERT_CHAT_PAGE_SIZE,
  DEFAULT_EXPERT_CHAT_PAGE_SIZE_MODE,
  EXPERT_CHAT_MIN_PANE_HEIGHT,
  EXPERT_CHAT_MIN_PANE_WIDTH,
  clampExpertChatPage,
  expertChatPageCount,
  expertChatPageForIndex,
  expertChatColumnCount,
  expertChatGridDimensions,
  expertChatHistoryOpenAfterFullscreenChange,
  expertChatResponsiveCapacity,
  expertChatResponsiveGridDimensions,
  expertChatRowCount,
  expertChatsForDisplay,
  expertChatsOpenedByUser,
  expertChatsOnPage,
  normalizeExpertChatDisplayMode,
  normalizeExpertChatPageSize,
  normalizeExpertChatPageSizeMode,
  resolveExpertChatPageSize,
  shouldMinimizeActiveBusyExpertChat,
  shouldPinActiveExpertChatDuringTurn,
  shouldPinRestoredBusyExpertChat,
} from "../src/chat/expert.ts";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");

test("la grille accepte tout nombre entier positif de chats par page", () => {
  assert.equal(DEFAULT_EXPERT_CHAT_PAGE_SIZE, 6);
  assert.equal(normalizeExpertChatPageSize("6"), 6);
  assert.equal(normalizeExpertChatPageSize("9"), 9);
  assert.equal(normalizeExpertChatPageSize("12"), 12);
  assert.equal(normalizeExpertChatPageSize("16"), 16);
  assert.equal(normalizeExpertChatPageSize("15"), 15);
  assert.equal(normalizeExpertChatPageSize("37"), 37);
  assert.equal(normalizeExpertChatPageSize("2.8"), 2);
  assert.equal(normalizeExpertChatPageSize("0"), 1);
  assert.equal(expertChatColumnCount(12), 4);
  assert.equal(expertChatColumnCount(16), 4);
  assert.equal(expertChatColumnCount(17), 5);
  assert.equal(expertChatRowCount(6), 2);
  assert.equal(expertChatRowCount(9), 3);
  assert.equal(expertChatRowCount(12), 3);
  assert.equal(expertChatRowCount(16), 4);
  assert.equal(expertChatRowCount(17), 4);
});

test("le mode auto adapte la grille au nombre de chats visibles", () => {
  assert.equal(DEFAULT_EXPERT_CHAT_PAGE_SIZE_MODE, "auto");
  assert.equal(normalizeExpertChatPageSizeMode(null), "auto");
  assert.equal(normalizeExpertChatPageSizeMode("auto"), "auto");
  assert.equal(normalizeExpertChatPageSizeMode(""), "auto");
  assert.equal(normalizeExpertChatPageSizeMode("9"), 9);
  assert.equal(normalizeExpertChatPageSizeMode("23"), 23);
  assert.equal(resolveExpertChatPageSize("auto"), 16);

  assert.deepEqual(expertChatGridDimensions(0, "auto"), { columns: 1, rows: 1 });
  assert.deepEqual(expertChatGridDimensions(1, "auto"), { columns: 1, rows: 1 });
  assert.deepEqual(expertChatGridDimensions(2, "auto"), { columns: 2, rows: 1 });
  assert.deepEqual(expertChatGridDimensions(3, "auto"), { columns: 3, rows: 1 });
  assert.deepEqual(expertChatGridDimensions(4, "auto"), { columns: 2, rows: 2 });
  assert.deepEqual(expertChatGridDimensions(6, "auto"), { columns: 3, rows: 2 });
  assert.deepEqual(expertChatGridDimensions(10, "auto"), { columns: 4, rows: 3 });
  assert.deepEqual(expertChatGridDimensions(16, "auto"), { columns: 4, rows: 4 });
  assert.deepEqual(expertChatGridDimensions(23, 23), { columns: 5, rows: 5 });
  assert.deepEqual(expertChatGridDimensions(37, 37), { columns: 7, rows: 6 });
});

test("le mode d'affichage peut ne garder que les chats disponibles", () => {
  const chats = [
    { id: "new", status: "idle" },
    { id: "working", status: "running" },
    { id: "syncing", status: "finalizing" },
    { id: "done", status: "completed" },
  ];
  const isAvailable = (chat) => !["running", "finalizing"].includes(chat.status);

  assert.equal(DEFAULT_EXPERT_CHAT_DISPLAY_MODE, "all");
  assert.equal(normalizeExpertChatDisplayMode(null), "all");
  assert.equal(normalizeExpertChatDisplayMode("invalid"), "all");
  assert.equal(normalizeExpertChatDisplayMode("available"), "available");
  assert.deepEqual(expertChatsForDisplay(chats, "all", isAvailable), chats);
  assert.deepEqual(
    expertChatsForDisplay(chats, "available", isAvailable).map((chat) => chat.id),
    ["new", "done"],
  );
});

test("le premier plan ne contient que les chats ouverts par l'utilisateur", () => {
  const chats = [
    { id: "manual", userOpened: true },
    { id: "mobile", userOpened: false },
    { id: "worker", userOpened: false },
    { id: "resumed", userOpened: true },
  ];

  assert.deepEqual(
    expertChatsOpenedByUser(chats, (chat) => chat.userOpened).map((chat) => chat.id),
    ["manual", "resumed"],
  );
  assert.deepEqual(chats.map((chat) => chat.id), ["manual", "mobile", "worker", "resumed"]);
});

test("les chats synchronises et les workers restent en arriere-plan jusqu'a un clic", () => {
  assert.match(
    main,
    /const userOpenedExpertChatPanesForCurrentEnvironment = \(\): ExpertChatPane\[\] =>[\s\S]*?expertChatsOpenedByUser/,
  );
  assert.match(
    main,
    /orchestrationRole: "worker",[\s\S]*?userOpened: false|userOpened: false,[\s\S]*?orchestrationRole: "worker"/,
  );
  assert.match(
    main,
    /key: syncedChatPaneKey\(turn\),[\s\S]*?userOpened: false/,
  );
  assert.match(
    main,
    /const activateExpertChatPane = [\s\S]*?pane\.userOpened = true;[\s\S]*?pendingActiveExpertChatKey = null;/,
  );
  const adoption = main.slice(
    main.indexOf("const adoptMissingActiveChatTurns ="),
    main.indexOf("const refreshActiveChatTurns = async"),
  );
  assert.doesNotMatch(adoption, /\brender\(\)/);
});

test("la restauration migre les anciennes discussions hors du premier plan", () => {
  assert.match(main, /v: 2,/);
  assert.match(
    main,
    /persistedVersion === 2 \? record\.userOpened !== false : !record\.sessionId/,
  );
  assert.match(
    main,
    /pendingActiveExpertChatKey[\s\S]*?activeExpertChatKey = pendingActiveExpertChatKey;[\s\S]*?reconcileExpertChatPage\(\)/,
  );
});

test("le chat actif reste epingle pendant le tour lance en mode Disponibles", () => {
  assert.equal(shouldPinActiveExpertChatDuringTurn("all", true, true), false);
  assert.equal(shouldPinActiveExpertChatDuringTurn("available", false, true), false);
  assert.equal(shouldPinActiveExpertChatDuringTurn("available", true, false), false);
  assert.equal(shouldPinActiveExpertChatDuringTurn("available", true, true), true);

  const sendStart = main.indexOf("const sendExpertChatMessage = async");
  const sendEnd = main.indexOf("const drainExpertChatSubmissionQueue", sendStart);
  const sendSource = main.slice(sendStart, sendEnd);
  const runningTurn = sendSource.indexOf('status: "running"');
  const pinDecision = sendSource.indexOf("shouldPinActiveExpertChatDuringTurn", runningTurn);
  const displayRefresh = sendSource.indexOf(
    "refreshExpertChatDisplayAfterAvailabilityChange",
    pinDecision,
  );
  assert.ok(runningTurn >= 0, "le tour optimiste doit passer en cours");
  assert.ok(pinDecision > runningTurn, "l'epingle doit etre posee apres le passage en cours");
  assert.ok(displayRefresh > pinDecision, "l'epingle doit preceder le recalcul du filtre");
});

test("un chat occupe restaure redevient visible une seule fois", () => {
  assert.equal(shouldPinRestoredBusyExpertChat("all", false, true), false);
  assert.equal(shouldPinRestoredBusyExpertChat("available", false, false), false);
  assert.equal(shouldPinRestoredBusyExpertChat("available", false, true), true);
  assert.equal(
    shouldPinRestoredBusyExpertChat("available", true, true),
    false,
    "un chat deja rattache peut rester volontairement reduit",
  );

  const refreshStart = main.indexOf("const refreshActiveChatTurns = async");
  const refreshEnd = main.indexOf("const scheduleActiveChatTurnsRefresh", refreshStart);
  const refreshSource = main.slice(refreshStart, refreshEnd);
  assert.match(refreshSource, /shouldPinRestoredBusyExpertChat\(/);
  assert.match(refreshSource, /explicitlyOpenedBusyChatVisibilityPins\.add\(pane\.key\)/);
});

test("un changement de disponibilite ne vole pas le focus du chat en cours", () => {
  const renderStart = main.indexOf("const render = () =>");
  const renderEnd = main.indexOf("const renderLegacyTerminalShell", renderStart);
  const renderSource = main.slice(renderStart, renderEnd);
  const capture = renderSource.indexOf("captureFocusedExpertChatPrompt()");
  const redraw = renderSource.indexOf("renderChatFirstShell()", capture);
  const restore = renderSource.indexOf("restoreFocusedExpertChatPrompt", redraw);
  assert.ok(capture >= 0, "le focus du prompt doit etre capture avant le rendu");
  assert.ok(redraw > capture, "la capture doit preceder le rendu global");
  assert.ok(restore > redraw, "le prompt doit retrouver le focus apres le rendu global");
  assert.match(main, /prompt\.setSelectionRange\(selectionStart, selectionEnd, snapshot\.selectionDirection\)/);
});

test("les preferences plein ecran pilotent independamment l'historique", () => {
  const keep = { openOnFullscreen: false, closeOnCompact: false };
  assert.equal(expertChatHistoryOpenAfterFullscreenChange(false, true, keep), false);
  assert.equal(expertChatHistoryOpenAfterFullscreenChange(true, false, keep), true);

  const automatic = { openOnFullscreen: true, closeOnCompact: true };
  assert.equal(
    expertChatHistoryOpenAfterFullscreenChange(false, true, automatic),
    true,
    "agrandir doit ouvrir l'historique",
  );
  assert.equal(
    expertChatHistoryOpenAfterFullscreenChange(true, false, automatic),
    false,
    "reduire doit fermer l'historique",
  );
});

test("les deux automatismes d'historique sont persistants dans Parametres", () => {
  for (const marker of [
    'data-chat-history-open-fullscreen="on"',
    'data-chat-history-close-compact="on"',
    "EXPERT_CHAT_HISTORY_OPEN_FULLSCREEN_STORAGE_KEY",
    "EXPERT_CHAT_HISTORY_CLOSE_COMPACT_STORAGE_KEY",
    "expertChatHistoryOpenOnFullscreen = loadExpertChatHistoryOpenOnFullscreen()",
    "expertChatHistoryCloseOnCompact = loadExpertChatHistoryCloseOnCompact()",
  ]) {
    assert.ok(main.includes(marker), `reglage d'historique manquant: ${marker}`);
  }
  assert.match(
    main,
    /const nextFullscreen = expertChatFullscreenKey !== pane\.key;[\s\S]*?pane\.historyOpen = expertChatHistoryOpenAfterFullscreenChange\([\s\S]*?openOnFullscreen: expertChatHistoryOpenOnFullscreen,[\s\S]*?closeOnCompact: expertChatHistoryCloseOnCompact/,
  );
});

test("le reglage Disponibles pilote le mur principal et reste persistant", () => {
  assert.match(main, /id="chatDisplaySettingsTitle">Affichage de la fenêtre principale/);
  assert.match(main, /data-chat-display-mode="all"/);
  assert.match(main, /data-chat-display-mode="available"/);
  assert.match(
    main,
    /localStorage\.setItem\(EXPERT_CHAT_DISPLAY_MODE_STORAGE_KEY, nextMode\)/,
  );
  assert.match(main, /expertChatDisplayMode = loadExpertChatDisplayMode\(\)/);
  assert.match(
    main,
    /expertChatsForDisplay\(\s*userOpenedExpertChatPanesForCurrentEnvironment\(\),\s*expertChatDisplayMode,\s*expertChatPaneIsAvailable,?\s*\)/,
  );
});

test("un chat masque reste suivi et le mur est rerendu a la fin du tour", () => {
  assert.match(
    main,
    /expertChatDisplayMode === "available"\s*&& currentEnvironmentPanes\.has\(pane\)/,
  );
  assert.match(main, /refreshExpertChatDisplayAfterAvailabilityChange/);
  assert.match(
    main,
    /const wasAvailable = expertChatPaneIsAvailable\(pane\);[\s\S]*?pane\.turn = \{[\s\S]*?status: "running"[\s\S]*?refreshExpertChatDisplayAfterAvailabilityChange\(\s*pane,\s*wasAvailable,?\s*\)/,
  );
  assert.match(main, /Ils réapparaîtront automatiquement à la fin/);
});

test("un chat en cours ouvert explicitement reste visible en mode Disponibles", () => {
  assert.match(
    main,
    /const explicitlyOpenedBusyChatVisibilityPins = new Set<string>\(\)/,
  );
  assert.match(
    main,
    /const expertChatPaneIsAvailable = \(pane: ExpertChatPane\): boolean =>[\s\S]*?explicitlyOpenedBusyChatVisibilityPins\.has\(pane\.key\)/,
  );
  assert.match(
    main,
    /const pinExplicitlyOpenedBusyExpertChat = \(pane: ExpertChatPane\): void =>[\s\S]*?expertChatDisplayMode === "available"[\s\S]*?explicitlyOpenedBusyChatVisibilityPins\.add\(pane\.key\)/,
  );
  assert.match(
    main,
    /const openDiscussionChat = async[\s\S]*?openDiscussionInExpert\(discussion, true\)/,
  );
  assert.equal(
    (main.match(/if \(revealBusyChat\) pinExplicitlyOpenedBusyExpertChat\(/g) ?? []).length,
    2,
  );
  assert.match(
    main,
    /\[data-open-pane\][\s\S]*?pinExplicitlyOpenedBusyExpertChat\(pane\);[\s\S]*?activateExpertChatPane\(pane, true\)/,
  );
  assert.match(
    main,
    /if \(!chatTurnIsBusy\(snapshot\.status\)\) \{[\s\S]*?explicitlyOpenedBusyChatVisibilityPins\.delete\(pane\.key\)/,
  );
});

test("un second clic sur le chat orange actif le reduit en mode Disponibles", () => {
  assert.equal(shouldMinimizeActiveBusyExpertChat("all", true, true, true), false);
  assert.equal(shouldMinimizeActiveBusyExpertChat("available", false, true, true), false);
  assert.equal(shouldMinimizeActiveBusyExpertChat("available", true, false, true), false);
  assert.equal(shouldMinimizeActiveBusyExpertChat("available", true, true, false), false);
  assert.equal(shouldMinimizeActiveBusyExpertChat("available", true, true, true), true);

  const minimizeStart = main.indexOf("const minimizeActiveBusyExpertChat =");
  const minimizeEnd = main.indexOf("const displayedExpertChatPanesForCurrentEnvironment", minimizeStart);
  const minimizeSource = main.slice(minimizeStart, minimizeEnd);
  assert.match(minimizeSource, /activeView !== "chat"/);
  assert.match(minimizeSource, /automaticQuotaResumeVisibilityPins\.delete\(pane\.key\)/);
  assert.match(minimizeSource, /explicitlyOpenedBusyChatVisibilityPins\.delete\(pane\.key\)/);
  assert.match(minimizeSource, /reconcileExpertChatPage\(\)/);
  assert.match(minimizeSource, /startAllExpertChatWork\(\)/);

  const openStart = main.indexOf("const openDiscussionChat = async");
  const openEnd = main.indexOf("// --- Grille de chats persistants", openStart);
  const openSource = main.slice(openStart, openEnd);
  assert.ok(
    openSource.indexOf("minimizeActiveBusyExpertChat(existing)")
      < openSource.indexOf("openDiscussionInExpert(discussion, true)"),
    "le second clic doit reduire le chat avant de tenter de l'ouvrir a nouveau",
  );
  assert.match(
    main,
    /\[data-open-pane\][\s\S]*?if \(minimizeActiveBusyExpertChat\(pane\)\) return;[\s\S]*?pinExplicitlyOpenedBusyExpertChat\(pane\)/,
  );
});

test("la grille s'adapte aux chats visibles meme si la page est incomplete", () => {
  assert.deepEqual(expertChatGridDimensions(1, 6), { columns: 1, rows: 1 });
  assert.deepEqual(expertChatGridDimensions(2, 9), { columns: 2, rows: 1 });
  assert.deepEqual(expertChatGridDimensions(3, 16), { columns: 3, rows: 1 });
  assert.deepEqual(expertChatGridDimensions(5, 37), { columns: 3, rows: 2 });
});

test("le mode auto adapte sa capacite a la taille utile de l'ecran", () => {
  assert.equal(EXPERT_CHAT_MIN_PANE_WIDTH, 340);
  assert.equal(EXPERT_CHAT_MIN_PANE_HEIGHT, 270);
  assert.equal(expertChatResponsiveCapacity(1600, 1000), 12);
  assert.equal(expertChatResponsiveCapacity(1112, 814), 6);
  assert.equal(expertChatResponsiveCapacity(952, 634), 4);
  assert.equal(expertChatResponsiveCapacity(696, 682), 4);
  assert.equal(expertChatResponsiveCapacity(652, 564), 2);
  assert.equal(expertChatResponsiveCapacity(572, 514), 1);
  assert.equal(expertChatResponsiveCapacity(Number.NaN, 0), 1);
});

test("la grille responsive privilegie des panneaux lisibles sans rogner le mode manuel", () => {
  assert.deepEqual(
    expertChatResponsiveGridDimensions(6, 1600, 1000),
    { columns: 3, rows: 2 },
  );
  assert.deepEqual(
    expertChatResponsiveGridDimensions(3, 1112, 814),
    { columns: 2, rows: 2 },
  );
  assert.deepEqual(
    expertChatResponsiveGridDimensions(4, 696, 682),
    { columns: 2, rows: 2 },
  );
  assert.deepEqual(
    expertChatResponsiveGridDimensions(6, 696, 682),
    { columns: 2, rows: 3 },
  );
  assert.deepEqual(
    expertChatResponsiveGridDimensions(6, 572, 514),
    { columns: 1, rows: 6 },
  );
});

test("l'interface permet un nombre libre et persiste le mode choisi", () => {
  assert.match(
    main,
    /id="expertChatPageSize" type="number" min="1" step="1"[^>]*placeholder="Auto"/,
  );
  assert.match(
    main,
    /responsiveExpertChatGridDimensions\(pagePanes\.length\)/,
  );
  assert.doesNotMatch(main, /expertChatWallTerminals|wallTerminals/);
  assert.match(
    main,
    /localStorage\.setItem\(EXPERT_CHATS_PER_PAGE_STORAGE_KEY, String\(expertChatPageSizeMode\)\)/,
  );
  assert.match(
    style,
    /grid-template-rows:\s*repeat\([\s\S]*?var\(--expert-chat-rows\),[\s\S]*?minmax\(var\(--expert-chat-pane-min-height\),\s*1fr\)[\s\S]*?\);/,
  );
  assert.match(main, /effectiveExpertChatPageSizeMode/);
  assert.match(main, /scheduleExpertChatResponsiveRender\(\)/);
  assert.match(style, /@container expert-chat-pane \(max-width: 500px\)/);
  assert.doesNotMatch(style, /grid-auto-rows:\s*minmax\(520px,/);
});

test("le bandeau superieur des chats peut etre masque puis restaure", () => {
  assert.match(main, /id="expertChatToolbarHide"/);
  assert.match(main, /id="expertChatToolbarShow"/);
  assert.match(
    main,
    /localStorage\.setItem\(EXPERT_CHAT_TOOLBAR_HIDDEN_STORAGE_KEY, String\(hidden\)\)/,
  );
  assert.match(main, /expertChatToolbarHidden = loadExpertChatToolbarHidden\(\)/);
  assert.match(
    style,
    /\.expert-chat-workspace\.is-toolbar-hidden \.expert-chat-toolbar\s*\{[^}]*height:\s*0;[^}]*visibility:\s*hidden;/,
  );
  assert.match(
    style,
    /\.expert-chat-workspace\.is-toolbar-hidden \.expert-chat-toolbar-restore\s*\{[^}]*display:\s*grid;/,
  );
});

test("les controles de pagination sont centres dans le bandeau des chats", () => {
  assert.match(
    style,
    /\.expert-chat-count\s*\{[^}]*align-items:\s*center;[^}]*justify-content:\s*center;/,
  );
  assert.match(
    style,
    /\.expert-chat-pagination > span\s*\{[^}]*align-items:\s*center;/,
  );
  assert.match(
    style,
    /\.expert-chat-toolbar-actions > \.expert-chat-count,[\s\S]*?\.expert-chat-toolbar-actions > \.expert-chat-pagination\s*\{[^}]*height:\s*38px;[^}]*min-height:\s*38px;/,
  );
});

test("les chats sont pagines sans limite et gardent leur ordre", () => {
  const chats = Array.from({ length: 100 }, (_, index) => index + 1);

  assert.equal(expertChatPageCount(chats.length, 6), 17);
  assert.deepEqual(expertChatsOnPage(chats, 0, 6), [1, 2, 3, 4, 5, 6]);
  assert.deepEqual(expertChatsOnPage(chats, 1, 6), [7, 8, 9, 10, 11, 12]);
  assert.deepEqual(expertChatsOnPage(chats, 16, 6), [97, 98, 99, 100]);

  assert.equal(expertChatPageCount(chats.length, 9), 12);
  assert.deepEqual(expertChatsOnPage(chats, 11, 9), [100]);
  assert.equal(expertChatPageCount(chats.length, 12), 9);
  assert.equal(expertChatPageCount(chats.length, 16), 7);
  assert.equal(expertChatPageCount(chats.length, 23), 5);
  assert.deepEqual(expertChatsOnPage(chats, 4, 23), chats.slice(92, 100));
  assert.equal(expertChatPageCount(chats.length, "auto"), 7);
  assert.deepEqual(expertChatsOnPage(chats, 0, "auto"), chats.slice(0, 16));
  assert.deepEqual(expertChatsOnPage(chats, 6, "auto"), [97, 98, 99, 100]);
});

test("un nouveau chat est place sur la page suivante", () => {
  assert.equal(expertChatPageForIndex(5, 6), 0);
  assert.equal(expertChatPageForIndex(6, 6), 1);
  assert.equal(expertChatPageForIndex(8, 9), 0);
  assert.equal(expertChatPageForIndex(9, 9), 1);
  assert.equal(expertChatPageForIndex(11, 12), 0);
  assert.equal(expertChatPageForIndex(12, 12), 1);
  assert.equal(expertChatPageForIndex(15, 16), 0);
  assert.equal(expertChatPageForIndex(16, 16), 1);
  assert.equal(expertChatPageForIndex(22, 23), 0);
  assert.equal(expertChatPageForIndex(23, 23), 1);
});

test("la page courante reste toujours valide", () => {
  assert.equal(clampExpertChatPage(-4, 12, 6), 0);
  assert.equal(clampExpertChatPage(8, 12, 6), 1);
  assert.equal(clampExpertChatPage(3, 0, 9), 0);
});
