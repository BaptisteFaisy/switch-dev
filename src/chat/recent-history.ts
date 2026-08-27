export type PersistedSentChatMessage = {
  role: "user" | "assistant";
  text: string;
  timestamp: number;
};

type RecentHistoryPart = {
  kind?: unknown;
  text?: unknown;
};

type RecentHistoryMessage = {
  role?: unknown;
  text?: unknown;
  timestamp?: unknown;
  parts?: unknown;
};

export const RECENT_SENT_CHAT_MESSAGE_LIMIT = 24;
export const RECENT_SENT_CHAT_HISTORY_MAX_CHARS = 64_000;
const RECENT_SENT_CHAT_MESSAGE_MAX_CHARS = 16_000;
const TRUNCATION_MARKER = "\n\n[… message raccourci dans l’historique local …]\n\n";

const visibleText = (value: unknown): value is string =>
  typeof value === "string" && value.trim().length > 0;

const sentMessageText = (message: RecentHistoryMessage): string | null => {
  if (visibleText(message.text)) return message.text;
  if (message.role !== "assistant" || !Array.isArray(message.parts)) return null;

  const finalParts = (message.parts as RecentHistoryPart[])
    .filter((part) => part?.kind === "text" && visibleText(part.text))
    .map((part) => part.text as string);
  return finalParts.length > 0 ? finalParts.join("\n\n") : null;
};

const boundedText = (text: string, limit: number): string => {
  if (text.length <= limit) return text;
  if (limit <= TRUNCATION_MARKER.length + 2) return text.slice(0, limit);
  const tailLength = Math.max(1, Math.floor((limit - TRUNCATION_MARKER.length) / 3));
  const headLength = limit - TRUNCATION_MARKER.length - tailLength;
  return `${text.slice(0, headLength)}${TRUNCATION_MARKER}${text.slice(-tailLength)}`;
};

/**
 * Conserve seulement les bulles réellement échangees. Les parts `reasoning`
 * et `tool`, le thinking courant et les etats de livraison ne sont jamais
 * serialises dans l'historique local.
 */
export const recentSentChatMessages = (
  messages: readonly RecentHistoryMessage[],
): PersistedSentChatMessage[] => {
  const recent: PersistedSentChatMessage[] = [];
  let remainingCharacters = RECENT_SENT_CHAT_HISTORY_MAX_CHARS;

  for (let index = messages.length - 1; index >= 0; index -= 1) {
    if (recent.length >= RECENT_SENT_CHAT_MESSAGE_LIMIT || remainingCharacters <= 0) break;
    const message = messages[index];
    if (message?.role !== "user" && message?.role !== "assistant") continue;
    const text = sentMessageText(message);
    if (!text) continue;

    const retainedText = boundedText(
      text,
      Math.min(RECENT_SENT_CHAT_MESSAGE_MAX_CHARS, remainingCharacters),
    );
    if (!retainedText) continue;
    recent.unshift({
      role: message.role,
      text: retainedText,
      timestamp:
        typeof message.timestamp === "number" && Number.isFinite(message.timestamp)
          ? Math.max(0, Math.floor(message.timestamp))
          : 0,
    });
    remainingCharacters -= retainedText.length;
  }

  return recent;
};

export const restoreRecentSentChatMessages = (
  value: unknown,
): PersistedSentChatMessage[] =>
  recentSentChatMessages(Array.isArray(value) ? value as RecentHistoryMessage[] : []);
