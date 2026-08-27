// Vue conversation interactive, facon OpenCode.
//
// Module PUR (donnees -> chaines HTML) : l'etat, les appels backend et le
// binding des listeners restent dans main.ts. Le fil (#chatFeed) est patche de
// maniere ciblee pour conserver le scroll et le brouillon pendant le streaming.

import { escapeHtml, renderMarkdown } from "./markdown";
// Seule entorse a la purete du module : les cartes de confirmation Microsoft
// 365 sont le dernier garde-fou avant un envoi irreversible et doivent vivre
// dans le fil lui-meme, y compris dans les panneaux experts.
import { renderMicrosoftPendingActions } from "../microsoft";
import type { ChatImageAttachment } from "./image-attachments";
import { resolveChatTurnWindow } from "./render-window";
import { CHAT_COMPOSER_MAX_CHARS } from "./message-size";
import { chatRuntimeShowsStateLabel } from "./status-display";
import {
  type ChatAgentToolDefinition,
  type ChatAgentToolId,
} from "./agent-tools";
import {
  chatMessageHasVisibleContent,
  chatPartHasVisibleContent,
  chatTextHasVisibleContent,
  chatTurnIsBusy,
  collapseRepeatedWaitParts,
  conversationWaitsForUser,
  formatChatDuration,
  formatChatResetCountdown,
  groupConsecutiveToolParts,
  type RuntimeChatPart,
  type RuntimeChatMessage,
} from "./runtime";
import {
  chatTokenUsagePresentation,
  type ChatContextWindowUsage,
} from "./token-usage";

export type ChatRole = "user" | "assistant";
export type ChatMode = "build" | "plan" | "ask";
export type ChatTurnStatus =
  | "running"
  | "finalizing"
  | "completed"
  | "failed"
  | "cancelled"
  | "idle";

export type ChatMessage = RuntimeChatMessage;
export type ChatPart = RuntimeChatPart;

export type ChatActivity = {
  id: string;
  kind: string;
  label: string;
  detail?: string | null;
  status: string;
};

export type ChatThought = {
  id: string;
  kind: "reasoning" | "commentary" | string;
  text: string;
  status: string;
};

export type ChatAccountOption = {
  id: string;
  label: string;
  providerLabel: string;
  model: string;
};

export type ChatSelectOption = {
  value: string;
  label: string;
};

export type ChatFavoritePrompt = {
  id: string;
  title: string;
};

export type ChatQuotaSuggestion = {
  accountId: string;
  accountLabel: string;
  remainingPercent: number;
  busy: boolean;
};

export type ChatQuotaStatus = {
  state: "loading" | "available" | "low" | "exhausted" | "unavailable" | "disconnected" | "error";
  remainingPercent: number | null;
  resetAt: number | null;
  detail: string;
};

export type ChatSyncState =
  | "connecting"
  | "live"
  | "reconnecting"
  | "closed"
  | "unsupported"
  | "polling";

export type ChatTurnStatusDisplayMode = "dot" | "label";

export type ChatPanelModel = {
  title: string;
  subtitle: string;
  totalTokens: number | null;
  contextUsage: ChatContextWindowUsage | null;
  contextCompacting: boolean;
  supportsCompact: boolean;
  hasCompactableSession: boolean;
  accountLabel: string;
  providerLabel: string;
  nodeLabel: string | null;
  loading: boolean;
  error: string | null;
  truncated: boolean;
  syncState: ChatSyncState;
  messages: ChatMessage[];
  /** Nombre maximal de tours montes dans le DOM, null pour tout afficher. */
  visibleTurnLimit: number | null;
  activities: ChatActivity[];
  thoughts: ChatThought[];
  parts: ChatPart[];
  turnStatus: ChatTurnStatus;
  /** Statut du tour tel que reporte par le serveur pour cette discussion, quand
   *  il differe du tour local. `pane.turn` repasse a null a chaque changement de
   *  discussion et la reconciliation peut tarder : sans ce repli, le badge du
   *  panneau afficherait « Disponible » alors que le tour tourne encore cote
   *  serveur (le bandeau lateral, lui, consulte deja le tour serveur). */
  serverTurnStatus?: ChatTurnStatus | null;
  turnStartedAt: number | null;
  turnFinishedAt: number | null;
  turnError: string | null;
  waitingForUser: boolean;
  turnStatusDisplayMode: ChatTurnStatusDisplayMode;
  quotaStatus: ChatQuotaStatus;
  quotaSuggestion: ChatQuotaSuggestion | null;
  accounts: ChatAccountOption[];
  selectedAccountId: string;
  selectedModel: string;
  modelSuggestions: string[];
  selectedReasoningEffort: string;
  reasoningEffortOptions: ChatSelectOption[];
  supportsReasoningEffort: boolean;
  /** OpenRouter conserve une valeur vide explicite : aucun niveau force. */
  reasoningEffortAutomatic?: boolean;
  fastModeEnabled: boolean;
  supportsFastMode: boolean;
  fastModeHelp: string;
  composerSelectorsEnabled: boolean;
  favoritePrompts: ChatFavoritePrompt[];
  supportsGoals: boolean;
  agentTools: ChatAgentToolDefinition[];
  enabledTools: ChatAgentToolId[];
  mode: ChatMode;
  draft: string;
  imageAttachments: ChatImageAttachment[];
  queuedCount: number;
  newConversation: boolean;
  workspaceLabel: string;
  historyOpen: boolean;
};

export type ChatPanelRenderOptions = {
  instanceId?: string;
  paneIndex?: number;
  active?: boolean;
  compact?: boolean;
  fullscreen?: boolean;
  autonomous?: {
    role: "available" | "linked";
    label: string;
    detail: string;
    tone?: "active" | "paused" | "completed" | "warning";
    disabled?: boolean;
  };
  orchestration?: {
    role: "orchestrator" | "worker";
    label: string;
    detail: string;
    disabled?: boolean;
  };
  accountTransition?: {
    label: string;
    detail: string;
  };
};

export const chatSyncLabel = (state: ChatSyncState): string => {
  switch (state) {
    case "live":
      return "En direct";
    case "connecting":
      return "Connexion…";
    case "reconnecting":
      return "Reconnexion…";
    case "polling":
    case "unsupported":
      return "Actualisation auto";
    default:
      return "Local";
  }
};

const timeLabel = (timestamp: number): string => {
  const date = new Date(timestamp * 1000);
  const time = date.toLocaleTimeString("fr-FR", { hour: "2-digit", minute: "2-digit" });
  if (date.toDateString() === new Date().toDateString()) return time;
  return `${date.toLocaleDateString("fr-FR", { day: "2-digit", month: "2-digit" })} ${time}`;
};

const renderMessage = (
  message: ChatMessage,
  providerLabel: string,
  index: number,
  showIdentity: boolean,
  highlightQuestion: boolean,
  instanceId = "",
): string => {
  if (!chatTextHasVisibleContent(message.text)) return "";
  return `
  <article id="${instanceId ? `chat-message-${instanceId}-${index}` : `chat-message-${index}`}" data-chat-message-index="${index}" class="chat-msg chat-msg--${message.role} ${message.deliveryState ? `chat-msg--${message.deliveryState}` : ""} ${highlightQuestion ? "chat-msg--question" : ""}">
    ${showIdentity
      ? `<div class="chat-msg-meta">
          <span class="chat-msg-avatar ${message.role}">${message.role === "user" ? "V" : "S"}</span>
          <span class="chat-msg-role">${message.role === "user" ? "Vous" : escapeHtml(providerLabel || "Assistant")}</span>
          ${highlightQuestion ? `<span class="chat-msg-question-badge">Question</span>` : ""}
          ${message.deliveryState === "pending" ? `<span class="chat-msg-delivery chat-msg-delivery--pending">Synchronisation…</span>` : ""}
          ${message.deliveryState === "failed" ? `<span class="chat-msg-delivery chat-msg-delivery--failed">Non synchronisé</span>` : ""}
          ${message.timestamp ? `<span class="chat-msg-time">${escapeHtml(timeLabel(message.timestamp))}</span>` : ""}
        </div>`
      : ""}
    <div class="chat-msg-body">${renderMarkdown(message.text)}</div>
  </article>`;
};

