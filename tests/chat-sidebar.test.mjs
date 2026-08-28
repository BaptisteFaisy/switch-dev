import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  CHAT_SIDEBAR_MAX_WIDTH,
  CHAT_SIDEBAR_MIN_WIDTH,
  DEFAULT_CHAT_SIDEBAR_PRIORITY_MODE,
  activeChatTurnForDiscussion,
  arrangeChatSidebarItems,
  chatSidebarStatus,
  chatSidebarStatusLabel,
  chatSidebarMaxWidth,
  clampChatSidebarWidth,
  defaultChatSidebarWidth,
  discussionForSession,
  groupChatSidebarItems,
  normalizeChatSidebarPriorityMode,
  orderChatSidebarDiscussions,
} from "../src/chat/sidebar.ts";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");

test("la barre de conversations utilise une largeur lisible par defaut", () => {
  assert.equal(defaultChatSidebarWidth(1440), 312);
  assert.equal(defaultChatSidebarWidth(980), 280);
});

test("la largeur redimensionnee reste dans ses bornes", () => {
  assert.equal(clampChatSidebarWidth(-20, 1440), CHAT_SIDEBAR_MIN_WIDTH);
  assert.equal(clampChatSidebarWidth(0, 1440), 0);
  assert.equal(clampChatSidebarWidth(80, 1440), 80);
  assert.equal(clampChatSidebarWidth(900, 1440), CHAT_SIDEBAR_MAX_WIDTH);
  assert.equal(clampChatSidebarWidth(318.6, 1440), 319);
});

test("la barre de conversations peut etre entierement masquee", () => {
  assert.equal(CHAT_SIDEBAR_MIN_WIDTH, 0);
  assert.equal(clampChatSidebarWidth(0, 760), 0);
});

test("des boutons explicites permettent de masquer puis restaurer la barre laterale", () => {
  assert.match(
    main,
    /id="chatSidebarCollapse"[^>]*class="icon-button chat-sidebar-collapse"[^>]*data-toggle-chat-sidebar[^>]*aria-label="Masquer la barre latérale"/,
  );
  assert.match(
    main,
    /class="icon-button chat-sidebar-expand"[^>]*data-toggle-chat-sidebar[^>]*aria-label="Afficher la barre latérale"/,
  );
  assert.match(
    style,
    /\.chat-app-layout\.is-sidebar-collapsed \.chat-sidebar-expand\s*\{[^}]*display: inline-flex;/s,
  );
  assert.match(main, /setChatSidebarWidth\(0\)/);
  assert.match(main, /setChatSidebarWidth\(defaultChatSidebarWidth\(window\.innerWidth\)\)/);
});

test("Ctrl+B masque et restaure la barre laterale via un raccourci configurable", () => {
  assert.match(main, /const toggleChatSidebar = \(\): void =>/);
  assert.match(main, /keyboardShortcutMatchesAction\("toggle-sidebar", event\)/);
  assert.match(main, /toggleChatSidebar\(\)/);
});

test("la poignee reste collee au bord gauche quand la barre est masquee", () => {
  assert.match(
    style,
    /\.chat-app-layout\.is-sidebar-collapsed \.chat-sidebar-resizer::before,\s*\.chat-app-layout\.is-sidebar-collapsed \.chat-sidebar-resizer::after\s*\{[^}]*left:\s*0;/s,
  );
});

test("la fenetre de chat garde au moins 360 pixels", () => {
  assert.equal(chatSidebarMaxWidth(760), 400);
  assert.equal(clampChatSidebarWidth(420, 760), 400);
});

test("les pastilles de la colonne de gauche suivent le statut du chat", () => {
  assert.equal(chatSidebarStatus("running", false), "running");
  assert.equal(chatSidebarStatus("running", true), "question");
  assert.equal(chatSidebarStatus("completed", true), "question");
  assert.equal(chatSidebarStatus("finalizing", true), "question");
  assert.equal(chatSidebarStatus("finalizing", false), "running");
  assert.equal(chatSidebarStatus("completed", false), "idle");
  assert.equal(
    chatSidebarStatus("completed", false, "running"),
    "running",
    "un snapshot local terminal ne doit pas masquer un tour encore actif cote serveur",
  );
  assert.equal(chatSidebarStatus("idle", false, "finalizing"), "running");
  assert.equal(chatSidebarStatusLabel("running"), "En cours");
  assert.equal(chatSidebarStatusLabel("question"), "Question");
  assert.equal(chatSidebarStatusLabel("idle"), "Disponible");
});

test("le mode Disponibles tient compte du catalogue serveur actif", () => {
  const availability = main.slice(
    main.indexOf("const expertChatPaneHasBusyTurn ="),
    main.indexOf("const pinExplicitlyOpenedBusyExpertChat ="),
  );

  assert.match(
    availability,
    /expertChatPaneIsAvailable[\s\S]*?!expertChatPaneHasBusyTurn\(pane\)/,
  );
  assert.match(
    availability,
    /activeChatTurnForDiscussion\(activeChatTurns, pane\.discussion\) !== null/,
  );
});

