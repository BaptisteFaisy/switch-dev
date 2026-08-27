export const CHAT_SIDEBAR_DEFAULT_WIDTH = 312;
export const CHAT_SIDEBAR_COMPACT_WIDTH = 280;
export const CHAT_SIDEBAR_MIN_WIDTH = 0;
export const CHAT_SIDEBAR_MAX_WIDTH = 420;

export type ChatSidebarStatus = "idle" | "running" | "question";

export const DEFAULT_CHAT_SIDEBAR_PRIORITY_MODE = "recent" as const;

export type ChatSidebarPriorityMode =
  | typeof DEFAULT_CHAT_SIDEBAR_PRIORITY_MODE
  | "question"
  | "available";

export type ChatSidebarPrioritizableItem = {
  status: ChatSidebarStatus;
};

export type ChatSidebarDiscussionIdentity = {
  accountId: string;
  sessionId: string;
  rolloutId?: string | null;
};

/**
 * Une copie peut conserver le meme identifiant natif sur deux comptes
 * (notamment Freebuff). Toute cle frontend doit donc inclure le compte pour ne
 * jamais ouvrir, rafraichir ou archiver la mauvaise ligne d'historique.
 */
export const discussionIdentityKey = (
  discussion: Pick<ChatSidebarDiscussionIdentity, "accountId" | "sessionId">,
): string => JSON.stringify([
  discussion.accountId.trim(),
  discussion.sessionId.trim(),
]);

export type ChatSidebarOrderedDiscussion = {
  sessionId: string;
  startedAt: number;
};

export type ChatSidebarTurnIdentity = {
  id: number;
  accountId: string;
  sessionId?: string | null;
  status: string;
  startedAt: number;
};

/**
 * Resout l'identite renvoyee par un tour de chat vers la discussion visible.
 * Apres une reprise Codex, le moteur peut connaitre le rollout courant alors
 * que la liste est regroupee sous l'identite logique de la conversation.
 */
export const discussionForSession = <T extends ChatSidebarDiscussionIdentity>(
  discussions: readonly T[],
  accountId: string,
  sessionId: string,
): T | null => {
  const expectedAccount = accountId.trim();
  const expectedSession = sessionId.trim();
  if (!expectedAccount || !expectedSession) return null;
  const accountDiscussions = discussions.filter(
    (discussion) => discussion.accountId === expectedAccount,
  );
  return accountDiscussions.find(
    (discussion) => discussion.sessionId.trim() === expectedSession,
  ) ?? accountDiscussions.find(
    (discussion) => discussion.rolloutId?.trim() === expectedSession,
  ) ?? null;
};

/**
 * Garde une place fixe pour chaque conversation dans la barre laterale.
 * L'activite d'un chat ne doit jamais le faire remonter devant les autres :
 * seul son instant de creation, immuable, participe au classement.
 */
export const orderChatSidebarDiscussions = <T extends ChatSidebarOrderedDiscussion>(
  discussions: readonly T[],
): T[] => [...discussions].sort((left, right) =>
  right.startedAt - left.startedAt || left.sessionId.localeCompare(right.sessionId),
);

export const normalizeChatSidebarPriorityMode = (
  value: unknown,
): ChatSidebarPriorityMode =>
  value === "question" || value === "available"
    ? value
    : DEFAULT_CHAT_SIDEBAR_PRIORITY_MODE;

/**
 * Applique les preferences visuelles de la liste d'environnement sans modifier
 * l'ordre de base entre deux chats de meme priorite. Les chats masques restent
 * actifs : seule leur rangee orange disparait de la barre laterale.
 */
export const arrangeChatSidebarItems = <T extends ChatSidebarPrioritizableItem>(
  items: readonly T[],
  priorityMode: ChatSidebarPriorityMode,
  hideRunning: boolean,
): T[] => {
  const visibleItems = hideRunning
    ? items.filter((item) => item.status !== "running")
    : [...items];
  if (priorityMode === "recent") return visibleItems;

  const preferredStatus: ChatSidebarStatus = priorityMode === "question"
    ? "question"
    : "idle";
  return visibleItems
    .map((item, index) => ({ item, index }))
    .sort((left, right) =>
      Number(right.item.status === preferredStatus)
      - Number(left.item.status === preferredStatus)
      || left.index - right.index,
    )
    .map(({ item }) => item);
};

/**
 * Retrouve le tour serveur actif d'une discussion, y compris apres une reprise
 * Codex ou le rollout courant differe de l'identite logique de la discussion.
 */
export const activeChatTurnForDiscussion = <T extends ChatSidebarTurnIdentity>(
  turns: readonly T[],
  discussion: ChatSidebarDiscussionIdentity | null | undefined,
): T | null => {
  if (!discussion) return null;
  const discussionIds = new Set(
    [discussion.sessionId, discussion.rolloutId]
      .map((value) => value?.trim())
      .filter((value): value is string => !!value),
  );
  return turns
    .filter(
      (turn) =>
        (turn.status === "running" || turn.status === "finalizing") &&
        turn.accountId === discussion.accountId &&
        !!turn.sessionId &&
        discussionIds.has(turn.sessionId.trim()),
    )
    .sort((left, right) => right.startedAt - left.startedAt || right.id - left.id)[0] ?? null;
};

export const chatSidebarStatus = (
  turnStatus: string | null | undefined,
  waitingForUser: boolean,
  serverTurnStatus?: string | null,
): ChatSidebarStatus => {
  if (waitingForUser) return "question";
  if (
    turnStatus === "running"
    || turnStatus === "finalizing"
    || serverTurnStatus === "running"
    || serverTurnStatus === "finalizing"
  ) return "running";
  return "idle";
};

export const chatSidebarStatusLabel = (status: ChatSidebarStatus): string => {
  switch (status) {
    case "running":
      return "En cours";
    case "question":
      return "Question";
    default:
      return "Disponible";
  }
};

const CHAT_CONTENT_MIN_WIDTH = 360;
const CHAT_COMPACT_VIEWPORT_WIDTH = 980;

export const chatSidebarMaxWidth = (viewportWidth: number): number => {
  const availableWidth = Number.isFinite(viewportWidth)
    ? Math.floor(viewportWidth) - CHAT_CONTENT_MIN_WIDTH
    : CHAT_SIDEBAR_MAX_WIDTH;
  return Math.max(
    CHAT_SIDEBAR_MIN_WIDTH,
    Math.min(CHAT_SIDEBAR_MAX_WIDTH, availableWidth),
  );
};

export const clampChatSidebarWidth = (width: number, viewportWidth: number): number => {
  const safeWidth = Number.isFinite(width) ? Math.round(width) : CHAT_SIDEBAR_DEFAULT_WIDTH;
  return Math.max(
    CHAT_SIDEBAR_MIN_WIDTH,
    Math.min(chatSidebarMaxWidth(viewportWidth), safeWidth),
  );
};

export const defaultChatSidebarWidth = (viewportWidth: number): number =>
  viewportWidth <= CHAT_COMPACT_VIEWPORT_WIDTH
    ? CHAT_SIDEBAR_COMPACT_WIDTH
    : CHAT_SIDEBAR_DEFAULT_WIDTH;