const activityIcon = (kind: string): string => {
  switch (kind) {
    case "edit":
      return "save";
    case "search":
      return "search";
    case "plan":
      return "list-checks";
    case "tool":
      return "wrench";
    case "command":
      return "play";
    case "wait":
      return "hourglass";
    default:
      return "sparkles";
  }
};

const renderActivities = (activities: ChatActivity[]): string => {
  if (!activities.length) return "";
  return `<div class="chat-activities" aria-label="Activites de l'agent">
    ${activities
      .map(
        (activity) => `
        <div class="chat-activity chat-activity--${escapeHtml(activity.status)}">
          <span class="chat-activity-icon"><i data-lucide="${activityIcon(activity.kind)}"></i></span>
          <span class="chat-activity-copy">
            <strong>${escapeHtml(activity.label)}</strong>
            ${activity.detail ? `<small>${escapeHtml(activity.detail)}</small>` : ""}
          </span>
          <span class="chat-activity-state" aria-hidden="true"></span>
        </div>`,
      )
      .join("")}
  </div>`;
};

const renderThoughtStream = (
  thoughts: ChatThought[],
  turnStatus: ChatTurnStatus,
): string => {
  const live = turnStatus === "running";
  if (!live && !thoughts.length) return "";
  const visibleThoughts = thoughts.length
    ? thoughts
    : [
        {
          id: "thinking-placeholder",
          kind: "reasoning",
          text: "L’agent travaille dans l'environnement et organise la prochaine étape…",
          status: "running",
        },
      ];
  return `<details data-chat-control="thinking" class="chat-thought-stream ${live ? "is-live" : "is-complete"}" ${live ? "open" : ""}>
    <summary>
      <span class="chat-thought-stream-icon"><i data-lucide="brain-circuit"></i></span>
      <span class="chat-thought-stream-title">
        <strong>${live ? "Pensée en direct" : "Réflexion du dernier tour"}</strong>
        <small>${live ? "Résumés et progression de l’agent" : `${visibleThoughts.length} étape(s) conservée(s)`}</small>
      </span>
      <span class="chat-thought-stream-state" aria-hidden="true"></span>
      <i class="chat-thought-stream-chevron" data-lucide="chevron-down"></i>
    </summary>
    <div class="chat-thought-list" aria-live="polite">
      ${visibleThoughts
        .map(
          (thought) => `<article class="chat-thought chat-thought--${escapeHtml(thought.kind)} chat-thought--${escapeHtml(thought.status)}">
            <span class="chat-thought-icon"><i data-lucide="${thought.kind === "commentary" ? "message-square-text" : "sparkles"}"></i></span>
            <div class="chat-thought-copy">
              <small>${thought.kind === "commentary" ? "Progression" : "Résumé de réflexion"}</small>
              <div>${renderMarkdown(thought.text)}</div>
            </div>
            <span class="chat-thought-state" aria-hidden="true"></span>
          </article>`,
        )
        .join("")}
    </div>
  </details>`;
};

const renderOpenCodeCopyAction = (label: string): string => `
  <button type="button" class="chat-part-copy" data-chat-copy title="${escapeHtml(label)}" aria-label="${escapeHtml(label)}">
    <i data-lucide="copy"></i>
  </button>`;

const renderOpenCodeUserMessage = (
  message: ChatMessage,
  index: number,
  instanceId: string,
): string => {
  if (!chatTextHasVisibleContent(message.text)) return "";
  return `
  <article id="${instanceId ? `chat-message-${instanceId}-${index}` : `chat-message-${index}`}" data-chat-message-index="${index}" data-component="user-message" class="chat-user-message ${message.deliveryState ? `chat-msg--${message.deliveryState}` : ""}">
    <div class="chat-user-bubble" data-chat-copy-source>
      <div class="chat-msg-body">${renderMarkdown(message.text)}</div>
    </div>
    <div class="chat-message-actions">
      ${message.deliveryState === "pending" ? `<span class="chat-msg-delivery chat-msg-delivery--pending">Synchronisation...</span>` : ""}
      ${message.deliveryState === "failed" ? `<span class="chat-msg-delivery chat-msg-delivery--failed">Non synchronise</span>` : ""}
      ${message.timestamp ? `<time>${escapeHtml(timeLabel(message.timestamp))}</time>` : ""}
      ${renderOpenCodeCopyAction("Copier le message")}
    </div>
  </article>`;
};

const renderOpenCodeToolOutput = (part: ChatPart): string => {
  const sections = [
    part.detail ? `<section><span>Entree</span><pre>${escapeHtml(part.detail)}</pre></section>` : "",
    part.output ? `<section><span>Sortie</span><pre>${escapeHtml(part.output)}</pre></section>` : "",
  ].filter(Boolean);
  return sections.length ? `<div class="chat-tool-details">${sections.join("")}</div>` : "";
};

const renderOpenCodeToolPart = (part: ChatPart): string => {
  const details = renderOpenCodeToolOutput(part);
  const running = part.status === "running" || part.status === "queued";
  const failed = part.status === "error" || part.status === "failed";
  const waitGroup = !!part.waitCount;
  const attributes = waitGroup ? ` data-tool-kind="wait-group" data-wait-cell-id="${escapeHtml(part.waitCellId || "")}"` : "";
  const trigger = `
    <span class="chat-tool-icon" aria-hidden="true">${running ? `<span class="chat-tool-spinner"></span>` : `<i data-lucide="${failed ? "circle-alert" : activityIcon(waitGroup ? "wait" : part.tool || "tool")}"></i>`}</span>
    <span class="chat-tool-copy"><strong>${escapeHtml(part.title || "Outil")}</strong>${part.subtitle ? `<small>${escapeHtml(part.subtitle)}</small>` : ""}</span>
    ${details ? `<i class="chat-tool-chevron" data-lucide="chevron-down"></i>` : ""}`;
  if (!details) {
    return `<div data-component="tool-part"${attributes} class="chat-tool-part ${waitGroup ? "chat-wait-group " : ""}chat-tool-part--${escapeHtml(part.status)}">${trigger}</div>`;
  }
  return `<details data-component="tool-part"${attributes} class="chat-tool-part ${waitGroup ? "chat-wait-group " : ""}chat-tool-part--${escapeHtml(part.status)}" ${running ? "open" : ""}>
    <summary>${trigger}</summary>
    ${details}
  </details>`;
};

const actionGroupSummary = (parts: ChatPart[]): string => {
  const kind = parts.every((part) => part.tool === parts[0]?.tool)
    ? parts[0]?.tool
    : null;
  const count = parts.length;
  switch (kind) {
    case "command":
      return `${count} commande${count > 1 ? "s" : ""}`;
    case "search":
      return `${count} recherche${count > 1 ? "s" : ""} web`;
    case "edit":
      return `${count} modification${count > 1 ? "s" : ""}`;
    case "plan":
      return `${count} mise${count > 1 ? "s" : ""} à jour du plan`;
    default:
      return `${count} action${count > 1 ? "s" : ""}`;
  }
};

// Les tours d'agent peuvent contenir plusieurs centaines d'actions. Les
// injecter dans le DOM alors que leur <details> est ferme faisait exploser le
// cout de chaque render global (sorties <pre>, lignes et icones comprises).
// On ne conserve ici que les references aux parts ; le HTML detaille est cree
// au premier depliage par le listener delegue de main.ts.
const DEFERRED_CHAT_ACTION_GROUP_LIMIT = 4096;
const deferredChatActionGroups = new Map<string, ChatPart[]>();
let deferredChatActionGroupSequence = 0;

