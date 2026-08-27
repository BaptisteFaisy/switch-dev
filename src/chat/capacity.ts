const normalizeCapacityError = (value: string): string =>
  value
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase();

/** Delai avant de retenter une demande refusee par le garde-fou memoire. */
export const CHAT_RESOURCE_RETRY_DELAY_MS = 5_000;

/**
 * Saturation du noeud local (distincte de la saturation d'un modele distant).
 * Ces rejets ne doivent pas echouer le message : l'UI le conserve dans sa file.
 */
export const isNodeCapacityError = (
  error: string | null | undefined,
): boolean => {
  if (!error?.trim()) return false;
  const value = normalizeCapacityError(error);
  return (
    value.includes("capacite chats atteinte") ||
    value.includes("cst node capacity reached") ||
    value.includes("aucun noeud de chat disponible") ||
    value.includes("memoire insuffisante")
  );
};

/**
 * Reconnait uniquement une saturation du modele, pas un quota de compte ni la
 * capacite d'un noeud local. Le texte exact est celui actuellement emis par
 * Codex ; les variantes "overloaded" couvrent le meme echec cote provider.
 */
export const isModelCapacityError = (
  error: string | null | undefined,
): boolean => {
  if (!error?.trim()) return false;
  const value = normalizeCapacityError(error);
  return (
    value.includes("selected model is at capacity") ||
    /\bmodel\b.{0,80}\b(?:at capacity|overloaded)\b/.test(value) ||
    /\b(?:at capacity|overloaded)\b.{0,80}\bmodel\b/.test(value)
  );
};

/**
 * Reconnait une coupure de flux transitoire (drop reseau/API en cours de
 * reponse), distincte d'une saturation modele ou d'un quota de compte. Le CLI
 * Claude affiche « API Error: Connection closed mid-response » ; on couvre aussi
 * les variantes reseau frequentes. Volontairement etroit pour ne pas rejouer
 * une vraie erreur applicative.
 */
export const isTransientStreamError = (
  error: string | null | undefined,
): boolean => {
  if (!error?.trim()) return false;
  const value = normalizeCapacityError(error);
  return (
    value.includes("connection closed") ||
    value.includes("response above may be incomplete") ||
    value.includes("connection reset") ||
    value.includes("econnreset") ||
    value.includes("socket hang up")
  );
};

export const MODEL_CAPACITY_RETRY_LIMIT = 3;
/** Reprise automatique plus prudente que la saturation (au plus 2 essais). */
export const TRANSIENT_STREAM_RETRY_LIMIT = 2;
export const MODEL_CAPACITY_CONTINUE_PROMPT = "continue";

/** Delais 3 s, 6 s puis 12 s pour ne pas marteler un modele sature. */
export const modelCapacityRetryDelayMs = (attempt: number): number => {
  const normalizedAttempt = Math.max(1, Math.floor(attempt));
  return Math.min(12_000, 3_000 * 2 ** (normalizedAttempt - 1));
};

/**
 * Une session existante a deja conserve la demande qui a echoue : "continue"
 * reproduit la reprise manuelle sans dupliquer tout le prompt. Si le provider
 * a echoue avant de creer la session, il faut en revanche rejouer la demande.
 */
export const modelCapacityRetryPrompt = (
  resumeSessionId: string | null | undefined,
  originalPrompt: string,
): string => resumeSessionId ? MODEL_CAPACITY_CONTINUE_PROMPT : originalPrompt;
