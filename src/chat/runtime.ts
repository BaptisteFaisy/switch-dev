export type ChatDeliveryState = "pending" | "failed";

export type RuntimeChatPart = {
  id: string;
  kind: "reasoning" | "text" | "tool" | string;
  status: string;
  text?: string | null;
  tool?: string | null;
  title?: string | null;
  subtitle?: string | null;
  detail?: string | null;
  output?: string | null;
  /** Metadonnees derivees cote client pour une serie de sondages `wait`. */
  waitCellId?: string | null;
  waitCount?: number;
};

export type RuntimeChatMessage = {
  role: "user" | "assistant";
  text: string;
  timestamp: number;
  deliveryState?: ChatDeliveryState;
  parts?: RuntimeChatPart[];
};

// Les espaces, controles et caracteres Unicode explicitement invisibles ne
// doivent pas suffire a reserver une bulle dans la timeline. Les caracteres
// visibles (ponctuation et syntaxe Markdown comprises) restent intacts.
const CHAT_NON_RENDERING_TEXT = /[\p{White_Space}\p{Default_Ignorable_Code_Point}\p{Control}\u2800]/gu;

export const chatTextHasVisibleContent = (
  value: string | null | undefined,
): boolean => !!value && value.replace(CHAT_NON_RENDERING_TEXT, "").length > 0;

export const chatPartHasVisibleContent = (part: RuntimeChatPart): boolean =>
  part.kind === "tool" || chatTextHasVisibleContent(part.text);

export const chatMessageHasVisibleContent = (message: RuntimeChatMessage): boolean =>
  message.role === "user"
    ? chatTextHasVisibleContent(message.text)
    : chatTextHasVisibleContent(message.text)
      || !!message.parts?.some(chatPartHasVisibleContent);

/** Un tour reste verrouillé pendant la synchronisation qui suit la réponse finale. */
export const chatTurnIsBusy = (status: string | null | undefined): boolean =>
  status === "running" || status === "finalizing";

type ChatTurnSyncIdentity = {
  id: number;
  status: string;
  startedAt: number;
};

/**
 * Evite qu'une reponse tardive du catalogue des tours actifs ne ressuscite un
 * tour deja termine localement, tout en autorisant la reprise apres reload ou
 * depuis un autre client.
 */
export const shouldAdoptActiveChatTurn = (
  current: ChatTurnSyncIdentity | null | undefined,
  candidate: ChatTurnSyncIdentity,
): boolean => {
  if (!chatTurnIsBusy(candidate.status)) return false;
  if (!current) return true;
  if (current.id === candidate.id) return chatTurnIsBusy(current.status);
  if (current.id === 0) return true;
  if (candidate.startedAt > current.startedAt) return true;
  if (chatTurnIsBusy(current.status)) return false;
  return candidate.startedAt === current.startedAt && candidate.id > current.id;
};

export type ActiveChatTurnPaneIdentity = {
  key: string;
  accountId: string | null | undefined;
  turnId?: number | null;
  sessionIds?: readonly (string | null | undefined)[];
};

export type ActiveChatTurnIdentity = {
  id: number;
  accountId: string;
  sessionId?: string | null;
  sourceChatKey?: string | null;
};

/**
 * Reconnait le panneau qui possede un tour, meme si le panneau vient d'un
 * autre appareil et n'a pas encore pu charger son resume de discussion.
 */
export const activeChatTurnBelongsToPane = (
  turn: ActiveChatTurnIdentity,
  pane: ActiveChatTurnPaneIdentity,
): boolean => {
  const accountId = pane.accountId?.trim();
  if (!accountId || turn.accountId !== accountId) return false;
  if (pane.turnId === turn.id) return true;

  const sourceChatKey = turn.sourceChatKey?.trim();
  if (sourceChatKey && sourceChatKey === pane.key.trim()) return true;

  const sessionId = turn.sessionId?.trim();
  if (!sessionId) return false;
  return (pane.sessionIds ?? []).some((candidate) => candidate?.trim() === sessionId);
};

/**
 * Transforme l'objectif saisi via le bouton Goal en demande utilisateur
 * explicite. Un provider n'active un goal que lorsque l'utilisateur le demande :
 * le bouton doit donc conserver cette intention dans le transcript, quel que
 * soit le transport utilise (Tauri local ou API web).
 */
export const createGoalPrompt = (objective: string): string => {
  const normalized = objective.trim();
  if (!normalized) return "";
  return `Crée un goal avec l'outil create_goal pour l'objectif suivant, puis commence à le poursuivre. Si le fournisseur préfixe les outils MCP, utilise son nom exposé correspondant (par exemple cst_chat_create_goal avec OpenCode) :\n\n${normalized}`;
};