const registerDeferredChatActionGroup = (parts: ChatPart[]): string => {
  deferredChatActionGroupSequence += 1;
  const id = `chat-action-group-${deferredChatActionGroupSequence}`;
  deferredChatActionGroups.set(id, parts);
  while (deferredChatActionGroups.size > DEFERRED_CHAT_ACTION_GROUP_LIMIT) {
    const oldest = deferredChatActionGroups.keys().next().value as string | undefined;
    if (!oldest) break;
    deferredChatActionGroups.delete(oldest);
  }
  return id;
};

const renderOpenCodeActionItem = (part: ChatPart): string => {
  const partRunning = part.status === "running" || part.status === "queued";
  const partFailed = part.status === "error" || part.status === "failed";
  const details = renderOpenCodeToolOutput(part);
  const row = `
    <span class="chat-action-state" aria-hidden="true">
      ${partRunning ? `<span class="chat-tool-spinner"></span>` : `<i data-lucide="${partFailed ? "circle-alert" : activityIcon(part.waitCount ? "wait" : part.tool || "tool")}"></i>`}
    </span>
    <span class="chat-action-copy">
      <strong>${escapeHtml(part.title || "Outil")}</strong>
      ${part.subtitle ? `<small>${escapeHtml(part.subtitle)}</small>` : ""}
    </span>`;
  if (!details) {
    return `<li class="chat-action-item chat-action-item--${escapeHtml(part.status)}"><div class="chat-action-row">${row}</div></li>`;
  }
  return `<li class="chat-action-item chat-action-item--${escapeHtml(part.status)}">
    <details>
      <summary class="chat-action-row">${row}<i class="chat-action-chevron" data-lucide="chevron-down"></i></summary>
      ${details}
    </details>
  </li>`;
};

const renderOpenCodeActionList = (parts: ChatPart[]): string =>
  `<ul class="chat-action-list">${parts.map(renderOpenCodeActionItem).join("")}</ul>`;

export const renderDeferredChatActionGroup = (
  id: string | null | undefined,
): string | null => {
  if (!id) return null;
  const parts = deferredChatActionGroups.get(id);
  if (!parts) return null;
  deferredChatActionGroups.delete(id);
  return renderOpenCodeActionList(parts);
};

export const pruneDeferredChatActionGroups = (activeIds: ReadonlySet<string>): void => {
  for (const id of deferredChatActionGroups.keys()) {
    if (!activeIds.has(id)) deferredChatActionGroups.delete(id);
  }
};

const renderChatAgentTools = (
  tools: readonly ChatAgentToolDefinition[],
  enabledTools: readonly ChatAgentToolId[],
): string =>
  tools.map((tool) => {
    const enabled = enabledTools.includes(tool.id);
    const action = enabled ? `Désactiver ${tool.label}` : `Activer ${tool.label}`;
    return `<button
      type="button"
      class="chat-agent-tool chat-agent-tool--${tool.tone} ${enabled ? "active" : ""}"
      data-chat-action="toggle-agent-tool"
      data-chat-tool="${escapeHtml(tool.id)}"
      title="${escapeHtml(enabled ? tool.activeTitle : tool.inactiveTitle)}"
      aria-label="${escapeHtml(action)}"
      aria-pressed="${enabled}"
    >
      <i data-lucide="${escapeHtml(tool.icon)}"></i><span>${escapeHtml(tool.label)}</span>
    </button>`;
  }).join("");

const renderOpenCodeActionGroup = (parts: ChatPart[]): string => {
  const running = parts.some((part) => part.status === "running" || part.status === "queued");
  const failed = parts.some((part) => part.status === "error" || part.status === "failed");
  const status = failed ? "error" : running ? "running" : "complete";
  const title = failed
    ? "Actions de l’IA à vérifier"
    : running
      ? "Actions de l’IA en cours"
      : "Actions effectuées par l’IA";
  const trigger = `
    <span class="chat-tool-icon" aria-hidden="true">${running ? `<span class="chat-tool-spinner"></span>` : `<i data-lucide="${failed ? "circle-alert" : "list-checks"}"></i>`}</span>
    <span class="chat-tool-copy"><strong>${title}</strong><small>${actionGroupSummary(parts)}</small></span>
    <i class="chat-tool-chevron" data-lucide="chevron-down"></i>`;
  const deferredGroupId = registerDeferredChatActionGroup(parts);

  return `<details data-component="tool-part" data-tool-kind="activity-group" data-chat-action-group="${deferredGroupId}" class="chat-tool-part chat-action-group chat-tool-part--${status}">
    <summary>${trigger}</summary>
    <div class="chat-action-deferred" data-chat-action-content></div>
  </details>`;
};

const renderOpenCodeAssistantMeta = (
  providerLabel: string,
  timestamp: number,
  startedAt: number | null,
  finishedAt: number | null,
): string => {
  const duration = startedAt
    ? formatChatDuration(Math.max(0, (finishedAt || Date.now() / 1000) - startedAt))
    : "";
  return `<div class="chat-assistant-meta">
    <span>${escapeHtml(providerLabel || "Assistant")}</span>
    ${timestamp ? `<time>${escapeHtml(timeLabel(timestamp))}</time>` : ""}
    ${duration ? `<span>${escapeHtml(duration)}</span>` : ""}
  </div>`;
};

const renderOpenCodePart = (
  part: ChatPart,
  options: {
    finalText: boolean;
    providerLabel: string;
    timestamp: number;
    startedAt: number | null;
    finishedAt: number | null;
  },
): string => {
  if (part.kind === "tool") return renderOpenCodeToolPart(part);
  const text = part.text ?? "";
  if (!chatTextHasVisibleContent(text)) return "";
  if (part.kind === "reasoning") {
    return `<div data-component="reasoning-part" class="chat-reasoning-part chat-part--${escapeHtml(part.status)}">
      <div class="chat-reasoning-markdown">${renderMarkdown(text)}</div>
    </div>`;
  }
  return `<article data-component="text-part" class="chat-text-part chat-part--${escapeHtml(part.status)}" data-chat-copy-source>
    <div class="chat-assistant-markdown">${renderMarkdown(text)}</div>
    ${options.finalText ? `<div class="chat-text-actions">${renderOpenCodeAssistantMeta(options.providerLabel, options.timestamp, options.startedAt, options.finishedAt)}${renderOpenCodeCopyAction("Copier la reponse")}</div>` : ""}
  </article>`;
};

const renderOpenCodeParts = (
  parts: ChatPart[],
  providerLabel: string,
  timestamp = 0,
  startedAt: number | null = null,
  finishedAt: number | null = null,
): string => {
  const visible = collapseRepeatedWaitParts(
    parts.filter(chatPartHasVisibleContent),
  );
  if (!visible.length) return "";
  const groups = groupConsecutiveToolParts(visible);
  const lastTextIndex = groups
    .map((group) => group.length === 1 && group[0]?.kind === "text")
    .lastIndexOf(true);
  return `<div data-component="assistant-parts" class="chat-assistant-parts">
    ${groups.map((group, index) => group.length > 1
      ? renderOpenCodeActionGroup(group)
      : renderOpenCodePart(group[0], {
          finalText: index === lastTextIndex,
          providerLabel,
          timestamp,
          startedAt,
          finishedAt,
        })).join("")}
  </div>`;
};

const renderOpenCodeThinking = (model: ChatPanelModel): string => {
  if (model.turnStatus !== "running" || model.parts.some(chatPartHasVisibleContent)) return "";
  return `<div data-component="thinking-row" class="chat-thinking-row" aria-live="polite">
    <span class="chat-thinking-shimmer">Thinking</span>
  </div>`;
};

type RenderTurn = {
  user: { message: ChatMessage; index: number } | null;
  assistants: Array<{ message: ChatMessage; index: number }>;
};

