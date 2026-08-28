import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  activeChatTurnBelongsToPane,
  chatMessageHasVisibleContent,
  chatMessagesEqual,
  chatPartHasVisibleContent,
  chatTextHasVisibleContent,
  chatTurnIsBusy,
  collapseRepeatedWaitParts,
  conversationWaitsForUser,
  formatChatDuration,
  formatChatResetCountdown,
  groupConsecutiveToolParts,
  reconcileChatMessages,
  shouldAdoptActiveChatTurn,
} from "../src/chat/runtime.ts";
const view = readFileSync(new URL("../src/chat/view.ts", import.meta.url), "utf8");
const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");
const chatBackend = readFileSync(new URL("../src-tauri/src/chat.rs", import.meta.url), "utf8");
const server = readFileSync(new URL("../src-tauri/src/server.rs", import.meta.url), "utf8");

test("un tour lance sur mobile retrouve son panneau sur un autre appareil", () => {
  const pane = {
    key: "chat-pane-mobile",
    accountId: "account-1",
    turnId: null,
    sessionIds: ["session-1"],
  };
  assert.equal(
    activeChatTurnBelongsToPane(
      { id: 12, accountId: "account-1", sourceChatKey: "chat-pane-mobile" },
      pane,
    ),
    true,
  );
  assert.equal(
    activeChatTurnBelongsToPane(
      { id: 13, accountId: "account-1", sessionId: "session-1" },
      { ...pane, key: "chat-pane-restored" },
    ),
    true,
    "la session persistee suffit apres un rechargement",
  );
  assert.equal(
    activeChatTurnBelongsToPane(
      { id: 14, accountId: "account-2", sessionId: "session-1" },
      pane,
    ),
    false,
    "deux comptes ne doivent jamais partager un panneau",
  );
});

test("le catalogue actif materialise les chats absents de cet appareil", () => {
  assert.match(main, /const adoptMissingActiveChatTurns = async/);
  assert.match(main, /expertChatPanes\.push\(pane\)/);
  assert.match(main, /await adoptMissingActiveChatTurns\(next\)/);
  assert.match(main, /awaitingDiscussion: !!sessionId && !discussion/);
  assert.match(main, /pane\.resumeSessionId/);
  assert.match(main, /restoreExpertChats\(\);[\s\S]*?void refreshActiveChatTurns\(\)/);
});