const isToolPart = (part: RuntimeChatPart): boolean => part.kind === "tool";

/**
 * Regroupe chaque serie d'actions consecutives dans la timeline.
 *
 * Le contenu reste ordonne et chaque action garde ses propres details, mais la
 * vue peut presenter toute la serie dans un seul bloc compact (commandes,
 * recherches web, modifications de fichiers, outils MCP, etc.).
 */
export const groupConsecutiveToolParts = <T extends RuntimeChatPart>(parts: T[]): T[][] => {
  const groups: T[][] = [];
  parts.forEach((part) => {
    const previous = groups[groups.length - 1];
    if (isToolPart(part) && previous?.every(isToolPart)) {
      previous.push(part);
      return;
    }
    groups.push([part]);
  });
  return groups;
};

const isWaitToolName = (value: string | null | undefined): boolean => {
  const normalized = value?.trim().toLocaleLowerCase();
  if (!normalized) return false;
  return normalized === "wait" || /(?:^|[.:/])wait$/.test(normalized);
};

const waitCellIdFromText = (value: string | null | undefined): string | null => {
  const raw = value?.trim();
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as { cell_id?: unknown; cellId?: unknown };
    const cellId = parsed?.cell_id ?? parsed?.cellId;
    if (typeof cellId === "string" || typeof cellId === "number") return String(cellId);
  } catch {
    // Les anciens rollouts peuvent contenir une representation non JSON.
  }
  return raw.match(/["']?cell_id["']?\s*[:=]\s*["']?([^"',}\s]+)/i)?.[1] ?? null;
};

const waitCellId = (part: RuntimeChatPart): string | null => {
  if (part.kind !== "tool") return null;
  if (![part.title, part.subtitle, part.tool].some(isWaitToolName)) return null;
  return waitCellIdFromText(part.detail) ?? waitCellIdFromText(part.subtitle);
};

const waitSubtitle = (cellId: string, count: number): string =>
  `Commande ${cellId} · ${count} vérification${count > 1 ? "s" : ""}`;

/**
 * Reunit tous les sondages `wait` visant la meme commande dans un tour.
 *
 * Les resumes de raisonnement peuvent etre emis entre deux sondages : le
 * regroupement se fait donc par `cell_id` sur tout le tour, pas seulement sur
 * des elements strictement adjacents. La premiere position reste stable et la
 * carte conserve le dernier statut et la derniere sortie connus.
 */
export const collapseRepeatedWaitParts = (
  parts: RuntimeChatPart[],
): RuntimeChatPart[] => {
  const collapsed: RuntimeChatPart[] = [];
  const waitIndexByCellId = new Map<string, number>();

  parts.forEach((part) => {
    const cellId = waitCellId(part);
    if (!cellId) {
      collapsed.push(part);
      return;
    }

    const existingIndex = waitIndexByCellId.get(cellId);
    if (existingIndex === undefined) {
      waitIndexByCellId.set(cellId, collapsed.length);
      collapsed.push({
        ...part,
        title: "Attente de la commande",
        subtitle: waitSubtitle(cellId, 1),
        waitCellId: cellId,
        waitCount: 1,
      });
      return;
    }

    const existing = collapsed[existingIndex];
    const count = (existing.waitCount ?? 1) + 1;
    collapsed[existingIndex] = {
      ...existing,
      status: part.status,
      title: "Attente de la commande",
      subtitle: waitSubtitle(cellId, count),
      detail: part.detail ?? existing.detail,
      output: part.output ?? existing.output,
      waitCellId: cellId,
      waitCount: count,
    };
  });

  return collapsed;
};

const sameMessageContent = (left: RuntimeChatMessage, right: RuntimeChatMessage): boolean =>
  left.role === right.role && left.text === right.text;

const timestampsCouldMatch = (left: RuntimeChatMessage, right: RuntimeChatMessage): boolean =>
  !left.timestamp || !right.timestamp || Math.abs(left.timestamp - right.timestamp) <= 120;

/**
 * Reconcile un transcript serveur avec les messages optimistes de l'interface.
 * Pendant un tour, un transcript legerement en retard ne doit jamais retirer la
 * fin du fil deja visible. Les messages optimistes disparaissent uniquement
 * lorsque leur equivalent persiste arrive du serveur.
 */
export const reconcileChatMessages = <T extends RuntimeChatMessage>(
  current: T[],
  incoming: T[],
  preserveStableTail = false,
): T[] => {
  const stableCurrent = current.filter((message) => !message.deliveryState);
  const incomingIsOlderPrefix =
    preserveStableTail &&
    incoming.length < stableCurrent.length &&
    incoming.every((message, index) => sameMessageContent(message, stableCurrent[index]));
  const merged = [...(incomingIsOlderPrefix ? stableCurrent : incoming)] as T[];

  current
    .filter((message) => !!message.deliveryState)
    .forEach((optimistic) => {
      const persisted = merged.some(
        (message) =>
          sameMessageContent(message, optimistic) && timestampsCouldMatch(message, optimistic),
      );
      if (persisted) return;

      const insertAt = merged.findIndex(
        (message) =>
          !!optimistic.timestamp &&
          !!message.timestamp &&
          message.timestamp > optimistic.timestamp,
      );
      if (insertAt < 0) merged.push(optimistic);
      else merged.splice(insertAt, 0, optimistic);
    });

  return merged;
};

export const chatMessagesEqual = (
  left: RuntimeChatMessage[],
  right: RuntimeChatMessage[],
): boolean =>
  left.length === right.length &&
  left.every(
    (message, index) =>
      sameMessageContent(message, right[index]) &&
      message.timestamp === right[index]?.timestamp &&
      message.deliveryState === right[index]?.deliveryState &&
      JSON.stringify(message.parts ?? []) === JSON.stringify(right[index]?.parts ?? []),
  );

export const markLatestPendingMessageFailed = <T extends RuntimeChatMessage>(messages: T[]): T[] => {
  let index = -1;
  for (let position = messages.length - 1; position >= 0; position -= 1) {
    if (messages[position]?.deliveryState === "pending") {
      index = position;
      break;
    }
  }
  if (index < 0) return messages;
  return messages.map((message, position) =>
    position === index ? ({ ...message, deliveryState: "failed" } as T) : message,
  );
};

const REQUEST_USER_INPUT_NAME = /(?:^|[.:/_])request_user_input$/i;
const REQUEST_USER_INPUT_CALL = /\btools\.(?:mcp__[a-z0-9_-]+__)?request_user_input\s*\(/i;

/**
 * Detecte l'interface structuree `request_user_input`, et non une simple
 * question dans le texte de l'assistant. Tant que l'outil n'a pas de sortie,
 * son formulaire est considere comme ouvert.
 */
export const partWaitsForUserInput = (part: RuntimeChatPart): boolean => {
  if (part.kind !== "tool") return false;
  if (["error", "failed", "cancelled"].includes(part.status.toLocaleLowerCase())) return false;
  if (part.output?.trim()) return false;

  const directToolName = [part.tool, part.title, part.subtitle]
    .some((value) => REQUEST_USER_INPUT_NAME.test(value?.trim() ?? ""));
  const nestedToolCall = [part.detail, part.subtitle]
    .some((value) => REQUEST_USER_INPUT_CALL.test(value ?? ""));
  return directToolName || nestedToolCall;
};

export const conversationWaitsForUser = (
  messages: RuntimeChatMessage[],
  activeParts?: RuntimeChatPart[],
): boolean => {
  if (activeParts !== undefined) return activeParts.some(partWaitsForUserInput);
  const last = messages[messages.length - 1];
  return !!last && last.role === "assistant" && (last.parts ?? []).some(partWaitsForUserInput);
};

export const formatChatDuration = (seconds: number): string => {
  const safe = Math.max(0, Math.floor(Number.isFinite(seconds) ? seconds : 0));
  if (safe < 60) return `${safe} s`;
  if (safe < 3600) {
    const minutes = Math.floor(safe / 60);
    const remainder = safe % 60;
    return remainder ? `${minutes} min ${remainder.toString().padStart(2, "0")} s` : `${minutes} min`;
  }
  const hours = Math.floor(safe / 3600);
  const minutes = Math.floor((safe % 3600) / 60);
  return minutes ? `${hours} h ${minutes.toString().padStart(2, "0")} min` : `${hours} h`;
};

export const formatChatResetCountdown = (resetAt: number, now = Date.now() / 1000): string => {
  const remaining = Math.max(0, Math.floor(resetAt - now));
  if (remaining <= 0) return "maintenant";
  if (remaining < 60) return `dans ${remaining} s`;
  if (remaining < 3600) return `dans ${Math.ceil(remaining / 60)} min`;
  if (remaining < 86_400) {
    const hours = Math.floor(remaining / 3600);
    const minutes = Math.floor((remaining % 3600) / 60);
    return minutes ? `dans ${hours} h ${minutes} min` : `dans ${hours} h`;
  }
  const days = Math.floor(remaining / 86_400);
  const hours = Math.floor((remaining % 86_400) / 3600);
  return hours ? `dans ${days} j ${hours} h` : `dans ${days} j`;
};