const groupMessagesIntoTurns = (messages: ChatMessage[]): RenderTurn[] => {
  const turns: RenderTurn[] = [];
  messages.forEach((message, index) => {
    if (!chatMessageHasVisibleContent(message)) return;
    if (message.role === "user") {
      turns.push({ user: { message, index }, assistants: [] });
      return;
    }
    if (!turns.length) turns.push({ user: null, assistants: [] });
    turns[turns.length - 1].assistants.push({ message, index });
  });
  return turns;
};

const chatTurns = (model: ChatPanelModel): RenderTurn[] => {
  const turns = groupMessagesIntoTurns(model.messages);
  if (!turns.length && (model.parts.some(chatPartHasVisibleContent) || model.turnStatus === "running")) {
    turns.push({ user: null, assistants: [] });
  }
  return turns;
};

const openCodeMessageParts = (message: ChatMessage): ChatPart[] => {
  const parts = message.parts ?? [];
  if (parts.some(chatPartHasVisibleContent)) return parts;
  return chatTextHasVisibleContent(message.text)
    ? [{
        id: `message-${message.timestamp}`,
        kind: "text",
        status: "complete",
        text: message.text,
      }]
    : [];
};

const renderOpenCodeTurn = (
  turn: RenderTurn,
  turnIndex: number,
  turnCount: number,
  model: ChatPanelModel,
  instanceId: string,
): string => {
  const lastTurn = turnIndex === turnCount - 1;
  const hasLiveParts = model.parts.some(chatPartHasVisibleContent);
  const livePartsTakeOver = lastTurn && hasLiveParts && model.turnStatus !== "completed";
  const assistantMessages = livePartsTakeOver ? [] : turn.assistants;
  const assistant = assistantMessages.map(({ message }, assistantIndex) => {
    const lastAssistant = assistantIndex === assistantMessages.length - 1;
    const currentCompletedTurn = lastTurn && model.turnStatus === "completed" && lastAssistant;
    return renderOpenCodeParts(
      openCodeMessageParts(message),
      model.providerLabel,
      message.timestamp,
      currentCompletedTurn ? model.turnStartedAt : null,
      currentCompletedTurn ? model.turnFinishedAt : null,
    );
  }).join("");
  const live = lastTurn && hasLiveParts && (livePartsTakeOver || !assistantMessages.length)
    ? renderOpenCodeParts(model.parts, model.providerLabel, 0, model.turnStartedAt, model.turnFinishedAt)
    : "";
  return `<section data-component="session-turn" class="chat-turn ${lastTurn ? "is-latest" : ""}">
    ${turn.user ? renderOpenCodeUserMessage(turn.user.message, turn.user.index, instanceId) : ""}
    ${assistant}
    ${live}
    ${lastTurn ? renderOpenCodeThinking(model) : ""}
  </section>`;
};

/** Rend uniquement le tour vivant afin que le streaming ne reconstruise pas
 * tout l'historique de la conversation a chaque evenement. */
export const renderChatLatestTurn = (
  model: ChatPanelModel,
  instanceId = "",
): string => {
  const turns = chatTurns(model);
  const latest = turns.at(-1);
  return latest
    ? renderOpenCodeTurn(latest, turns.length - 1, turns.length, model, instanceId)
    : "";
};

const quotaStatusLabel = (quota: ChatQuotaStatus): string => {
  switch (quota.state) {
    case "loading":
      return "Lecture du quota…";
    case "disconnected":
      return "Compte non connecté";
    case "exhausted":
      return "Quota épuisé";
    case "unavailable":
      return "Quota non exposé";
    case "error":
      return "Quota indisponible";
    default:
      return `Quota ${Math.round(quota.remainingPercent ?? 0)} %`;
  }
};

const renderLegacyChatRuntimeStatus = (model: ChatPanelModel): string => {
  const waitingForUser = model.waitingForUser;
  const runningActivity = [...model.activities]
    .reverse()
    .find((activity) => activity.status === "running" || activity.status === "queued");
  const latestActivity = model.activities[model.activities.length - 1];
  let state = model.turnStatus;
  let title = "Prêt pour votre message";
  let detail = "L'agent attend vos instructions.";
  let icon = "circle";

  if (waitingForUser) {
    state = "idle";
    title = "Votre réponse est attendue";
    detail = "L'assistant vous a posé une question dans son dernier message.";
    icon = "message-circle-question";
  } else if (model.turnStatus === "running") {
    title = `${model.providerLabel || "L'agent"} travaille`;
    detail = runningActivity?.label || latestActivity?.label || "Analyse et préparation de la réponse";
    icon = "loader-circle";
  } else if (model.turnStatus === "finalizing") {
    title = "Réponse terminée";
    detail = "Synchronisation de la conversation…";
    icon = "circle-check";
  } else if (model.turnStatus === "completed") {
    title = "Réponse terminée";
    detail = latestActivity?.label || "Le dernier tour est terminé.";
    icon = "circle-check";
  } else if (model.turnStatus === "failed") {
    title = "La réponse a échoué";
    detail = model.turnError || "Consultez l'erreur affichée dans le fil.";
    icon = "triangle-alert";
  } else if (model.turnStatus === "cancelled") {
    title = "Réponse arrêtée";
    detail = "L'exécution a été interrompue.";
    icon = "square";
  }

  const startedAt = model.turnStartedAt ?? 0;
  const finishedAt = model.turnFinishedAt ?? 0;
  const duration = startedAt
    ? formatChatDuration(Math.max(0, (finishedAt || Date.now() / 1000) - startedAt))
    : "";
  const durationLabel = model.turnStatus === "running" ? "Tour" : "Total";
  const quota = model.quotaStatus;
  const quotaPercent = quota.remainingPercent === null
    ? null
    : Math.max(0, Math.min(100, quota.remainingPercent));
  const quotaReset = quota.resetAt ? formatChatResetCountdown(quota.resetAt) : "";
  const quotaTitle = [quota.detail, quotaReset ? `Réinitialisation ${quotaReset}` : ""]
    .filter(Boolean)
    .join(" · ");

  return `<section data-chat-control="runtime" class="chat-runtime chat-runtime--${waitingForUser ? "waiting" : escapeHtml(state)}" aria-live="polite">
    <span class="chat-runtime-icon" aria-hidden="true"><i data-lucide="${icon}"></i></span>
    <span class="chat-runtime-copy">
      <strong>${escapeHtml(title)}</strong>
      <small>${escapeHtml(detail)}</small>
    </span>
    <span class="chat-runtime-metrics">
      ${startedAt ? `<span class="chat-runtime-elapsed" data-chat-elapsed data-chat-started-at="${startedAt}" ${finishedAt ? `data-chat-finished-at="${finishedAt}"` : ""} title="Temps écoulé depuis le début du tour"><i data-lucide="clock-3"></i><span class="chat-runtime-elapsed-label">${durationLabel}</span><strong data-chat-elapsed-value>${escapeHtml(duration)}</strong></span>` : ""}
      <span class="chat-runtime-quota chat-runtime-quota--${escapeHtml(quota.state)}" title="${escapeHtml(quotaTitle)}">
        <i data-lucide="gauge"></i>
        <span>${escapeHtml(quotaStatusLabel(quota))}</span>
        ${quotaPercent === null ? "" : `<span class="chat-runtime-quota-meter" aria-hidden="true"><span style="width:${quotaPercent}%"></span></span>`}
        ${quota.resetAt ? `<small data-chat-reset data-chat-reset-at="${quota.resetAt}">${escapeHtml(quotaReset)}</small>` : ""}
      </span>
    </span>
    ${waitingForUser ? `<button type="button" class="tool-button primary chat-runtime-reply" data-chat-action="focus-prompt"><i data-lucide="reply"></i><span>Répondre</span></button>` : ""}
  </section>`;
};

