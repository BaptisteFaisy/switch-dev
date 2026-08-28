export type ChatTokenUsagePresentation = {
  count: number | null;
  value: string;
  unit: "token" | "tokens";
  title: string;
  pressure: ChatContextPressure;
  usedPercent: number | null;
  signature: string;
};

export type ChatContextWindowUsage = {
  usedTokens: number;
  contextWindow: number;
  remainingTokens: number;
  usedPercent: number;
};

export type ChatContextPressure = "safe" | "warning" | "danger" | "unknown";

export const CHAT_CONTEXT_WARNING_PERCENT = 60;
export const CHAT_CONTEXT_DANGER_PERCENT = 80;
export const CHAT_AUTO_COMPACT_THRESHOLD_TOKENS = 400_000;

const CHAT_TOKEN_NUMBER_FORMAT = new Intl.NumberFormat("fr-FR", {
  maximumFractionDigits: 0,
});

export const normalizeChatTokenCount = (value: unknown): number | null => {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) return null;
  return Math.round(value);
};

export const normalizeChatContextUsage = (
  value: unknown,
): ChatContextWindowUsage | null => {
  if (!value || typeof value !== "object") return null;
  const candidate = value as Partial<ChatContextWindowUsage>;
  const usedTokens = normalizeChatTokenCount(candidate.usedTokens);
  const contextWindow = normalizeChatTokenCount(candidate.contextWindow);
  const usedPercent = normalizeChatTokenCount(candidate.usedPercent);
  if (
    usedTokens === null
    || contextWindow === null
    || contextWindow <= 0
    || usedPercent === null
  ) {
    return null;
  }
  return {
    usedTokens,
    contextWindow,
    remainingTokens: Math.max(
      0,
      normalizeChatTokenCount(candidate.remainingTokens)
        ?? contextWindow - usedTokens,
    ),
    usedPercent: Math.max(0, Math.min(100, usedPercent)),
  };
};

export const chatContextPressure = (
  usage: ChatContextWindowUsage | null | undefined,
): ChatContextPressure => {
  const normalized = normalizeChatContextUsage(usage);
  if (!normalized) return "unknown";
  if (normalized.usedPercent >= CHAT_CONTEXT_DANGER_PERCENT) return "danger";
  if (normalized.usedPercent >= CHAT_CONTEXT_WARNING_PERCENT) return "warning";
  return "safe";
};

export const isCompactSlashCommand = (value: unknown): boolean =>
  typeof value === "string" && /^\/compact\s*$/i.test(value.trim());

export const shouldAutoCompactChatContext = (value: unknown): boolean => {
  const usage = normalizeChatContextUsage(value);
  return !!usage && usage.usedTokens > CHAT_AUTO_COMPACT_THRESHOLD_TOKENS;
};

export const chatTokenUsagePresentation = (
  totalTokens: unknown,
  contextUsage: unknown = null,
): ChatTokenUsagePresentation => {
  const context = normalizeChatContextUsage(contextUsage);
  if (context) {
    const pressure = chatContextPressure(context);
    const used = CHAT_TOKEN_NUMBER_FORMAT.format(context.usedTokens);
    const window = CHAT_TOKEN_NUMBER_FORMAT.format(context.contextWindow);
    const advice = pressure === "danger"
      ? "Contexte presque plein : lancez /compact maintenant."
      : pressure === "warning"
        ? "Contexte chargé : pensez à lancer /compact."
        : "Contexte sain : aucune compaction nécessaire.";
    return {
      count: context.usedTokens,
      value: `${used} / ${window}`,
      unit: "tokens",
      title: `${used} tokens présents sur ${window} (${context.usedPercent} % de la fenêtre exploitable utilisés). ${advice}`,
      pressure,
      usedPercent: context.usedPercent,
      signature: `context:${context.usedTokens}:${context.contextWindow}:${context.usedPercent}`,
    };
  }

  const count = normalizeChatTokenCount(totalTokens);
  if (count === null) {
    return {
      count: null,
      value: "—",
      unit: "tokens",
      title: "Nombre de tokens indisponible pour ce chat",
      pressure: "unknown",
      usedPercent: null,
      signature: "unavailable",
    };
  }

  const value = CHAT_TOKEN_NUMBER_FORMAT.format(count);
  const singular = count === 1;
  return {
    count,
    value,
    unit: singular ? "token" : "tokens",
    title: `${value} ${singular ? "token utilisé" : "tokens utilisés"} dans ce chat`,
    pressure: "unknown",
    usedPercent: null,
    signature: `total:${count}`,
  };
};