test("une synchronisation initiale en panne ne supprime pas les chats locaux", () => {
  assert.match(main, /let pendingExpertChatRecords: PersistedExpertChatPane\[\] = \[\]/);
  assert.match(
    main,
    /pendingExpertChatRecords\.push\(normalizedRecord\);[\s\S]*?return \[\];/,
  );
  assert.match(
    main,
    /panes: \[\.\.\.expertChatPanes\.map\([\s\S]*?\.\.\.pendingExpertChatRecords\.filter/,
  );
  assert.match(
    main,
    /const newlyAttachedPanes = attachPendingExpertChatRecords\(latestDiscussions\)/,
  );
  assert.match(
    main,
    /nextRenderSignature === discussionsRenderSignature && newlyAttachedPanes\.length === 0/,
  );
});

test("un chat restaure tardivement reconstruit le mur et conserve sa selection", () => {
  const applyStart = main.indexOf("const applyDiscussionsSnapshot =");
  const applyEnd = main.indexOf("\nconst refreshDiscussions", applyStart);
  const applySource = main.slice(applyStart, applyEnd);
  assert.match(
    applySource,
    /if \(newlyAttachedPanes\.length > 0\)[\s\S]*?render\(\);[\s\S]*?startAllExpertChatWork\(\);/,
  );

  const attachStart = main.indexOf("const attachPendingExpertChatRecords =");
  const attachEnd = main.indexOf("\nconst restoreExpertChats", attachStart);
  const attachSource = main.slice(attachStart, attachEnd);
  assert.match(
    attachSource,
    /activeExpertChatKey = pendingActiveExpertChatKey;[\s\S]*?pendingActiveExpertChatKey = null;[\s\S]*?reconcileExpertChatPage\(\);/,
  );
});

test("l'indexation d'un nouveau rollout ne bloque plus le suivi du chat", () => {
  assert.match(main, /discussionAttachPromise: Promise<boolean> \| null/);
  assert.match(main, /if \(pane\.discussionAttachPromise\) return pane\.discussionAttachPromise/);
  const applyStart = main.indexOf("const applyExpertChatTurnSnapshot = async");
  const applyEnd = main.indexOf("\nconst pollExpertChatTurn", applyStart);
  const source = main.slice(applyStart, applyEnd);
  assert.match(source, /const attachment = attachCreatedExpertChat\(pane, snapshot\.sessionId\)/);
  assert.match(source, /if \(shouldLaunchAutomaticOrchestration\) \{[\s\S]*?attached = await attachment/);
  assert.match(source, /else \{[\s\S]*?void attachment\.catch/);
  assert.doesNotMatch(source, /snapshot\.sessionId\s*\?\s*await attachCreatedExpertChat/);
});

test("le rattachement d'un rollout respecte compte et noeud", () => {
  assert.match(main, /const discussionMatchesExpertPaneSession =/);
  assert.match(main, /discussion\.accountId !== pane\.accountId/);
  assert.match(main, /discussionNode !== expectedNode/);
});

test("les contenus sans glyphe visible ne creent plus de bulle vide", () => {
  const invisible = " \n\t\u00a0\u200b\ufeff\u2800";

  assert.equal(chatTextHasVisibleContent(invisible), false);
  assert.equal(chatTextHasVisibleContent("..."), true, "la ponctuation reste un contenu volontaire");
  assert.equal(
    chatMessageHasVisibleContent({ role: "user", text: invisible, timestamp: 1 }),
    false,
  );
  assert.equal(
    chatMessageHasVisibleContent({
      role: "assistant",
      text: "",
      timestamp: 2,
      parts: [{ id: "empty", kind: "text", status: "complete", text: invisible }],
    }),
    false,
  );
  assert.equal(
    chatPartHasVisibleContent({ id: "tool", kind: "tool", status: "running" }),
    true,
    "une carte d'outil sans texte reste informative",
  );
  assert.match(view, /if \(!visible\.length\) return "";/);
  assert.match(view, /if \(!chatMessageHasVisibleContent\(message\)\) return;/);
});

test("un transcript en retard ne retire jamais le dernier message envoye", () => {
  const persisted = [
    { role: "user", text: "Premier message", timestamp: 100 },
    { role: "assistant", text: "Premiere reponse", timestamp: 101 },
  ];
  const optimistic = {
    role: "user",
    text: "Continue",
    timestamp: 200,
    deliveryState: "pending",
  };

  assert.deepEqual(
    reconcileChatMessages([...persisted, optimistic], persisted, true),
    [...persisted, optimistic],
  );

  const latestAssistant = {
    role: "assistant",
    text: "La reponse vient de se terminer.",
    timestamp: 202,
  };
  assert.deepEqual(
    reconcileChatMessages([...persisted, latestAssistant], persisted, true),
    [...persisted, latestAssistant],
  );
});

test("le badge optimiste disparait quand le serveur a persiste le message", () => {
  const pending = [{ role: "user", text: "Continue", timestamp: 200, deliveryState: "pending" }];
  const server = [{ role: "user", text: "Continue", timestamp: 201 }];
  const merged = reconcileChatMessages(pending, server, true);

  assert.equal(chatMessagesEqual(merged, server), true);
  assert.equal(merged[0].deliveryState, undefined);
});

test("un message utilisateur en attente ne recoit pas de cadre pointille", () => {
  // `chat-user-message` partage la classe d'etat `chat-msg--pending` sans avoir
  // de bordure. Un selecteur generique sur cette classe faisait donc apparaitre
  // la bordure `medium` blanche par defaut du navigateur autour de tout le tour.
  assert.doesNotMatch(style, /\.chat-app-layout \.chat-msg--pending\s*\{/);
  assert.match(style, /\.chat-app-layout \.chat-msg\.chat-msg--pending\s*\{/);
});

test("seule une interface de question structuree place le chat en attente", () => {
  assert.equal(
    conversationWaitsForUser([
      { role: "assistant", text: "Quel environnement voulez-vous utiliser ?", timestamp: 1 },
    ]),
    false,
  );
  const questionPart = {
    id: "question-1",
    kind: "tool",
    status: "running",
    tool: "request_user_input",
    title: "request_user_input",
  };
  assert.equal(
    conversationWaitsForUser([
      { role: "assistant", text: "", timestamp: 1, parts: [questionPart] },
    ]),
    true,
  );
  assert.equal(conversationWaitsForUser([], [questionPart]), true);
  assert.equal(
    conversationWaitsForUser([], [{ ...questionPart, status: "complete", output: "Option A" }]),
    false,
  );
});

test("la reponse finale sort du mode anime pendant la synchronisation", () => {
  assert.equal(chatTurnIsBusy("running"), true);
  assert.equal(chatTurnIsBusy("finalizing"), true);
  assert.equal(chatTurnIsBusy("completed"), false);
  assert.match(view, /model\.turnStatus === "finalizing"/);
  assert.match(view, /Réponse terminée, synchronisation en cours/);
  assert.match(main, /snapshot\.status === "finalizing"/);
  assert.match(chatBackend, /event_type == "turn\.completed"/);
  assert.match(chatBackend, /provider == Provider::Claude && event_type == "result"/);
  assert.match(chatBackend, /PROVIDER_EXIT_GRACE/);
});

test("la reconciliation serveur restaure un tour sans ressusciter un snapshot termine", () => {
  const running = { id: 12, status: "running", startedAt: 200 };
  assert.equal(shouldAdoptActiveChatTurn(null, running), true);
  assert.equal(
    shouldAdoptActiveChatTurn({ id: 12, status: "completed", startedAt: 200 }, running),
    false,
  );
  assert.equal(
    shouldAdoptActiveChatTurn({ id: 11, status: "completed", startedAt: 100 }, running),
    true,
  );
  assert.equal(
    shouldAdoptActiveChatTurn({ id: 0, status: "failed", startedAt: 300 }, running),
    true,
  );
  assert.equal(
    shouldAdoptActiveChatTurn(
      { id: 13, status: "running", startedAt: 300 },
      { id: 12, status: "running", startedAt: 200 },
    ),
    false,
  );
});

test("les durees et resets sont lisibles pendant un long tour", () => {
  assert.equal(formatChatDuration(9), "9 s");
  assert.equal(formatChatDuration(65), "1 min 05 s");
  assert.equal(formatChatDuration(3720), "1 h 02 min");
  assert.equal(formatChatResetCountdown(10_000, 6_400), "dans 1 h");
});

test("le chat expose le chronometre, le quota et la reponse classique", () => {
  assert.match(view, /data-chat-control="runtime"/);
  assert.match(view, /data-chat-elapsed/);
  assert.match(view, /data-chat-elapsed-value/);
  assert.match(view, /Temps écoulé depuis le début du tour/);
  assert.match(view, /Quota épuisé/);
  assert.match(view, /Votre réponse est attendue/);
  assert.match(view, /data-chat-action="focus-prompt"/);
  assert.match(main, /activeView === "limits" \|\| activeView === "chat"/);
  assert.match(main, /startedAt: Math\.floor\(Date\.now\(\) \/ 1000\)/);
  assert.match(main, /startedAt: Math\.min\(previousStartedAt, snapshot\.startedAt\)/);
  assert.equal(
    (main.match(/startedAt: Math\.min\(optimisticStartedAt, snapshot\.startedAt\)/g) ?? []).length,
    3,
  );
  assert.match(main, /reconcileChatMessages\(/);
});

test("le badge du panneau reflete le tour serveur quand pane.turn est stale (plus de « Disponible » a tort)", () => {
  // Le modele du panneau resout le tour serveur comme le bandeau lateral...
  assert.match(
    main,
    /const paneServerTurn\s*=\s*activeChatTurnForDiscussion\(activeChatTurns, discussion\)\s*\?\?\s*activeChatTurnForPaneIdentity\(activeChatTurns, pane\)/,
  );
  assert.match(main, /serverTurnStatus: paneServerTurn\?\.status \?\? null/);
  assert.match(main, /paneLocalWaitsForUser \|\| paneServerWaitsForUser/);
  // ...et le rendu du panneau traite le tour comme « En cours » des que le
  // serveur l'execute, meme sans tour local (badge plus jamais « Disponible »
  // a tort pendant qu'un tour tourne).
  assert.match(
    view,
    /model\.serverTurnStatus === "running" \|\| model\.serverTurnStatus === "finalizing"/,
  );
  assert.match(view, /chatTurnIsBusy\(model\.turnStatus\) \|\| serverBusy/);
});

test("le bandeau et la colonne de gauche portent les statuts du chat", () => {
  assert.match(view, /data-chat-control="turn-status"/);
  assert.match(view, /"En cours" : "Disponible"/);
  assert.match(view, /if \(waitingForUser\) stateLabel = "Question"/);
  assert.match(main, /turnStatus\.outerHTML = renderChatTurnStatus\(model\)/);
  assert.match(
    main,
    /renderChatSidebarStatus\(openedPane \?\? null, discussion, orchestrationMember\?\.role === "orchestrator"\)/,
  );
  assert.match(
    main,
    /renderChatSidebarStatus\(pane, pane\.discussion, orchestrationMember\?\.role === "orchestrator"\)/,
  );
  assert.match(main, /data-chat-status-pane/);
  assert.match(main, /invoke<ActiveChatTurnSummary\[]>\("list_active_chat_turns"\)/);
  assert.match(main, /activeChatTurnForDiscussion\(next, pane\.discussion\)/);
  assert.match(style, /\.chat-turn-status--idle \{[\s\S]*?background: #22c55e;/);
  assert.match(style, /\.chat-turn-status--running \{[\s\S]*?background: #f59e0b;/);
  assert.match(style, /\.chat-turn-status--question \{[\s\S]*?background: #ef4444;/);
  assert.match(style, /\.chat-side-status--idle \{[\s\S]*?background: #22c55e;/);
  assert.match(style, /\.chat-side-status--running \{[\s\S]*?background: #f59e0b;/);
  assert.match(style, /\.chat-side-status--question \{[\s\S]*?background: #ef4444;/);
  assert.match(style, /\.chat-runtime-inline--waiting \.chat-runtime-dot \{[\s\S]*?background: #ef4444;/);
});

test("la pensee visible diffuse les resumes sans exposer le raisonnement brut", () => {
  assert.match(view, /data-component="reasoning-part"/);
  assert.match(view, /chat-reasoning-markdown/);
  assert.match(main, /parts: chatTurn\?\.parts \?\? \[\]/);
  assert.match(main, /parts: pane\.turn\?\.parts \?\? \[\]/);
  assert.match(style, /\.chat-reasoning-part/);
  assert.match(chatBackend, /hide_agent_reasoning=false/);
  assert.match(chatBackend, /show_raw_agent_reasoning=false/);
  assert.doesNotMatch(chatBackend, /get\("encrypted_content"\)/);
});

test("le chat suit la timeline OpenCode au lieu de separer pensee et outils", () => {
  assert.match(view, /data-component="message-timeline"/);
  assert.match(view, /data-component="session-turn"/);
  assert.match(view, /data-component="reasoning-part"/);
  assert.match(view, /data-component="tool-part"/);
  assert.match(view, /data-component="text-part"/);
  assert.match(view, /data-component="thinking-row"/);
  assert.match(view, /groupMessagesIntoTurns/);
  assert.match(view, /renderOpenCodeParts\(model\.parts/);
  assert.match(main, /parts: chatTurn\?\.parts \?\? \[\]/);
  assert.match(main, /parts: pane\.turn\?\.parts \?\? \[\]/);
  assert.match(style, /Timeline de chat portee du modele OpenCode/);
  assert.match(style, /\.chat-user-bubble/);
  assert.doesNotMatch(
    view,
    /data-component="user-message"[^\n]*chat-msg--question/,
    "le conteneur utilisateur ne doit pas recevoir le style de question pleine largeur",
  );
  assert.match(style, /\.chat-thinking-shimmer/);
  assert.match(chatBackend, /pub struct ChatPart/);
  assert.match(chatBackend, /upsert_part/);
});

test("toutes les actions consecutives sont regroupees dans une seule liste depliante", () => {
  const command = (id) => ({
    id,
    kind: "tool",
    tool: "command",
    status: "complete",
    title: "Commande executee",
    detail: `npm run ${id}`,
  });
  const textPart = { id: "text", kind: "text", status: "complete", text: "Etape suivante" };
  const search = (id) => ({
    id,
    kind: "tool",
    tool: "search",
    status: "complete",
    title: "Recherche web",
    subtitle: id,
  });
  const edit = { id: "edit", kind: "tool", tool: "edit", status: "complete", title: "Fichiers modifies" };
  const groups = groupConsecutiveToolParts([
    command("lint"),
    search("documentation"),
    edit,
    textPart,
    search("premiere requete"),
    search("seconde requete"),
  ]);

  assert.deepEqual(groups.map((group) => group.map((part) => part.id)), [
    ["lint", "documentation", "edit"],
    ["text"],
    ["premiere requete", "seconde requete"],
  ]);
  assert.match(view, /data-tool-kind="activity-group"/);
  assert.match(view, /data-chat-action-group=/);
  assert.match(view, /renderDeferredChatActionGroup/);
  assert.match(main, /data-chat-action-content/);
  assert.match(view, /Actions effectuées par l’IA/);
  assert.match(view, /recherche\$\{count > 1 \? "s" : ""\} web/);
  assert.match(view, /<ul class="chat-action-list">/);
  assert.match(style, /\.chat-action-list/);
});

test("les details lourds des groupes d'actions ne sont construits qu'au depliage", () => {
  const start = view.indexOf("const renderOpenCodeActionGroup");
  const end = view.indexOf("const renderOpenCodeAssistantMeta", start);
  const implementation = view.slice(start, end);

  assert.ok(start >= 0 && end > start);
  assert.match(implementation, /registerDeferredChatActionGroup\(parts\)/);
  assert.match(implementation, /data-chat-action-content/);
  assert.doesNotMatch(implementation, /parts\.map/);
  assert.match(main, /document\.addEventListener\("toggle"/);
  assert.match(main, /renderDeferredChatActionGroup\(groupId\)/);
});

test("les sondages wait du meme cell_id partagent une seule carte", () => {
  const wait = (id, cellId, status = "complete", output = null) => ({
    id,
    kind: "tool",
    tool: "tool",
    status,
    title: "wait",
    detail: JSON.stringify({ cell_id: cellId, yield_time_ms: 30_000 }),
    output,
  });
  const reasoning = {
    id: "reasoning",
    kind: "reasoning",
    status: "complete",
    text: "La commande travaille encore.",
  };
  const collapsed = collapseRepeatedWaitParts([
    wait("wait-1", "109", "complete", "Toujours actif"),
    reasoning,
    wait("wait-2", "109"),
    wait("wait-3", "115"),
    wait("wait-4", "109", "running"),
  ]);

  assert.equal(collapsed.length, 3, "les resumes intermediaires ne cassent pas le regroupement");
  assert.equal(collapsed[0].id, "wait-1");
  assert.equal(collapsed[0].waitCellId, "109");
  assert.equal(collapsed[0].waitCount, 3);
  assert.equal(collapsed[0].status, "running");
  assert.equal(collapsed[0].title, "Attente de la commande");
  assert.equal(collapsed[0].subtitle, "Commande 109 · 3 vérifications");
  assert.equal(collapsed[1], reasoning);
  assert.equal(collapsed[2].waitCellId, "115");
  assert.match(view, /data-tool-kind="wait-group"/);
});

test("le bouton d'envoi OpenCode reste sombre et conserve son icone", () => {
  assert.match(main, /ArrowUp/);
  assert.match(main, /MessageCircleQuestion/);
  assert.match(style, /\.chat-app-layout \.chat-send \{[\s\S]*?appearance: none;/);
  assert.match(style, /\.chat-app-layout \.chat-send \{[\s\S]*?background: rgba\(255, 255, 255, 0\.08\);/);
  assert.doesNotMatch(style, /\.chat-app-layout \.chat-send \{[\s\S]*?linear-gradient\(180deg, #f1f1f1/);
});

test("un envoi d'image dont la reponse s'est perdue ne bascule plus le chat en « Disponible »", () => {
  // Le tour serveur est committe et lance AVANT que la reponse HTTP ne parte
  // (chat.rs). Avec une image (~27 Mo base64) la reponse peut se perdre alors que
  // l'agent tourne deja : il ne faut donc jamais fabriquer un « failed » (rendu
  // « Disponible ») sur une coupure de transport, et il faut pouvoir readopter le
  // tour orphelin d'un nouveau chat qui n'a pas encore de sessionId.

  // 1) apiAt distingue un rejet HTTP definitif (4xx/5xx) d'une coupure de transport.
  assert.match(platform, /rejection\.httpStatus = response\.status;/);

  // 2) Reconciliation par sourceChatKey, meme sans discussion ni sessionId.
  assert.match(main, /const isDefiniteChatTurnRejection =/);
  assert.match(main, /const activeChatTurnBySourceKey =/);
  assert.match(main, /turn\.sourceChatKey === pane\.key/);
  assert.match(
    main,
    /activeChatTurnForDiscussion\(next, pane\.discussion\)\s*\?\?\s*activeChatTurnForPaneIdentity\(next, pane\)/,
  );

  // 3) Le catch reconcilie au lieu de marquer « failed » sur erreur ambigue, et
  //    n'ecrase jamais un vrai tour serveur deja adopte (id != 0).
  assert.match(
    main,
    /if \(pane\.turn && pane\.turn\.id !== 0\) \{\s*\n\s*return chatTurnIsBusy\(pane\.turn\.status\);/,
  );
  assert.match(
    main,
    /if \(!isDefiniteChatTurnRejection\(error\) && pane\.turn\?\.status === "running"\)/,
  );

  // 4) Le serveur transporte le sourceChatKey du snapshot vers le catalogue actif.
  assert.match(
    chatBackend,
    /pub struct ChatTurnSnapshot \{[\s\S]*?pub source_chat_key: Option<String>,/,
  );
  assert.match(
    chatBackend,
    /pub struct ActiveChatTurnSummary \{[\s\S]*?pub source_chat_key: Option<String>,/,
  );
  assert.match(chatBackend, /source_chat_key: snapshot\.source_chat_key\.clone\(\),/);
});

test("le statut d'execution d'un chat est le meme sur tous les appareils", () => {
  // Le catalogue des tours actifs etait filtre sur le seul proprietaire
  // nominatif. Un tour lance au jeton administrateur (application de bureau),
  // par un agent autonome ou par une orchestration n'en a aucun : il s'affichait
  // « En cours » la ou il avait demarre et « Disponible » sur les autres
  // appareils. La visibilite repose desormais aussi sur l'environnement.
  assert.match(chatBackend, /workspaces: Arc<Mutex<HashMap<u64, String>>>/);
  assert.match(chatBackend, /pub\(crate\) fn active_visible_to\(/);
  assert.match(chatBackend, /pub\(crate\) fn is_visible_to\(/);
  assert.doesNotMatch(chatBackend, /fn active_for_owner\(/);
  // Le chemin complet est conserve : `display_path` ne renvoie qu'un libelle.
  assert.match(chatBackend, /workspaces\.insert\(id, path\.to_string_lossy\(\)\.to_string\(\)\)/);
  assert.match(server, /active_visible_to\(&identity\.id/);
  assert.match(server, /fn chat_workspace_is_visible\(/);
  // `chat_turn_status` et `stop_chat_turn` partagent la meme regle, sinon le
  // panneau de chat de l'autre appareil ne pourrait pas suivre ni arreter le tour.
  assert.equal((server.match(/is_visible_to\(id, &identity\.id/g) ?? []).length, 2);
  assert.doesNotMatch(server, /chat\.is_owned_by\(id, &identity\.id\)/);
});

test("une mise a jour d'outil dans un transcript declenche le rafraichissement", () => {
  const before = [{
    role: "assistant",
    text: "Je verifie.",
    timestamp: 10,
    parts: [{ id: "tool-1", kind: "tool", status: "running", title: "Tests" }],
  }];
  const after = [{
    ...before[0],
    parts: [{ id: "tool-1", kind: "tool", status: "complete", title: "Tests", output: "OK" }],
  }];
  assert.equal(chatMessagesEqual(before, after), false);
});
