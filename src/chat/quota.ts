export type ChatQuotaBucket = {
  usedPercent?: number | null;
  rateLimitReachedType?: string | null;
};

export type ChatAccountQuota = {
  id: string;
  hasTokens: boolean;
  /** Erreur de lecture du quota renvoyee par le provider, si presente. */
  error?: string | null;
  sessionUsedPercent?: number | null;
  weeklyUsedPercent?: number | null;
  buckets?: ChatQuotaBucket[];
};

export type BestQuotaAccount<T extends ChatAccountQuota> = {
  account: T;
  remainingPercent: number;
};

export const OPEN_CHAT_QUOTA_RESERVATION_PERCENT = 20;

export type BestNewChatQuotaAccount<T extends ChatAccountQuota> =
  BestQuotaAccount<T> & {
    effectiveRemainingPercent: number;
    openChatCount: number;
    reservedPercent: number;
  };

export type ChatQuotaTurn = {
  id: number;
  accountId: string;
  status: string;
};

export type QuotaDisplayAccount = {
  id: string;
  codexHome?: string | null;
  provider?: string | null;
  hasTokens?: boolean;
  source?: string | null;
  error?: string | null;
  refreshedAt?: number | null;
};

const normalizedError = (value: string): string =>
  value
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase();

/**
 * Un fichier de jetons peut encore exister alors que le provider refuse la
 * session (401, token revoque, reconnexion requise). Un ancien pourcentage en
 * cache ne doit jamais rendre ce compte eligible a une nouvelle conversation
 * ou a une reprise automatique.
 */
export const isQuotaAuthenticationError = (
  error: string | null | undefined,
): boolean => {
  if (!error?.trim()) return false;
  return /token[_ ]?invalidat|refresh[_ ]?token|revoked|revoqu|\b401\b|\b403\b|unauthor|authentication|session (?:has )?ended|sign(?:ing)? ?in again|log ?in again|not logged in|connexion requise|authentication required/i
    .test(error);
};

const quotaAccountIsSelectable = (account: ChatAccountQuota): boolean =>
  account.hasTokens && !isQuotaAuthenticationError(account.error);

/**
 * Distingue un quota de compte epuise d'une limite de contexte ou d'un prompt
 * trop long : seules les premieres doivent proposer de changer de compte.
 */