test("le titre semantique est affiche et rafraichi dans la colonne de gauche", () => {
  const renderer = main.slice(
    main.indexOf("const renderChatSidebarConversations ="),
    main.indexOf("const activeChatTurnsStatusSignature ="),
  );
  const snapshot = main.slice(
    main.indexOf("const applyDiscussionsSnapshot ="),
    main.indexOf("const refreshDiscussions ="),
  );

  assert.match(
    renderer,
    /const title = discussion\.title\?\.trim\(\)[\s\S]*?\|\| discussion\.preview\?\.trim\(\)[\s\S]*?\|\| "Conversation sans titre"/,
  );
  assert.match(renderer, /<strong>\$\{escapeHtml\(title\)\}<\/strong>/);
  assert.match(snapshot, /host\.innerHTML = renderChatSidebarConversations\(\)/);
  assert.match(main, /chatSidebarRefreshPending[\s\S]*requestAnimationFrame\(refreshChatSidebarConversations\)/);
});

test("le chat selectionne est distinct des autres chats simplement ouverts", () => {
  assert.match(main, /chat-side-item \$\{openedPane \? "active" : ""\} \$\{current \? "current" : ""\}/);
  assert.match(main, /current \? 'aria-current="true"' : ""/);
  assert.match(style, /\.chat-side-item\.current\s*\{[^}]*box-shadow:\s*inset 3px 0 0 var\(--chat-accent\)/s);
});

test("un tour serveur actif est retrouve apres reload et a travers un fork", () => {
  const discussion = {
    accountId: "account-a",
    sessionId: "logical-session",
    rolloutId: "latest-rollout",
  };
  const turns = [
    {
      id: 4,
      accountId: "account-b",
      sessionId: "latest-rollout",
      status: "running",
      startedAt: 40,
    },
    {
      id: 5,
      accountId: "account-a",
      sessionId: "latest-rollout",
      status: "running",
      startedAt: 50,
    },
    {
      id: 6,
      accountId: "account-a",
      sessionId: "logical-session",
      status: "completed",
      startedAt: 60,
    },
  ];

  assert.equal(activeChatTurnForDiscussion(turns, discussion)?.id, 5);
  assert.equal(activeChatTurnForDiscussion(turns, { ...discussion, accountId: "missing" }), null);
});

test("un chat orchestre s'ouvre avec son rollout courant", () => {
  const discussions = [
    {
      accountId: "account-a",
      sessionId: "logical-session",
      rolloutId: "latest-rollout",
    },
    {
      accountId: "account-b",
      sessionId: "other-session",
      rolloutId: "latest-rollout",
    },
  ];

  assert.equal(
    discussionForSession(discussions, "account-a", "logical-session"),
    discussions[0],
  );
  assert.equal(
    discussionForSession(discussions, "account-a", "latest-rollout"),
    discussions[0],
  );
  assert.equal(discussionForSession(discussions, "missing", "latest-rollout"), null);
});

test("les chats sont regroupes par orchestration avec le pilote en premier", () => {
  const items = [
    { id: "direct-a", status: "idle", orchestrationId: null, orchestrationRole: null, orchestrationPosition: 0 },
    { id: "tester-2", status: "idle", orchestrationId: "run-a", orchestrationRole: "tester", orchestrationPosition: 2 },
    { id: "worker-2", status: "running", orchestrationId: "run-a", orchestrationRole: "worker", orchestrationPosition: 2 },
    { id: "orchestrator", status: "running", orchestrationId: "run-a", orchestrationRole: "orchestrator", orchestrationPosition: 0 },
    { id: "worker-1", status: "idle", orchestrationId: "run-a", orchestrationRole: "worker", orchestrationPosition: 1 },
    { id: "direct-b", status: "question", orchestrationId: null, orchestrationRole: null, orchestrationPosition: 0 },
  ];

  const groups = groupChatSidebarItems(items);

  assert.deepEqual(groups.map(({ orchestrationId }) => orchestrationId), [null, "run-a"]);
  assert.deepEqual(groups[0].items.map(({ id }) => id), ["direct-a", "direct-b"]);
  assert.deepEqual(
    groups[1].items.map(({ id }) => id),
    ["orchestrator", "worker-1", "worker-2", "tester-2"],
  );
  assert.deepEqual(items.map(({ id }) => id), [
    "direct-a",
    "tester-2",
    "worker-2",
    "orchestrator",
    "worker-1",
    "direct-b",
  ]);
});

