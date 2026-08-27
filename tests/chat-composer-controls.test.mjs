import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const view = readFileSync(new URL("../src/chat/view.ts", import.meta.url), "utf8");
const agentTools = readFileSync(new URL("../src/chat/agent-tools.ts", import.meta.url), "utf8");
const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const skillsView = readFileSync(new URL("../src/skills-view.ts", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
const chatBackend = readFileSync(new URL("../src-tauri/src/chat.rs", import.meta.url), "utf8");
const backendTools = readFileSync(new URL("../src-tauri/src/chat_tools.rs", import.meta.url), "utf8");

test("le compositeur de chat n'affiche plus de selecteur de compte", () => {
  // Le compte est route automatiquement pour la conversation ; une preference
  // facultative existe dans la modale, jamais dans la barre du compositeur.
  assert.doesNotMatch(view, /chat-account-select/);
  assert.doesNotMatch(view, /data-chat-control="account"/);
  assert.doesNotMatch(view, /id="\$\{id\("chatAccount"\)\}"/);
  // Le helper qui construisait les <option> de comptes doit disparaitre.
  assert.doesNotMatch(view, /const accountOptions = /);
});

test("plus aucun listener n'est cable sur le selecteur de compte du chat", () => {
  // Sans le <select>, les handlers de changement de compte seraient morts.
  assert.doesNotMatch(main, /#chatAccount"\)\?\.addEventListener/);
  assert.doesNotMatch(main, /\[data-chat-control='account'\]"\)\?\.addEventListener/);
});

test("le compositeur garde le selecteur d'intensite de raisonnement", () => {
  assert.match(view, /chat-effort-select/);
  assert.match(view, /data-chat-control="reasoning-effort"/);
  assert.match(view, /id="\$\{id\("chatReasoningEffort"\)\}"/);
});

test("les intensites proposees suivent le modele reel, pas une liste globale figee", () => {
  // Les <option> proviennent de model.reasoningEffortOptions (catalogue Codex du
  // modele selectionne), et le controle reste desactive quand le fournisseur ne
  // gere pas l'intensite de raisonnement.
  assert.match(view, /model\.reasoningEffortOptions\s*\n?\s*\.map\(/);
  assert.match(view, /!model\.supportsReasoningEffort/);
  // La valeur pre-selectionnee suit aussi le modele.
  assert.match(view, /option\.value === model\.selectedReasoningEffort/);
});

test("les options d'intensite du chat derivent du catalogue du modele", () => {
  // Cote main.ts, les options du chat passent par chatReasoningEffortOptions,
  // qui privilegie les efforts annonces par le catalogue du modele.
  assert.match(main, /reasoningEffortOptions: chatReasoningEffortOptions\(account, selectedModel\)/);
  assert.match(main, /chatCatalogModel\(account, model\)\?\.supportedReasoningEfforts/);
});

test("les parametres peuvent verrouiller les trois selecteurs du chat", () => {
  assert.match(main, /id="chatSelectorSettingsTitle">Sélecteurs dans les chats/);
  assert.match(main, /data-chat-composer-selectors="enabled"/);
  assert.match(main, /data-chat-composer-selectors="disabled"/);
  assert.match(
    main,
    /localStorage\.setItem\(CHAT_COMPOSER_SELECTORS_STORAGE_KEY, String\(enabled\)\)/,
  );
  assert.match(main, /composerSelectorsEnabled: chatComposerSelectorsEnabled/);
  assert.match(view, /busy \|\| !model\.composerSelectorsEnabled \? "disabled" : ""/);
  assert.match(
    view,
    /!model\.selectedAccountId \|\| !model\.composerSelectorsEnabled \? "disabled" : ""/,
  );
  assert.match(
    view,
    /!model\.supportsReasoningEffort \|\| !model\.composerSelectorsEnabled \? "disabled" : ""/,
  );
});

test("la saisie et l'envoi restent disponibles pendant que l'agent travaille", () => {
  const textareaTemplate = view.match(/<textarea id="\$\{id\("chatPrompt"\)\}"[^>]*>/)?.[0] ?? "";

  assert.ok(textareaTemplate, "le textarea du compositeur est absent");
  assert.doesNotMatch(textareaTemplate, /\bdisabled\b/);
  assert.match(view, /data-chat-action="send" type="submit"/);
  assert.match(view, /busy \? "Mettre le message en attente" : "Envoyer"/);
  assert.match(view, /const busy = chatTurnIsBusy\(model\.turnStatus\)/);
});

test("la zone de saisie reste compacte en hauteur dans chaque chat", () => {
  assert.match(
    style,
    /\.chat-app-layout \.chat-composer-box\s*\{[^}]*min-height:\s*76px;/,
  );
  assert.match(
    style,
    /\.chat-app-layout \.chat-composer textarea\s*\{[^}]*min-height:\s*36px;[^}]*max-height:\s*132px;/,
  );
  assert.match(
    style,
    /\.chat-app-layout \.chat-panel--compact:not\(\.is-fullscreen\) \.chat-composer textarea\s*\{[^}]*min-height:\s*34px;[^}]*max-height:\s*64px;[^}]*overflow-y:\s*auto;/,
  );
  assert.match(
    main,
    /const promptMaxHeight = root\.matches\("\.chat-panel--compact:not\(\.is-fullscreen\)"\) \? 64 : 132;/,
  );
  assert.match(main, /Math\.min\(prompt\.scrollHeight, promptMaxHeight\)/);
  assert.match(main, /Math\.min\(chatPrompt\.scrollHeight, 132\)/);
});

test("les messages en attente sont visibles, annulables et envoyes dans l'ordre", () => {
  assert.match(view, /class="chat-queue-state"/);
  assert.match(view, /model\.queuedCount/);
  assert.match(view, /data-chat-action="clear-queue"/);
  assert.match(main, /chatQueuedSubmissions\.push\(submission\)/);
  assert.match(main, /pane\.queuedSubmissions\.push\(submission\)/);
  assert.match(main, /const submission = chatQueuedSubmissions\.shift\(\)/);
  assert.match(main, /const submission = pane\.queuedSubmissions\.shift\(\)/);
  assert.match(main, /if \(!queuedSubmission\) \{\s*chatDraft = ""/);
  assert.match(main, /if \(!queuedSubmission\) pane\.draft = ""/);
  assert.match(
    main,
    /visiblePanes\.has\(pane\)\s*\|\|\s*pane\.queuedSubmissions\.length > 0/,
  );
});

test("les modes fixes et les skills epingles sont rendus comme des boutons accessibles", () => {
  assert.match(agentTools, /"question"/);
  assert.match(agentTools, /"proof"/);
  assert.match(agentTools, /chatSkillToolDefinition/);
  assert.match(agentTools, /tone: "skill"/);
  assert.match(agentTools, /message-circle-question/);
  assert.match(agentTools, /scan-eye/);
  assert.match(main, /chatAgentToolDefinitions/);
  assert.match(main, /chatSkillButtonIds/);
  assert.match(view, /data-chat-action="toggle-agent-tool"/);
  assert.match(view, /data-chat-tool="\$\{escapeHtml\(tool\.id\)\}"/);
  assert.match(view, /renderChatAgentTools\(model\.agentTools, model\.enabledTools\)/);
  assert.match(view, /aria-pressed="\$\{enabled\}"/);
  assert.match(style, /chat-agent-tool--question\[aria-pressed="true"\]/);
  assert.match(style, /chat-agent-tool--proof\[aria-pressed="true"\]/);
  assert.match(style, /chat-agent-tool--skill\[aria-pressed="true"\]/);
});

test("un nombre variable de boutons reste visible dans les chats compacts", () => {
  assert.match(
    style,
    /\.chat-panel--compact:not\(\.is-fullscreen\) \.chat-composer-toolbar\s*\{[^}]*flex-wrap:\s*wrap/,
  );
  assert.match(
    style,
    /\.chat-panel--compact:not\(\.is-fullscreen\) \.chat-agent-tools\s*\{[^}]*grid-template-columns:\s*repeat\(auto-fit,[^;]+;[^}]*flex:\s*1 0 100%;[^}]*max-width:\s*none;[^}]*overflow:\s*visible/,
  );
  assert.match(
    style,
    /\.chat-panel--compact:not\(\.is-fullscreen\) \.chat-agent-tool\s*\{[^}]*width:\s*100%/,
  );
});

test("chaque message capture les outils actifs et les transmet au moteur", () => {
  assert.match(main, /enabledTools: \[\.\.\.chatEnabledTools\]/);
  assert.match(main, /enabledTools: \[\.\.\.pane\.enabledTools\]/);
  assert.match(main, /agentSkills: chatAgentSkillPrompts\(chatEnabledTools\)/);
  assert.match(main, /\.\.\.chatAgentSkillPrompts\(pane\.enabledTools\)/);
  assert.match(main, /\.\.\.\(automaticOrchestration \? \[automaticOrchestrationRoutingSkill\(pane\.mode\)\] : \[\]\)/);
  assert.match(main, /agentTools: submission\.enabledTools\.filter\(isChatAgentModeId\)/);
  assert.match(main, /agentSkills: submission\.agentSkills/);
  assert.match(main, /migratePersistedChatAgentTools\(persisted\)/);
  assert.match(main, /toggleChatAgentTool\(pane\.enabledTools, toolId\)/);
  assert.match(platform, /agentTools,/);
  assert.match(platform, /agentSkills,/);
  assert.match(platform, /questionTool: agentTools\.includes\("question"\)/);
  assert.match(platform, /proofTool: agentTools\.includes\("proof"\)/);
});

test("les consignes des modes et des skills restent invisibles dans le message utilisateur", () => {
  assert.match(chatBackend, /chat_tool_instructions\(/);
  assert.match(backendTools, /request_user_input/);
  assert.match(backendTools, /capture d'écran/);
  assert.match(backendTools, /ChatAgentSkill/);
  assert.match(backendTools, /chat_skills_document/);
  assert.match(chatBackend, /TemporaryChatSkillsFile/);
  assert.match(chatBackend, /developer_instructions/);
  assert.match(chatBackend, /--append-system-prompt/);
  assert.match(main, /text: prompt/);
});

test("la vue Skills permet d'ajouter et retirer chaque bouton de toutes les fenetres", () => {
  assert.match(main, /CHAT_SKILL_BUTTONS_STORAGE_KEY/);
  assert.match(skillsView, /data-skill-chat-button/);
  assert.match(main, /toggleSkillChatButton/);
  assert.match(skillsView, /Ajouter aux chats/);
  assert.match(skillsView, /Retirer des chats/);
  assert.match(main, /persistChatSkillButtonIds/);
});