export const renderChatRuntimeStatus = (model: ChatPanelModel): string => {
  const waitingForUser = model.waitingForUser;
  // Replie sur le tour serveur quand le tour local est absent/stale, pour ne pas
  // afficher un runtime « au repos » alors que le serveur execute encore le tour.
  const serverBusy =
    model.serverTurnStatus === "running" || model.serverTurnStatus === "finalizing";
  const effectiveStatus: ChatTurnStatus = chatTurnIsBusy(model.turnStatus)
    ? model.turnStatus
    : serverBusy
      ? (model.serverTurnStatus as ChatTurnStatus)
      : model.turnStatus;
  const showStateLabel = chatRuntimeShowsStateLabel(effectiveStatus, waitingForUser);
  const runningActivity = [...model.activities]
    .reverse()
    .find((activity) => activity.status === "running" || activity.status === "queued");
  const latestPart = model.parts[model.parts.length - 1];
  const startedAt = model.turnStartedAt ?? 0;
  const finishedAt = model.turnFinishedAt ?? 0;
  const duration = startedAt
    ? formatChatDuration(Math.max(0, (finishedAt || Date.now() / 1000) - startedAt))
    : "";
  const quota = model.quotaStatus;
  const quotaPercent = quota.remainingPercent === null
    ? null
    : Math.max(0, Math.min(100, quota.remainingPercent));
  const quotaReset = quota.resetAt ? formatChatResetCountdown(quota.resetAt) : "";
  const quotaTitle = [quota.detail, quotaReset ? `Reinitialisation ${quotaReset}` : ""]
    .filter(Boolean)
    .join(" - ");

  let stateLabel: string | null = null;
  if (waitingForUser) stateLabel = "Question";
  else if (effectiveStatus === "running") {
    stateLabel = latestPart?.title || runningActivity?.label || "Thinking";
  } else if (effectiveStatus === "finalizing") {
    stateLabel = "Réponse terminée, synchronisation en cours";
  }
  else if (effectiveStatus === "failed") stateLabel = "Echec";
  else if (effectiveStatus === "cancelled") stateLabel = "Arrete";

  return `<div data-chat-control="runtime" class="chat-runtime-inline chat-runtime-inline--${waitingForUser ? "waiting" : escapeHtml(effectiveStatus)}" aria-live="polite">
    ${showStateLabel && stateLabel ? `<span class="chat-runtime-state" title="${escapeHtml(stateLabel)}">
      <span class="chat-runtime-dot" aria-hidden="true"></span>
      <span>${escapeHtml(stateLabel)}</span>
    </span>` : ""}
    ${startedAt ? `<span class="chat-runtime-elapsed" data-chat-elapsed data-chat-started-at="${startedAt}" ${finishedAt ? `data-chat-finished-at="${finishedAt}"` : ""} title="Temps ecoule depuis le debut du tour"><i data-lucide="clock-3"></i><strong data-chat-elapsed-value>${escapeHtml(duration)}</strong></span>` : ""}
    <span class="chat-runtime-quota chat-runtime-quota--${escapeHtml(quota.state)}" title="${escapeHtml(quotaTitle)}">
      <i data-lucide="gauge"></i>
      <span>${escapeHtml(quotaStatusLabel(quota))}</span>
      ${quotaPercent === null ? "" : `<span class="chat-runtime-quota-meter" aria-hidden="true"><span style="width:${quotaPercent}%"></span></span>`}
      ${quota.resetAt ? `<small data-chat-reset data-chat-reset-at="${quota.resetAt}">${escapeHtml(quotaReset)}</small>` : ""}
    </span>
    ${waitingForUser ? `<button type="button" class="chat-runtime-reply" data-chat-action="focus-prompt"><i data-lucide="reply"></i><span>Repondre</span></button>` : ""}
  </div>`;
};

export const renderChatTurnParts = (
  parts: ChatPart[],
  providerLabel: string,
  startedAt: number | null = null,
  finishedAt: number | null = null,
): string => renderOpenCodeParts(parts, providerLabel, 0, startedAt, finishedAt);

export const renderChatTurnStatus = (model: ChatPanelModel): string => {
  const serverBusy =
    model.serverTurnStatus === "running" || model.serverTurnStatus === "finalizing";
  const state = model.waitingForUser
    ? "question"
    : chatTurnIsBusy(model.turnStatus) || serverBusy
      ? "running"
      : "idle";
  const label = state === "question" ? "Question" : state === "running" ? "En cours" : "Disponible";
  const title = state === "question"
    ? "Une interface de question attend votre reponse"
    : state === "running"
      ? "Le chat est en cours d'execution"
      : "Le chat est disponible";
  const displayMode = model.turnStatusDisplayMode === "dot" ? "dot" : "label";
  const content = displayMode === "dot"
    ? '<span class="chat-turn-status-dot" aria-hidden="true"></span>'
    : `<span class="chat-turn-status-label">${label}</span>`;
  return `<span data-chat-control="turn-status" data-chat-status-display="${displayMode}" class="chat-turn-status chat-turn-status--${state} chat-turn-status--${displayMode}" role="status" title="${title}" aria-label="${title}">
    ${content}
  </span>`;
};

export const renderChatTokenUsage = (
  model: ChatPanelModel,
  blocked = false,
): string => {
  const usage = chatTokenUsagePresentation(model.totalTokens, model.contextUsage);
  const canCompact = model.supportsCompact
    && model.hasCompactableSession
    && !model.contextCompacting
    && model.queuedCount === 0
    && !blocked;
  const title = model.contextCompacting
    ? "Compaction du contexte Codex en cours…"
    : model.supportsCompact && model.hasCompactableSession
      ? `${usage.title} Cliquez ici ou saisissez /compact pour compacter.`
      : model.supportsCompact
        ? `${usage.title} Démarrez d'abord la conversation pour pouvoir la compacter.`
        : `${usage.title} La commande /compact est réservée aux conversations Codex.`;
  const detail = usage.usedPercent === null ? usage.unit : `${usage.usedPercent} %`;
  return `<button type="button" data-chat-control="tokens" data-chat-action="compact" data-chat-token-count="${usage.count ?? ""}" data-chat-token-signature="${escapeHtml(usage.signature)}" data-chat-context-percent="${usage.usedPercent ?? ""}" class="chat-token-usage pressure-${usage.pressure} ${usage.count === null ? "is-unavailable" : ""} ${model.contextCompacting ? "is-compacting" : ""}" title="${escapeHtml(title)}" aria-label="${escapeHtml(title)}" ${model.contextCompacting ? 'aria-busy="true"' : ""} ${canCompact ? "" : "disabled"}>
    <i data-lucide="${model.contextCompacting ? "loader-circle" : "gauge"}" aria-hidden="true"></i>
    <strong data-chat-token-value>${escapeHtml(usage.value)}</strong>
    <small>${escapeHtml(detail)}</small>
  </button>`;
};

const renderWelcome = (): string => `
  <div class="chat-welcome">
    <span class="chat-welcome-mark"><i data-lucide="sparkles"></i></span>
    <h1>Que voulez-vous construire ?</h1>
    <p>Decrivez le resultat attendu. L'agent explore l'environnement, modifie les fichiers et vous repond ici.</p>
    <div class="chat-starters">
      <button type="button" data-chat-starter="Analyse ce projet et explique-moi clairement son architecture.">
        <strong>Comprendre le projet</strong><span>Architecture, flux et points d'entree</span>
      </button>
      <button type="button" data-chat-starter="Aide-moi a diagnostiquer ce probleme : ">
        <strong>Trouver un bug</strong><span>Reproduction, cause et correction</span>
      </button>
      <button type="button" data-chat-starter="Ameliore l'interface de cette application, notamment son accessibilite et son responsive.">
        <strong>Ameliorer l'interface</strong><span>Accessibilite, responsive et finition</span>
      </button>
    </div>
  </div>`;

