import assert from "node:assert/strict";
import test from "node:test";

import {
  bestQuotaAccount,
  bestQuotaAccountForNewChat,
  combinedQuotaUsage,
  deduplicateQuotaAccountsForDisplay,
  fallbackAccountForNewChat,
  isQuotaAuthenticationError,
  isQuotaExhaustionError,
  OPEN_CHAT_QUOTA_RESERVATION_PERCENT,
  quotaAfterOpenChatReservations,
  remainingQuotaPercent,
  shouldRecoverRunningQuotaTurn,
} from "../src/chat/quota.ts";
import { readFileSync } from "node:fs";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const chatView = readFileSync(new URL("../src/chat/view.ts", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");
const settingsBackend = readFileSync(new URL("../src-tauri/src/settings.rs", import.meta.url), "utf8");
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");

test("reconnait les erreurs de quota sans confondre la fenetre de contexte", () => {
  assert.equal(isQuotaExhaustionError("You've hit your usage limit. Try again later."), true);
  assert.equal(isQuotaExhaustionError("rate_limit_exceeded (HTTP 429)"), true);
  assert.equal(isQuotaExhaustionError("Quota de jetons epuise"), true);
  assert.equal(isQuotaExhaustionError("Maximum context length exceeded"), false);
  assert.equal(isQuotaExhaustionError("Le prompt est trop long"), false);
});

test("le quota restant correspond a la limite la plus contraignante", () => {
  assert.equal(
    remainingQuotaPercent({
      id: "a",
      hasTokens: true,
      sessionUsedPercent: 35,
      weeklyUsedPercent: 72,
      buckets: [],
    }),
    28,
  );
  assert.equal(
    remainingQuotaPercent({
      id: "a",
      hasTokens: true,
      sessionUsedPercent: 1,
      buckets: [{ usedPercent: 2, rateLimitReachedType: "primary" }],
    }),
    0,
  );
});

test("choisit le compte compatible ayant le plus de quota", () => {
  const accounts = [
    { id: "courant", hasTokens: true, sessionUsedPercent: 100 },
    { id: "faible", hasTokens: true, sessionUsedPercent: 75, weeklyUsedPercent: 40 },
    { id: "meilleur", hasTokens: true, sessionUsedPercent: 25, weeklyUsedPercent: 35 },
    { id: "autre-provider", hasTokens: true, sessionUsedPercent: 0, weeklyUsedPercent: 0 },
  ];

  assert.deepEqual(
    bestQuotaAccount(accounts, "courant", ["faible", "meilleur"]),
    { account: accounts[2], remainingPercent: 65 },
  );
});

test("les limites visibles sont reactivees toutes les 30 secondes", () => {
  assert.match(main, /const LIMIT_POLL_INTERVAL_MS = 30_000/);
  assert.match(
    main,
    /runWhenPageVisible\(\(\) => void refreshLimitStatus\(\)\)[\s\S]*?LIMIT_POLL_INTERVAL_MS/,
  );
});

test("ignore un quota en cache lorsque l'authentification du compte est invalide", () => {
  const accounts = [
    { id: "courant", hasTokens: true, weeklyUsedPercent: 100 },
    { id: "sans-token", hasTokens: false, weeklyUsedPercent: 0 },
    {
      id: "invalide",
      hasTokens: true,
      weeklyUsedPercent: 10,
      error: "Could not parse your authentication token. Please try signing in again. (401 Unauthorized)",
    },
    { id: "sain", hasTokens: true, weeklyUsedPercent: 60 },
  ];

  assert.equal(isQuotaAuthenticationError(accounts[2].error), true);
  assert.equal(isQuotaAuthenticationError("HTTP 403 Forbidden"), true);
  assert.equal(isQuotaAuthenticationError("temporary network timeout"), false);
  assert.equal(
    bestQuotaAccount(accounts, "courant", ["sans-token", "invalide", "sain"])?.account.id,
    "sain",
  );
  assert.equal(
    bestQuotaAccountForNewChat(accounts, ["sans-token", "invalide", "sain"], [])?.account.id,
    "sain",
  );
  assert.equal(bestQuotaAccount(accounts, "courant", ["sans-token"]), null);
  assert.equal(bestQuotaAccount(accounts, "courant", ["invalide"]), null);
});

test("les limites s'affichent depuis un cache pendant le rafraichissement serveur", () => {
  assert.match(settingsBackend, /const RATE_LIMIT_CACHE_TTL_SECS: u64 = 60/);
  assert.match(settingsBackend, /fn account_limit_views_fast\(/);
  assert.match(settingsBackend, /spawn_account_limit_refresh\(settings\.clone\(\), signature\)/);
  assert.match(main, /const LIMIT_REFRESH_FOLLOWUP_MS = 750/);
  assert.match(main, /limitStatus\.some\(\(row\) => row\.refreshing === true\)/);
  assert.match(platform, /\/api\/limits\$\{args\.force \? "\?force=true" : ""\}/);
});

test("combine l'utilisation de tous les comptes ayant un quota lisible", () => {
  assert.deepEqual(
    combinedQuotaUsage([
      { id: "leger", hasTokens: true, sessionUsedPercent: 20, weeklyUsedPercent: 10 },
      { id: "charge", hasTokens: true, sessionUsedPercent: 80, weeklyUsedPercent: 50 },
      { id: "inconnu", hasTokens: true },
      { id: "deconnecte", hasTokens: false, sessionUsedPercent: 100 },
    ]),
    {
      usedPercent: 50,
      remainingPercent: 50,
      measuredAccountCount: 2,
    },
  );
  assert.deepEqual(combinedQuotaUsage([]), {
    usedPercent: null,
    remainingPercent: null,
    measuredAccountCount: 0,
  });
});

test("affiche la grosse barre d'utilisation globale au centre du bandeau des chats", () => {
  assert.match(
    main,
    /<\/div>\s*\$\{renderExpertChatGlobalUsage\(\)\}\s*<div class="expert-chat-toolbar-actions">/,
  );
  assert.match(main, /if \(activeView === "chat"\) syncExpertChatGlobalUsageUi\(\)/);
  assert.match(
    style,
    /\.expert-chat-global-usage \{[\s\S]*?flex: 1 1 420px;[\s\S]*?max-width: 620px;/,
  );
  assert.match(style, /\.expert-chat-global-usage-track \{[\s\S]*?height: 11px;/);
});

test("reserve 20 points de quota par chat deja ouvert", () => {
  assert.equal(OPEN_CHAT_QUOTA_RESERVATION_PERCENT, 20);
  assert.deepEqual(quotaAfterOpenChatReservations(86, 2), {
    effectiveRemainingPercent: 46,
    reservedPercent: 40,
  });
  assert.deepEqual(quotaAfterOpenChatReservations(50, 3), {
    effectiveRemainingPercent: 0,
    reservedPercent: 60,
  });
});

test("le nouveau chat choisit le plus gros quota apres reservation des chats ouverts", () => {
  const accounts = [
    { id: "presque-vide", hasTokens: true, sessionUsedPercent: 80 },
    { id: "occupe", hasTokens: true, sessionUsedPercent: 10 },
    { id: "libre", hasTokens: true, sessionUsedPercent: 30 },
  ];

  assert.deepEqual(
    bestQuotaAccountForNewChat(
      accounts,
      accounts.map((account) => account.id),
      ["occupe", "occupe"],
    ),
    {
      account: accounts[2],
      remainingPercent: 70,
      effectiveRemainingPercent: 70,
      openChatCount: 0,
      reservedPercent: 0,
    },
    "deux chats retranchent 40 points au compte qui avait 90 % de quota serveur",
  );
});

test("le nouveau chat peut utiliser un compte connecte sans quota mesure", () => {
  const accounts = [
    { id: "deconnecte", hasTokens: false },
    { id: "occupe", hasTokens: true },
    { id: "libre", hasTokens: true },
    { id: "invalide", hasTokens: true, error: "401 Unauthorized" },
  ];

  assert.equal(
    fallbackAccountForNewChat(
      accounts,
      accounts.map((account) => account.id),
      ["occupe", "occupe"],
    )?.id,
    "libre",
  );
  assert.equal(
    fallbackAccountForNewChat(accounts, ["deconnecte", "invalide"], []),
    null,
  );
});

test("l'action principale route silencieusement vers le compte compatible le plus disponible", () => {
  const routingStart = main.indexOf("const confirmNewChatWithBestQuota = async");
  const routingEnd = main.indexOf("\nconst openNewChatModal =", routingStart);
  const routing = main.slice(routingStart, routingEnd);

  assert.match(main, /id="confirmNewChat"/);
  assert.doesNotMatch(main, /id="confirmBestQuotaNewChat"/);
  assert.match(main, /const confirmNewChatWithBestQuota = async/);
  assert.match(
    main,
    /compatibleAccountIds[\s\S]*?bestQuotaAccountForNewChat\([\s\S]*?fallbackAccountForNewChat/,
  );
  assert.match(routing, /selectAutomaticAccount\(compatibleAccountIds\)[\s\S]*?selectAutomaticAccount\(allAccountIds\)/);
  assert.match(routing, /if \(!automaticAccount\)[\s\S]*?Aucun agent connecté/);
  assert.match(
    main,
    /newChatRoutingMode === "automatic"[\s\S]*?void confirmNewChatWithBestQuota\(\)/,
  );
  assert.doesNotMatch(routing, /await refreshLimitStatus/);
});

test("n'affiche qu'une limite pour plusieurs profils partageant le meme home", () => {
  const rows = [
    {
      id: "copie-ancienne",
      provider: "codex",
      codexHome: "%CST_DATA_DIR%\\codex-homes\\compte",
      hasTokens: false,
      source: "unavailable",
      error: "lecture impossible",
      refreshedAt: 10,
    },
    {
      id: "principal",
      provider: "codex",
      codexHome: "%cst_data_dir%/codex-homes/compte/",
      hasTokens: true,
      source: "server",
      refreshedAt: 20,
    },
    {
      id: "claude-separe",
      provider: "claude",
      codexHome: "%CST_DATA_DIR%/codex-homes/compte",
      hasTokens: true,
      source: "authenticated",
      refreshedAt: 20,
    },
  ];

  assert.deepEqual(
    deduplicateQuotaAccountsForDisplay(rows).map((row) => row.id),
    ["principal", "claude-separe"],
  );
});

test("recupere un tour running quand son compte atteint reellement zero", () => {
  const exhausted = {
    id: "epuise",
    hasTokens: true,
    weeklyUsedPercent: 100,
    buckets: [{ rateLimitReachedType: "rate_limit_reached" }],
  };
  const available = {
    id: "disponible",
    hasTokens: true,
    weeklyUsedPercent: 72,
    buckets: [],
  };

  assert.equal(
    shouldRecoverRunningQuotaTurn(
      { id: 10, accountId: "epuise", status: "running" },
      [exhausted, available],
    ),
    true,
    "une commande sans sortie ne doit pas maintenir le tour indefiniment",
  );
  assert.equal(
    shouldRecoverRunningQuotaTurn(
      { id: 10, accountId: "disponible", status: "running" },
      [exhausted, available],
    ),
    false,
  );
  assert.equal(
    shouldRecoverRunningQuotaTurn(
      { id: 0, accountId: "epuise", status: "running" },
      [exhausted],
    ),
    false,
    "le demarrage optimiste attend encore son identifiant backend",
  );
  assert.equal(
    shouldRecoverRunningQuotaTurn(
      { id: 10, accountId: "epuise", status: "completed" },
      [exhausted],
    ),
    false,
  );
});

test("un quota epuise transfere automatiquement la discussion sans bouton", () => {
  const start = main.indexOf("const automaticallyTransferQuotaExhaustedDiscussion =");
  const end = main.indexOf("\nconst readChatPreferences", start);
  assert.notEqual(start, -1);
  assert.notEqual(end, -1);
  const automaticTransfer = main.slice(start, end);

  assert.match(automaticTransfer, /await refreshQuotaAlternatives\(\)/);
  assert.match(automaticTransfer, /quotaSuggestionFor\(currentTurn, currentDiscussion\)/);
  assert.match(
    automaticTransfer,
    /await continueDiscussionWith\(currentDiscussion, suggestion\.accountId, pane, \{[\s\S]*?preserveNavigation: true,[\s\S]*?activateTarget: pane === null,[\s\S]*?\}\)/,
  );
  assert.ok(
    (main.match(/automaticallyTransferQuotaExhaustedDiscussion\(/g) ?? []).length >= 5,
    "les echecs immediats et suivis des deux vues doivent lancer le transfert",
  );
  assert.match(chatView, /Continuité automatique/);
  assert.doesNotMatch(chatView, /Transfert automatique vers/);
  assert.doesNotMatch(chatView, /data-chat-action="quota-switch"/);
});

test("le polling de quota arrete une commande bloquee avant le transfert", () => {
  const refreshStart = main.indexOf("const refreshLimitStatus =");
  const refreshEnd = main.indexOf("\n// Ouvre un terminal interactif", refreshStart);
  const automaticStart = main.indexOf("const automaticallyTransferQuotaExhaustedDiscussion =");
  const automaticEnd = main.indexOf("\nconst readChatPreferences", automaticStart);
  assert.notEqual(refreshStart, -1);
  assert.notEqual(refreshEnd, -1);
  assert.notEqual(automaticStart, -1);
  assert.notEqual(automaticEnd, -1);

  const refresh = main.slice(refreshStart, refreshEnd);
  const automaticTransfer = main.slice(automaticStart, automaticEnd);
  assert.match(refresh, /recoverQuotaExhaustedChatTurns\(\)/);
  assert.match(automaticTransfer, /shouldRecoverRunningQuotaTurn\(currentTurn, limitStatus\)/);
  assert.match(automaticTransfer, /"stop_chat_turn"/);
  assert.ok(
    automaticTransfer.indexOf('"stop_chat_turn"') <
      automaticTransfer.indexOf("continueDiscussionWith(currentDiscussion, suggestion.accountId, pane, {"),
    "la commande source doit etre terminee avant de copier et reprendre la discussion",
  );
});

test("une relance automatique conserve le workspace et affiche la cible du main-chat", () => {
  const automaticStart = main.indexOf("const automaticallyTransferQuotaExhaustedDiscussion =");
  const automaticEnd = main.indexOf("\nconst readChatPreferences", automaticStart);
  const continuationStart = main.indexOf("const continueDiscussionWith =");
  const continuationEnd = main.indexOf("\nconst discussionHasRunningTurn", continuationStart);
  const folderStart = main.indexOf("const preserveDiscussionFolder =");
  const folderEnd = main.indexOf("\n// Quand un terminal se ferme", folderStart);
  const resumeStart = main.indexOf("const resumeDiscussionInChat =");
  const resumeEnd = main.indexOf("\nconst toggleExpertChatFullscreen", resumeStart);

  for (const position of [
    automaticStart,
    automaticEnd,
    continuationStart,
    continuationEnd,
    folderStart,
    folderEnd,
    resumeStart,
    resumeEnd,
  ]) {
    assert.notEqual(position, -1);
  }

  const automaticTransfer = main.slice(automaticStart, automaticEnd);
  const continuation = main.slice(continuationStart, continuationEnd);
  const preservedFolder = main.slice(folderStart, folderEnd);
  const resume = main.slice(resumeStart, resumeEnd);

  assert.match(automaticTransfer, /preserveNavigation: true/);
  assert.match(automaticTransfer, /activateTarget: pane === null/);
  assert.match(
    continuation,
    /options\.preserveNavigation\s*\? preserveDiscussionFolder\(discussion\)\s*: restoreDiscussionFolder\(discussion\)/,
  );
  assert.doesNotMatch(preservedFolder, /activateDiscussionFolder|setCurrentWorkspace|setChatWorkspaceFilter/);
  assert.match(
    resume,
    /const activateReusePane = !!reusePane[\s\S]*?\(!preserveNavigation \|\| activateTarget\)[\s\S]*?activeView !== "chat"/,
  );
  assert.match(
    resume,
    /if \(activateTarget\) \{\s*activeExpertChatKey = pane\.key;\s*moveExpertChatPageToPane\(pane\);\s*activeView = "chat";/,
  );
  assert.match(resume, /const targetMustBeRendered = activateTarget \|\|/);
  assert.match(resume, /const keepReusedPaneVisible = preserveNavigation/);
  assert.match(resume, /automaticQuotaResumeVisibilityPins\.add\(pane\.key\)/);
  assert.match(
    main,
    /const expertChatPaneIsAvailable = \(pane: ExpertChatPane\): boolean =>[\s\S]*?automaticQuotaResumeVisibilityPins\.has\(pane\.key\)/,
  );
  assert.match(
    main,
    /if \(!chatTurnIsBusy\(snapshot\.status\)\) \{\s*automaticQuotaResumeVisibilityPins\.delete\(pane\.key\)/,
  );
});

test("une bascule de quota conserve la source jusqu'au retrait manuel", () => {
  const detachStart = main.indexOf("const detachTransferredDiscussionSourceFromOpenViews =");
  const detachEnd = main.indexOf("\nconst syncStatusTextDom", detachStart);
  const continuationStart = main.indexOf("const continueDiscussionWith =");
  const continuationEnd = main.indexOf("\nconst discussionHasRunningTurn", continuationStart);
  const finalizationStart = main.indexOf("const finalizeTransferredDiscussion =");
  const finalizationEnd = main.indexOf("\nconst releaseTransferredDiscussion", finalizationStart);
  const releaseStart = main.indexOf("const releaseTransferredDiscussion =");
  const releaseEnd = main.indexOf("\nconst expertPaneForDiscussion", releaseStart);
  const automaticStart = main.indexOf("const automaticallyTransferQuotaExhaustedDiscussion =");
  const automaticEnd = main.indexOf("\nconst readChatPreferences", automaticStart);
  const manualArchiveStart = main.indexOf("const archiveDiscussionById =");
  const manualArchiveEnd = main.indexOf("\nconst openDiscussionArchiveModal", manualArchiveStart);
  assert.notEqual(detachStart, -1);
  assert.notEqual(detachEnd, -1);
  assert.notEqual(continuationStart, -1);
  assert.notEqual(continuationEnd, -1);
  assert.notEqual(finalizationStart, -1);
  assert.notEqual(finalizationEnd, -1);
  assert.notEqual(releaseStart, -1);
  assert.notEqual(releaseEnd, -1);
  assert.notEqual(automaticStart, -1);
  assert.notEqual(automaticEnd, -1);
  assert.notEqual(manualArchiveStart, -1);
  assert.notEqual(manualArchiveEnd, -1);

  const detach = main.slice(detachStart, detachEnd);
  const continuation = main.slice(continuationStart, continuationEnd);
  const finalization = main.slice(finalizationStart, finalizationEnd);
  const release = main.slice(releaseStart, releaseEnd);
  const automatic = main.slice(automaticStart, automaticEnd);
  const manualArchive = main.slice(manualArchiveStart, manualArchiveEnd);
  assert.match(detach, /chatDiscussion = null/);
  assert.match(detach, /chatTurn = null/);
  assert.doesNotMatch(detach, /delete_discussion|archive: true/);
  assert.doesNotMatch(finalization, /delete_discussion|archive: true/);
  assert.doesNotMatch(automatic, /delete_discussion|archive: true/);
  assert.match(release, /detachTransferredDiscussionSourceFromOpenViews\(discussion\)/);
  assert.match(manualArchive, /"delete_discussion"/);
  assert.match(manualArchive, /archive:\s*!permanentDelete/);
  assert.match(
    main,
    /return snapshot\.status !== "failed" && snapshot\.status !== "cancelled"/,
  );
});