test("la colonne de gauche rend des blocs d'equipe et distingue l'orchestrateur", () => {
  assert.match(main, /data-sidebar-orchestration-group=/);
  assert.match(main, /id="chatSidebarGroupsTitle">Groupes de chats/);
  assert.match(main, /id="newChatGroupFromSidebar"/);
  assert.match(main, /openNewChatModal\(\{ group: true \}\)/);
  assert.match(main, /visibleGroupCount = groupChatSidebarItems\(visibleItems\)\.length/);
  assert.match(main, /Chats individuels/);
  assert.doesNotMatch(main, /Chats de cet environnement/);
  assert.match(main, /createdSubchatCount[\s\S]*plannedSubchatCount[\s\S]*sous-chat/);
  assert.match(main, /chat-side-status--orchestrator/);
  assert.match(main, /return "Orchestrateur"/);
  assert.match(style, /\.chat-sidebar-group-stack\s*\{/);
  assert.match(style, /\.chat-sidebar-new-group\s*\{/);
  assert.match(style, /\.chat-side-chat-group--orchestration\s*\{/);
  assert.match(style, /\.chat-side-status--orchestrator\s*\{[^}]*transform:\s*rotate\(45deg\)/s);
});

test("l'activite d'un chat ne change jamais sa place dans l'environnement", () => {
  const discussions = [
    { sessionId: "ancien", startedAt: 100, lastActivity: 900 },
    { sessionId: "recent-b", startedAt: 200, lastActivity: 400 },
    { sessionId: "recent-a", startedAt: 200, lastActivity: 300 },
  ];

  const initialOrder = orderChatSidebarDiscussions(discussions).map(({ sessionId }) => sessionId);
  const refreshedOrder = orderChatSidebarDiscussions([
    { ...discussions[0], lastActivity: 2_000 },
    { ...discussions[1], lastActivity: 401 },
    { ...discussions[2], lastActivity: 1_500 },
  ]).map(({ sessionId }) => sessionId);

  assert.deepEqual(initialOrder, ["recent-a", "recent-b", "ancien"]);
  assert.deepEqual(refreshedOrder, initialOrder);
  assert.deepEqual(
    discussions.map(({ sessionId }) => sessionId),
    ["ancien", "recent-b", "recent-a"],
    "le tri ne doit pas muter le snapshot recu",
  );
});

test("la priorite de la liste remonte les questions ou les chats verts de facon stable", () => {
  const items = [
    { id: "orange-a", status: "running" },
    { id: "vert-a", status: "idle" },
    { id: "question-a", status: "question" },
    { id: "vert-b", status: "idle" },
    { id: "question-b", status: "question" },
    { id: "orange-b", status: "running" },
  ];

  assert.equal(DEFAULT_CHAT_SIDEBAR_PRIORITY_MODE, "recent");
  assert.equal(normalizeChatSidebarPriorityMode("invalid"), "recent");
  assert.deepEqual(
    arrangeChatSidebarItems(items, "question", false).map(({ id }) => id),
    ["question-a", "question-b", "orange-a", "vert-a", "vert-b", "orange-b"],
  );
  assert.deepEqual(
    arrangeChatSidebarItems(items, "available", false).map(({ id }) => id),
    ["vert-a", "vert-b", "orange-a", "question-a", "question-b", "orange-b"],
  );
  assert.deepEqual(
    items.map(({ id }) => id),
    ["orange-a", "vert-a", "question-a", "vert-b", "question-b", "orange-b"],
    "le tableau source ne doit pas etre mute",
  );
});

test("les chats orange peuvent etre masques sans disparaitre de la source", () => {
  const items = [
    { id: "orange", status: "running" },
    { id: "question", status: "question" },
    { id: "vert", status: "idle" },
  ];

  assert.deepEqual(
    arrangeChatSidebarItems(items, "question", true).map(({ id }) => id),
    ["question", "vert"],
  );
  assert.equal(items.length, 3);
});

test("les preferences de tri et de masquage sont disponibles dans Parametres", () => {
  for (const marker of [
    'data-chat-sidebar-priority="question"',
    'data-chat-sidebar-priority="available"',
    'data-chat-sidebar-running="hide"',
    "CHAT_SIDEBAR_PRIORITY_STORAGE_KEY",
    "CHAT_SIDEBAR_HIDE_RUNNING_STORAGE_KEY",
    "chatSidebarPriorityMode = loadChatSidebarPriorityMode()",
    "chatSidebarHideRunning = loadChatSidebarHideRunning()",
  ]) {
    assert.ok(main.includes(marker), `preference manquante: ${marker}`);
  }
  assert.match(
    main,
    /arrangeChatSidebarItems\(\s*\[\.\.\.draftItems, \.\.\.conversationItems\],\s*chatSidebarPriorityMode,\s*chatSidebarHideRunning,/,
  );
  assert.match(
    main,
    /previousStatus !== status[\s\S]*?chatSidebarPriorityMode !== "recent" \|\| chatSidebarHideRunning[\s\S]*?refreshChatSidebarConversations\(\)/,
  );
});

test("retirer un chat depuis la colonne de gauche utilise une modale integree", () => {
  const start = main.indexOf("const openDiscussionArchiveModal =");
  const end = main.indexOf("// Compte cible retenu", start);
  assert.ok(start >= 0 && end > start, "parcours d'archivage introuvable");
  assert.doesNotMatch(main.slice(start, end), /window\.confirm/);
  assert.match(main, /id="discussionArchiveBackdrop"/);
  assert.match(main, /aria-labelledby="discussionArchiveTitle"/);
  assert.match(main, /id="cancelDiscussionArchive" data-dialog-initial-focus/);
  assert.match(main, /if \(discussion\) openDiscussionArchiveModal\(discussion\)/);
  assert.match(style, /\.discussion-archive-modal/);
});