const renderLegacyChatFeedInner = (model: ChatPanelModel, instanceId = ""): string => {
  if (model.loading && model.messages.length === 0) {
    return `<div class="chat-empty"><span class="chat-loader"></span>Chargement de la conversation…</div>`;
  }
  if (model.error && model.messages.length === 0) {
    return `<div class="chat-error">${escapeHtml(model.error)}</div>`;
  }

  const notice = model.truncated
    ? `<div class="chat-notice">Discussion tres longue : seuls les derniers messages sont affiches.</div>`
    : "";
  const visibleMessages = model.messages
    .map((message, index) => ({ message, index }))
    .filter(({ message }) => chatMessageHasVisibleContent(message));
  const messages = visibleMessages.length
    ? visibleMessages
        .map(({ message, index }, visibleIndex) => {
          const previousMessage = visibleMessages[visibleIndex - 1]?.message;
          const showIdentity = message.role !== "assistant" || previousMessage?.role !== "assistant";
          const highlightQuestion =
            visibleIndex === visibleMessages.length - 1 &&
            message.role === "assistant" &&
            conversationWaitsForUser([message]);
          return renderMessage(message, model.providerLabel, index, showIdentity, highlightQuestion, instanceId);
        })
        .join("")
    : renderWelcome();
  const thinking = renderThoughtStream(model.thoughts, model.turnStatus);
  const turnError = model.turnError
    ? `<div class="chat-error chat-turn-error">${escapeHtml(model.turnError)}</div>`
    : "";
  const quotaSuggestion = model.quotaSuggestion
    ? `<div class="chat-quota-suggestion" role="status" aria-live="polite">
        <span><strong>Continuité automatique.</strong> Une capacité compatible prend le relais…</span>
      </div>`
    : "";
  return notice + messages + thinking + renderActivities(model.activities) + turnError + quotaSuggestion;
};

export const renderChatFeedInner = (model: ChatPanelModel, instanceId = ""): string => {
  if (model.loading && model.messages.length === 0) {
    return `<div class="chat-empty"><span class="chat-loader"></span>Chargement de la conversation...</div>`;
  }
  if (model.error && model.messages.length === 0) {
    return `<div class="chat-error">${escapeHtml(model.error)}</div>`;
  }

  const notice = model.truncated
    ? `<div class="chat-notice">Discussion tres longue : seuls les derniers messages sont affiches.</div>`
    : "";
  const turns = chatTurns(model);
  const { hiddenTurnCount } = resolveChatTurnWindow(turns.length, model.visibleTurnLimit);
  const visibleTurns = hiddenTurnCount > 0 ? turns.slice(hiddenTurnCount) : turns;
  const olderTurns = hiddenTurnCount > 0
    ? `<div class="chat-render-window" role="status">
        <button type="button" data-chat-action="show-older-turns" data-hidden-turn-count="${hiddenTurnCount}">
          Afficher les ${hiddenTurnCount} echange${hiddenTurnCount > 1 ? "s" : ""} precedent${hiddenTurnCount > 1 ? "s" : ""}
        </button>
      </div>`
    : "";
  const conversation = visibleTurns.length
    ? `<div data-component="message-timeline" class="chat-message-timeline">
        ${visibleTurns.map((turn, index) => renderOpenCodeTurn(turn, index, visibleTurns.length, model, instanceId)).join("")}
      </div>`
    : renderWelcome();
  const microsoftPendingActions = renderMicrosoftPendingActions();
  const turnError = model.turnError
    ? `<div class="chat-error chat-turn-error"><i data-lucide="circle-alert"></i><span>${escapeHtml(model.turnError)}</span></div>`
    : "";
  const quotaSuggestion = model.quotaSuggestion
    ? `<div class="chat-quota-suggestion" role="status" aria-live="polite">
        <span><strong>Continuité automatique.</strong> Une capacité compatible prend le relais...</span>
      </div>`
    : "";
  return notice + olderTurns + conversation + microsoftPendingActions + turnError + quotaSuggestion;
};

const modelSuggestions = (model: ChatPanelModel): string => {
  const selectedIsAvailable = model.modelSuggestions.some(
    (candidate) => candidate.toLocaleLowerCase() === model.selectedModel.toLocaleLowerCase(),
  );
  // Un ID OpenRouter disparu reste visible dans le champ comme etat invalide,
  // sans reapparaitre dans la liste des modeles disponibles.
  const includeSelected = !model.reasoningEffortAutomatic || selectedIsAvailable;
  return Array.from(new Set([
    ...(includeSelected ? [model.selectedModel] : []),
    ...model.modelSuggestions,
  ].filter(Boolean)))
    .map((value) => `<option value="${escapeHtml(value)}"></option>`)
    .join("");
};

const reasoningEffortOptions = (model: ChatPanelModel): string => {
  const automatic = model.reasoningEffortAutomatic
    ? `<option value="" ${model.selectedReasoningEffort ? "" : "selected"}>Automatique · OpenRouter</option>`
    : "";
  if (!model.supportsReasoningEffort) {
    return automatic || `<option value="">Non applicable</option>`;
  }
  return automatic + model.reasoningEffortOptions
    .map(
      (option) =>
        `<option value="${escapeHtml(option.value)}" ${option.value === model.selectedReasoningEffort ? "selected" : ""}>${escapeHtml(option.label)}</option>`,
    )
    .join("");
};

const renderChatImageAttachments = (
  images: readonly ChatImageAttachment[],
): string => images.length ? `<div class="chat-image-attachments" data-chat-control="image-attachments" role="list" aria-label="Images jointes au prochain message">
  ${images.map((image) => {
    const imageId = escapeHtml(image.id);
    const name = escapeHtml(image.name);
    return `<figure class="chat-image-attachment" role="listitem">
      <img src="${escapeHtml(image.previewUrl)}" alt="" />
      <figcaption title="${name}">${name}</figcaption>
      <button type="button" data-chat-action="remove-image" data-chat-image-id="${imageId}" title="Retirer ${name}" aria-label="Retirer l'image ${name}"><i data-lucide="x"></i></button>
    </figure>`;
  }).join("")}
</div>` : "";