export const isQuotaExhaustionError = (error: string | null | undefined): boolean => {
  if (!error?.trim()) return false;
  const value = normalizedError(error);
  if (
    /context (?:window|length)|maximum context|prompt (?:is )?too long|message (?:is )?too (?:large|long)/.test(
      value,
    )
  ) {
    return false;
  }

  return [
    /(?:rate|usage)[ _-]?limit(?:ed| has been)? (?:exceeded|reached)/,
    /(?:rate|usage)[ _-]?limit/,
    /you(?:'ve| have) hit your (?:usage|rate) limit/,
    /too many requests/,
    /insufficient[ _-]?quota/,
    /\bquota\b.{0,50}(?:exceeded|exhausted|epuise|atteint|reached)/,
    /(?:exceeded|exhausted|epuise|atteint|reached).{0,50}\bquota\b/,
    /(?:no|zero|0) (?:weighted )?tokens? (?:left|remaining)/,
    /out of (?:weighted )?tokens?/,
    /plus de (?:jetons?|tokens?)/,
    /(?:jetons?|tokens?).{0,30}(?:epuise|indisponible)/,
    /(?:http(?: status)?\s*)?429\b/,
  ].some((pattern) => pattern.test(value));
};

const validPercent = (value: number | null | undefined): value is number =>
  typeof value === "number" && Number.isFinite(value);

const quotaDisplayIdentity = (account: QuotaDisplayAccount): string => {
  const home = account.codexHome
    ?.trim()
    .replace(/\\/g, "/")
    .replace(/\/+$/, "")
    .toLowerCase();
  return home
    ? `${account.provider?.toLowerCase() || "codex"}:${home}`
    : `profile:${account.id}`;
};

const quotaDisplayQuality = (account: QuotaDisplayAccount): number => {
  const reliableSource = [
    "server",
    "session",
    "claude-usage",
    "authenticated",
  ].includes(account.source ?? "");
  return (
    (account.hasTokens ? 4 : 0)
    + (reliableSource ? 2 : 0)
    + (account.error ? 0 : 1)
  );
};

/**
 * Plusieurs profils peuvent pointer vers le même CODEX_HOME. Pour les totaux
 * agrégés du bandeau de chat, leur quota commun ne doit être compté qu'une
 * fois. La vue Limites, elle, affiche volontairement chaque profil.
 */
export const deduplicateQuotaAccountsForDisplay = <T extends QuotaDisplayAccount>(
  accounts: T[],
): T[] => {
  const rows: T[] = [];
  const positions = new Map<string, number>();

  for (const account of accounts) {
    const key = quotaDisplayIdentity(account);
    const existingIndex = positions.get(key);
    if (existingIndex === undefined) {
      positions.set(key, rows.length);
      rows.push(account);
      continue;
    }
    const existing = rows[existingIndex];
    const quality = quotaDisplayQuality(account);
    const existingQuality = quotaDisplayQuality(existing);
    if (
      quality > existingQuality
      || (quality === existingQuality
        && (account.refreshedAt ?? 0) > (existing.refreshedAt ?? 0))
    ) {
      rows[existingIndex] = account;
    }
  }

  return rows;
};

/**
 * Le quota utilisable d'un compte est son goulot d'etranglement : si le quota
 * 5 h est libre a 80 % mais l'hebdomadaire a 20 %, il reste effectivement 20 %.
 */
export const remainingQuotaPercent = (account: ChatAccountQuota): number | null => {
  if (!account.hasTokens) return null;
  if (account.buckets?.some((bucket) => bucket.rateLimitReachedType?.trim())) return 0;

  const used = [
    account.sessionUsedPercent,
    account.weeklyUsedPercent,
    ...(account.buckets ?? []).map((bucket) => bucket.usedPercent),
  ].filter(validPercent);
  if (!used.length) return null;
  return Math.max(0, Math.min(100, 100 - Math.max(...used)));
};

export type CombinedQuotaUsage = {
  usedPercent: number | null;
  remainingPercent: number | null;
  measuredAccountCount: number;
};

/**
 * Combine la capacite de plusieurs comptes en donnant le meme poids a chacun.
 * Les comptes sans quota lisible sont signales par le compteur, mais ne
 * faussent pas la moyenne affichee.
 */
export const combinedQuotaUsage = (
  accounts: readonly ChatAccountQuota[],
): CombinedQuotaUsage => {
  const remaining = accounts
    .map(remainingQuotaPercent)
    .filter((value): value is number => value !== null);
  if (remaining.length === 0) {
    return {
      usedPercent: null,
      remainingPercent: null,
      measuredAccountCount: 0,
    };
  }

  const remainingPercent = remaining.reduce((total, value) => total + value, 0)
    / remaining.length;
  return {
    usedPercent: Math.max(0, Math.min(100, 100 - remainingPercent)),
    remainingPercent,
    measuredAccountCount: remaining.length,
  };
};

const normalizedOpenChatCount = (value: number): number =>
  Number.isFinite(value) ? Math.max(0, Math.floor(value)) : 0;

/**
 * Un chat ouvert reserve virtuellement 20 points de quota. Cette reservation
 * ne modifie pas le quota serveur : elle sert uniquement a repartir les
 * nouveaux chats entre les comptes disponibles.
 */
export const quotaAfterOpenChatReservations = (
  remainingPercent: number,
  openChatCount: number,
  reservationPercent = OPEN_CHAT_QUOTA_RESERVATION_PERCENT,
): { effectiveRemainingPercent: number; reservedPercent: number } => {
  const normalizedReservation = Number.isFinite(reservationPercent)
    ? Math.max(0, reservationPercent)
    : OPEN_CHAT_QUOTA_RESERVATION_PERCENT;
  const normalizedRemaining = Number.isFinite(remainingPercent)
    ? Math.max(0, Math.min(100, remainingPercent))
    : 0;
  const reservedPercent = normalizedOpenChatCount(openChatCount) * normalizedReservation;
  return {
    effectiveRemainingPercent: Math.max(
      0,
      normalizedRemaining - reservedPercent,
    ),
    reservedPercent,
  };
};

/**
 * Choisit le compte le plus disponible pour un nouveau chat. Chaque chat deja
 * ouvert sur un compte retranche 20 points a son quota restant afin d'eviter
 * de concentrer toutes les conversations sur le meme compte.
 */
export const bestQuotaAccountForNewChat = <T extends ChatAccountQuota>(
  accounts: T[],
  eligibleAccountIds: Iterable<string>,
  openChatAccountIds: Iterable<string>,
): BestNewChatQuotaAccount<T> | null => {
  const eligible = new Set(eligibleAccountIds);
  const openChatCounts = new Map<string, number>();
  for (const accountId of openChatAccountIds) {
    openChatCounts.set(accountId, (openChatCounts.get(accountId) ?? 0) + 1);
  }

  let best: BestNewChatQuotaAccount<T> | null = null;
  for (const account of accounts) {
    if (!eligible.has(account.id) || !quotaAccountIsSelectable(account)) continue;
    const remainingPercent = remainingQuotaPercent(account);
    if (remainingPercent === null || remainingPercent <= 0) continue;
    const openChatCount = openChatCounts.get(account.id) ?? 0;
    const { effectiveRemainingPercent, reservedPercent } =
      quotaAfterOpenChatReservations(remainingPercent, openChatCount);
    if (effectiveRemainingPercent <= 0) continue;

    const candidate = {
      account,
      remainingPercent,
      effectiveRemainingPercent,
      openChatCount,
      reservedPercent,
    };
    if (
      !best
      || candidate.effectiveRemainingPercent > best.effectiveRemainingPercent
      || (
        candidate.effectiveRemainingPercent === best.effectiveRemainingPercent
        && candidate.remainingPercent > best.remainingPercent
      )
      || (
        candidate.effectiveRemainingPercent === best.effectiveRemainingPercent
        && candidate.remainingPercent === best.remainingPercent
        && candidate.openChatCount < best.openChatCount
      )
    ) {
      best = candidate;
    }
  }

  return best;
};

/**
 * Un tour deja lance doit etre repris ailleurs lorsque la lecture serveur
 * confirme que son compte est a zero. L'identifiant 0 est uniquement l'etat
 * optimiste du frontend : il n'existe pas encore de processus backend a
 * interrompre.
 */
export const shouldRecoverRunningQuotaTurn = (
  turn: ChatQuotaTurn | null | undefined,
  accounts: ChatAccountQuota[],
): boolean => {
  if (!turn || turn.id <= 0 || turn.status !== "running") return false;
  const account = accounts.find((candidate) => candidate.id === turn.accountId);
  return !!account && remainingQuotaPercent(account) === 0;
};

/** Choisit le compte compatible qui possede le plus de quota reel restant. */
export const bestQuotaAccount = <T extends ChatAccountQuota>(
  accounts: T[],
  currentAccountId: string,
  eligibleAccountIds: Iterable<string>,
): BestQuotaAccount<T> | null => {
  const eligible = new Set(eligibleAccountIds);
  let best: BestQuotaAccount<T> | null = null;

  for (const account of accounts) {
    if (
      account.id === currentAccountId
      || !eligible.has(account.id)
      || !quotaAccountIsSelectable(account)
    ) continue;
    const remainingPercent = remainingQuotaPercent(account);
    if (remainingPercent === null || remainingPercent <= 0) continue;
    if (!best || remainingPercent > best.remainingPercent) {
      best = { account, remainingPercent };
    }
  }

  return best;
};