export const renderChatHistory = (model: ChatPanelModel, instanceId = ""): string => {
  if (!model.historyOpen) return "";
  const userMessages = model.messages
    .map((message, index) => ({ message, index }))
    .filter(({ message }) => message.role === "user");
  const content = userMessages.length
    ? `<ol class="chat-history-list">
        ${userMessages
          .map(
            ({ message, index }, position) => `
              <li>
                <button type="button" data-chat-history-message="${index}" title="Revenir a ce message dans le chat">
                  <span class="chat-history-index">${position + 1}</span>
                  <span class="chat-history-copy">${escapeHtml(message.text)}</span>
                  ${message.timestamp ? `<time>${escapeHtml(timeLabel(message.timestamp))}</time>` : ""}
                </button>
              </li>`,
          )
          .join("")}
      </ol>`
    : `<div class="chat-history-empty">Vous n'avez encore envoye aucun message dans ce chat.</div>`;
  return `
    <aside id="${instanceId ? `chatHistory-${instanceId}` : "chatHistory"}" data-chat-control="history" class="chat-history" aria-label="Historique de ce chat">
      <header>
        <div>
          <strong>Historique du chat</strong>
          <span>${userMessages.length} message(s) envoye(s)</span>
        </div>
        <button id="${instanceId ? `chatHistoryClose-${instanceId}` : "chatHistoryClose"}" data-chat-action="history-close" type="button" class="icon-button" title="Fermer l'historique" aria-label="Fermer l'historique">
          <i data-lucide="x"></i>
        </button>
      </header>
      ${model.truncated ? `<div class="chat-history-notice">Le debut de cette tres longue conversation n'est plus charge.</div>` : ""}
      ${content}
    </aside>`;
};

export const renderChatPanel = (
  model: ChatPanelModel,
  options: ChatPanelRenderOptions = {},
): string => {
  const accountTransition = options.accountTransition;
  const running = model.turnStatus === "running";
  const busy = chatTurnIsBusy(model.turnStatus) || !!accountTransition || model.contextCompacting;
  const queued = model.queuedCount > 0;
  const userMessageCount = model.messages.filter((message) => message.role === "user").length;
  const instanceId = (options.instanceId ?? "").replace(/[^a-zA-Z0-9_-]/g, "-");
  const id = (base: string) => instanceId ? `${base}-${instanceId}` : base;
  const compact = options.compact === true;
  const fullscreen = options.fullscreen === true;
  const autonomous = options.autonomous;
  const orchestration = options.orchestration;
  const managedByOrchestration =
    orchestration?.role === "orchestrator" || orchestration?.role === "worker";
  const expertClass = instanceId
    ? `chat-panel--expert ${options.active ? "active" : ""} ${compact ? "chat-panel--compact" : ""} ${fullscreen ? "is-fullscreen" : ""} ${managedByOrchestration ? "is-orchestration-managed" : ""} ${autonomous?.role === "linked" ? "is-autonomous-linked" : ""}`
    : "";
  return `
  <section id="${id("chatPanel")}" data-chat-panel="${instanceId}" class="chat-panel ${expertClass} ${model.newConversation ? "chat-panel--new" : ""} ${model.historyOpen ? "chat-panel--history" : ""} ${accountTransition ? "is-account-transitioning" : ""}" ${accountTransition ? 'aria-busy="true"' : ""}>
    <header class="chat-head">
      ${compact ? "" : `<button id="${id("chatBack")}" data-chat-action="back" class="icon-button wide" title="Toutes les conversations" aria-label="Toutes les conversations">
        <i data-lucide="arrow-left"></i>
      </button>`}
      ${!compact && options.paneIndex ? `<span class="expert-chat-pane-index" title="Chat ${options.paneIndex}">${options.paneIndex}</span>` : ""}
      <div class="chat-head-main">
        <strong class="chat-title" title="${escapeHtml(model.title)}">${escapeHtml(model.title)}</strong>
        ${compact ? "" : `<span id="${id("chatSubtitle")}" data-chat-control="subtitle" class="chat-sub">${escapeHtml(model.subtitle)}</span>`}
      </div>
      ${renderChatTurnStatus(model)}
      ${renderChatTokenUsage(model, busy)}
      <div class="chat-head-actions">
        ${compact ? "" : `<span id="${id("chatSync")}" data-chat-control="sync" class="chat-sync chat-sync--${model.syncState}" aria-live="polite">
          <span class="chat-sync-dot" aria-hidden="true"></span>
          <span data-chat-sync-label>${escapeHtml(chatSyncLabel(model.syncState))}</span>
        </span>
        <span class="chat-provider">${escapeHtml(model.providerLabel)}</span>
        ${model.nodeLabel ? `<span class="chat-node" title="Chat alloue a ${escapeHtml(model.nodeLabel)}"><i data-lucide="server"></i><span>${escapeHtml(model.nodeLabel)}</span></span>` : ""}
        <button id="${id("chatResume")}" type="button" data-open-discussions class="tool-button chat-resume-button" title="Choisir une discussion a reprendre" aria-label="Reprendre une discussion">
          <i data-lucide="messages-square"></i><span>Reprendre une discussion</span>
        </button>
        <button id="${id("chatHistoryToggle")}" data-chat-action="history-toggle" class="tool-button chat-history-button ${model.historyOpen ? "primary" : ""}" title="Afficher les messages envoyes dans ce chat" aria-label="Historique de ce chat" aria-expanded="${model.historyOpen}">
          <i data-lucide="history"></i><span>Historique</span><small>${userMessageCount}</small>
        </button>
         <button id="${id("chatRefresh")}" data-chat-action="refresh" class="icon-button wide" title="Actualiser" aria-label="Actualiser">
           <i data-lucide="refresh-ccw"></i>
         </button>`}
        ${compact ? `<button id="${id("chatHistoryToggle")}" data-chat-action="history-toggle" class="tool-button chat-history-button ${model.historyOpen ? "primary" : ""}" title="Afficher les messages envoyes dans ce chat" aria-label="Historique de ce chat" aria-expanded="${model.historyOpen}">
          <i data-lucide="history"></i><span>Historique</span><small>${userMessageCount}</small>
        </button>` : ""}
        ${compact ? "" : `<button id="${id("chatNew")}" data-chat-action="new" class="tool-button primary chat-new-button" title="Nouvelle conversation">
          <i data-lucide="plus"></i><span>Nouveau chat</span>
        </button>`}
      </div>
      <span class="chat-account-name" title="Compte : ${escapeHtml(model.accountLabel)}" aria-label="Compte : ${escapeHtml(model.accountLabel)}">
        <i data-lucide="user-round" aria-hidden="true"></i>
        <span>${escapeHtml(model.accountLabel)}</span>
      </span>
      ${instanceId ? `<div class="expert-chat-pane-controls" aria-label="Actions de ce chat">
        ${autonomous ? `<button
          id="${id("chatAutonomous")}"
          type="button"
          class="expert-chat-autonomous-action role-${autonomous.role} tone-${autonomous.tone ?? "paused"}"
          data-chat-action="${autonomous.role === "linked" ? "edit-autonomous" : "autonomize"}"
          title="${escapeHtml(autonomous.detail)}"
          aria-label="${escapeHtml(autonomous.detail)}"
          ${autonomous.disabled || accountTransition ? "disabled" : ""}
        >
          <i data-lucide="bot"></i>
          <span>${escapeHtml(autonomous.label)}</span>
        </button>` : ""}
        ${orchestration ? `<button
          id="${id("chatOrchestration")}"
          type="button"
          class="expert-chat-orchestration-action role-${orchestration.role}"
          data-chat-action="open-orchestration"
          title="${escapeHtml(orchestration.detail)}"
          aria-label="${escapeHtml(orchestration.detail)}"
          ${orchestration.disabled || accountTransition ? "disabled" : ""}
        >
          <i data-lucide="${orchestration.role === "orchestrator" ? "brain-circuit" : "bot"}"></i>
          <span>${escapeHtml(orchestration.label)}</span>
        </button>` : ""}
        <button id="${id("chatFullscreen")}" type="button" class="expert-chat-pane-fullscreen" data-chat-action="fullscreen" title="${fullscreen ? "Quitter le plein écran" : "Afficher ce chat en plein écran"}" aria-label="${fullscreen ? "Quitter le plein écran" : "Afficher ce chat en plein écran"}" aria-pressed="${fullscreen}" ${accountTransition ? "disabled" : ""}>
          <i data-lucide="${fullscreen ? "minimize-2" : "maximize-2"}"></i>
        </button>
        <button id="${id("chatClose")}" type="button" class="expert-chat-pane-close" data-chat-action="close" title="Fermer ce chat" aria-label="Fermer ce chat" ${accountTransition ? "disabled" : ""}>
          <i data-lucide="x"></i>
        </button>
      </div>` : ""}
    </header>
    ${accountTransition ? `<div class="chat-account-transition" role="status" aria-live="polite">
      <span class="chat-account-transition-icon" aria-hidden="true"><i data-lucide="loader-circle"></i></span>
      <span><strong>${escapeHtml(accountTransition.label)}</strong><small>${escapeHtml(accountTransition.detail)}</small></span>
    </div>` : ""}
    <div class="chat-conversation-body">
      <div id="${id("chatFeed")}" data-chat-control="feed" class="chat-feed" tabindex="0" aria-label="Messages de la conversation">${renderChatFeedInner(model, instanceId)}</div>
      ${renderChatHistory(model, instanceId)}
    </div>
    ${managedByOrchestration ? `<footer class="chat-orchestration-managed" data-chat-control="orchestration-managed">
      <span class="chat-orchestration-managed-icon"><i data-lucide="${orchestration?.role === "orchestrator" ? "brain-circuit" : "bot"}"></i></span>
      <span><strong>${escapeHtml(orchestration?.label || "Chat orchestré")}</strong><small>${escapeHtml(orchestration?.detail || "Piloté par l’orchestration")}</small></span>
      <button type="button" class="tool-button" data-chat-action="open-orchestration"><i data-lucide="users"></i><span>Suivre</span></button>
    </footer>` : `<form id="${id("chatComposer")}" data-chat-control="composer" class="chat-composer ${busy ? "is-running" : ""} ${queued ? "has-queued-message" : ""}">
      <div class="chat-composer-box">
        ${renderChatImageAttachments(model.imageAttachments)}
        <textarea id="${id("chatPrompt")}" data-chat-control="prompt" rows="1" maxlength="${CHAT_COMPOSER_MAX_CHARS}" placeholder="${busy ? "Écrivez le prochain message à envoyer…" : `Demandez a ${escapeHtml(model.providerLabel || "l'agent")} de construire quelque chose…`}" aria-label="${busy ? "Prochain message à mettre en attente" : "Message à envoyer"}">${escapeHtml(model.draft)}</textarea>
        ${model.favoritePrompts.length ? `<div class="chat-favorite-prompts" data-chat-control="favorite-prompts" role="group" aria-label="Prompts favoris à insérer">
          ${model.favoritePrompts.map((prompt) => {
            const promptId = escapeHtml(prompt.id);
            const promptTitle = escapeHtml(prompt.title);
            return `<button type="button" class="chat-favorite-prompt-button" data-chat-action="favorite-prompt" data-chat-prompt-id="${promptId}" title="Insérer le prompt favori : ${promptTitle}" aria-label="Insérer le prompt favori : ${promptTitle}"><i data-lucide="star"></i><span>${promptTitle}</span></button>`;
          }).join("")}
        </div>` : ""}
        ${queued ? `<div class="chat-queue-state" role="status" aria-live="polite">
          <span><i data-lucide="clock-3"></i><strong>${model.queuedCount}</strong> message${model.queuedCount > 1 ? "s" : ""} en attente${busy ? " · envoi automatique à la fin du tour" : ""}</span>
          <button type="button" data-chat-action="clear-queue" title="Annuler ${model.queuedCount > 1 ? "les messages en attente" : "le message en attente"}">Annuler</button>
        </div>` : ""}
        <div class="chat-composer-toolbar">
          <div class="chat-composer-context">
            <button
              id="${id("chatPrompts")}"
              data-chat-action="prompts"
              type="button"
              class="chat-prompt-button"
              title="Insérer un prompt enregistré"
              aria-label="Choisir un prompt enregistré"
            >
              <i data-lucide="message-square-text"></i><span>Prompts</span>
            </button>
            <label class="chat-mode-select" title="${model.composerSelectorsEnabled ? "Mode de travail" : "Sélection verrouillée dans les paramètres"}">
              <i data-lucide="sparkles"></i>
              <select id="${id("chatMode")}" data-chat-control="mode" ${busy || !model.composerSelectorsEnabled ? "disabled" : ""} aria-label="Mode de travail">
                <option value="build" ${model.mode === "build" ? "selected" : ""}>Construire</option>
                <option value="plan" ${model.mode === "plan" ? "selected" : ""}>Planifier</option>
                <option value="ask" ${model.mode === "ask" ? "selected" : ""}>Question</option>
              </select>
            </label>
            <label class="chat-model-select" title="${model.composerSelectorsEnabled ? "Modele utilise pour les prochains messages" : "Sélection verrouillée dans les paramètres"}">
              <i data-lucide="cpu"></i>
              <input id="${id("chatModel")}" data-chat-control="model" list="${id("chatModelSuggestions")}" value="${escapeHtml(model.selectedModel)}" ${busy || !model.selectedAccountId || !model.composerSelectorsEnabled ? "disabled" : ""} aria-label="Modele" autocomplete="off" spellcheck="false" maxlength="160" />
              <datalist id="${id("chatModelSuggestions")}">${modelSuggestions(model)}</datalist>
            </label>
            <label class="chat-effort-select" title="${!model.composerSelectorsEnabled ? "Sélection verrouillée dans les paramètres" : model.supportsReasoningEffort ? "Intensite de raisonnement" : "Ce fournisseur ne gere pas l'intensite de raisonnement"}">
              <i data-lucide="gauge"></i>
              <select id="${id("chatReasoningEffort")}" data-chat-control="reasoning-effort" ${busy || !model.selectedAccountId || !model.supportsReasoningEffort || !model.composerSelectorsEnabled ? "disabled" : ""} aria-label="Intensite de raisonnement">
                ${reasoningEffortOptions(model)}
              </select>
            </label>
            <button
              id="${id("chatFastMode")}"
              data-chat-control="fast-mode"
              type="button"
              class="chat-fast-mode ${model.fastModeEnabled ? "active" : ""}"
              title="${escapeHtml(!model.composerSelectorsEnabled ? "Sélection verrouillée dans les paramètres" : model.fastModeHelp)}"
              aria-label="${model.fastModeEnabled ? "Désactiver" : "Activer"} Fast mode"
              aria-pressed="${model.fastModeEnabled}"
              ${busy || !model.selectedAccountId || !model.supportsFastMode || !model.composerSelectorsEnabled ? "disabled" : ""}
            >
              <i data-lucide="zap"></i><span>${model.fastModeEnabled ? "Fast" : "Normal"}</span>
            </button>
            <span class="chat-workspace-chip" title="${escapeHtml(model.workspaceLabel)}"><i data-lucide="folder-open"></i>${escapeHtml(model.workspaceLabel)}</span>
          </div>
          <div class="chat-agent-tools" role="group" aria-label="Outils de conduite de l'agent">
            ${renderChatAgentTools(model.agentTools, model.enabledTools)}
          </div>
          ${renderChatRuntimeStatus(model)}
          <div class="chat-voice-control">
            <select
              data-chat-control="voice-mode"
              class="chat-voice-mode"
              title="Format du texte dicté"
              aria-label="Format du texte dicté"
              ${model.accounts.length ? "" : "disabled"}
            >
              <option value="faithful">Fidèle</option>
              <option value="clean" selected>Nettoyé</option>
              <option value="summary">Résumé</option>
            </select>
            <button
              id="${id("chatVoice")}"
              data-chat-action="voice"
              type="button"
              class="chat-voice"
              title="Dicter puis nettoyer localement"
              aria-label="Dicter un message avec le mode de texte choisi"
              aria-pressed="false"
              ${model.accounts.length ? "" : "disabled"}
            >
              <i class="chat-voice-idle-icon" data-lucide="mic"></i>
              <i class="chat-voice-stop-icon" data-lucide="square"></i>
              <i class="chat-voice-working-icon" data-lucide="loader-circle"></i>
            </button>
            <span id="${id("chatVoiceStatus")}" data-chat-control="voice-status" class="chat-voice-status" role="status" aria-live="polite"></span>
          </div>
          <button
            id="${id("chatGoal")}"
            data-chat-action="goal"
            type="button"
            class="chat-goal"
            title="${model.supportsGoals ? "Créer un goal à partir du texte saisi" : "Les goals ne sont pas pris en charge par ce fournisseur"}"
            aria-label="Créer un goal"
            ${busy || !model.selectedAccountId || !model.supportsGoals ? "disabled" : ""}
          >
            <i data-lucide="target"></i><span>Goal</span>
          </button>
          ${running ? `<button id="${id("chatStop")}" data-chat-action="stop" type="button" class="chat-send chat-stop" title="Arreter la reponse" aria-label="Arreter la reponse"><i data-lucide="square"></i></button>` : ""}
          <button id="${id("chatSend")}" data-chat-action="send" type="submit" class="chat-send ${busy ? "chat-queue-send" : ""}" title="${busy ? "Mettre le message en attente" : "Envoyer"}" aria-label="${busy ? "Mettre le message en attente" : "Envoyer"}" ${model.accounts.length && !accountTransition ? "" : "disabled"}><i data-lucide="arrow-up"></i></button>
        </div>
      </div>
      <small>${busy ? "Entree pour mettre en attente" : "Entree pour envoyer"} · Maj + Entree pour une nouvelle ligne · Collez une image avec Ctrl + V</small>
    </form>`}
  </section>`;
};
