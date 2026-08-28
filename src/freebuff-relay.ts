// Relais Freebuff -----------------------------------------------------------
// Un chat dont chaque message est transmis au TUI Freebuff ouvert dans un
// terminal Switch : le texte est ecrit dans le PTY (suivi d'un Enter) et la
// sortie du terminal est renvoyee dans le fil comme reponse de l'assistant.
// Le terminal Freebuff ne peut pas etre lance avec un prompt en argument
// (voir chat.rs) : ce relais est donc le seul moyen de piloter son TUI depuis
// une interface de chat.

export const FREEBUFF_RELAY_STORAGE_KEY = "codex-switch-terminal.freebuff-relay.v1";
export const FREEBUFF_RELAY_MESSAGE_MAX_LENGTH = 50_000;
// Au-dela de cette duree sans aucune sortie du TUI apres un envoi, l'echange
// passe en statut « bloque » : le terminal semble arrete.
export const FREEBUFF_RELAY_STALL_THRESHOLD_MS = 3 * 60_000;
// Cadence de verification du chien de garde (bien plus fine que le seuil).
export const FREEBUFF_RELAY_STALL_CHECK_INTERVAL_MS = 15_000;
// Nombre d'echanges conserves dans l'historique (les plus anciens disparaissent).
export const FREEBUFF_RELAY_MAX_EXCHANGES = 60;
// Taille de sortie stockee par echange (localStorage). L'affichage peut
// montrer un peu plus, en gardant la fin du flux.
export const FREEBUFF_RELAY_STORED_OUTPUT_LIMIT = 12_000;
export const FREEBUFF_RELAY_RENDERED_OUTPUT_LIMIT = 60_000;
// Taille maximale du tampon brut en memoire pendant l'envoi en cours.
export const FREEBUFF_RELAY_RAW_BUFFER_LIMIT = 200_000;

export type FreebuffRelayExchangeStatus = "sending" | "sent" | "failed" | "stalled";

export type FreebuffRelayExchange = {
  id: string;
  userText: string;
  sentAt: number;
  output: string;
  status: FreebuffRelayExchangeStatus;
  error: string | null;
  /** true : le texte est valide par Entree ; false : envoye tel quel (menus interactifs). */
  withEnter: boolean;
};

export type FreebuffRelayState = {
  /** Cle du terminal session Freebuff cible. */
  targetKey: string | null;
  exchanges: FreebuffRelayExchange[];
};

export type FreebuffRelayStorage = Pick<Storage, "getItem" | "setItem">;

export type FreebuffRelaySession = {
  key: string;
  title: string;
  accountLabel: string;
  accountId: string;
  model: string;
  ptyId: number | null;
  running: boolean;
};

export type FreebuffRelayBridge = {
  /** Terminaux Freebuff ouverts (avec PTY pilotable) proposes comme cibles. */
  sessions: () => FreebuffRelaySession[];
  /** Ecrit dans le PTY. Renvoie false si l'ecriture a echoue. */
  write: (ptyId: number, data: string) => Promise<boolean>;
  /** Flux de sortie du terminal, filtre par le module sur la cible choisie. */
  subscribeOutput: (listener: (key: string, data: string) => void) => () => void;
  /** Bascule vers la vue Terminaux (etat vide). */
  onOpenTerminalView?: () => void;
  /** Modeles Freebuff acceptes par le validateur du TUI (liste du selecteur). */
  models?: () => string[];
  /**
   * Applique un nouveau modele au terminal cible : le modele est enregistre
   * sur le compte, la config reecrite, puis le TUI redemarre sur la
   * conversation en cours. Renvoie false (avec motif) si inapplicable.
   */
  applyModel?: (sessionKey: string, model: string) => Promise<{
    ok: boolean;
    error?: string | null;
  }>;
  /**
   * Modele effectivement actif sur le TUI, relu depuis le settings.json du
   * canal manicode. `null` si la cle est absente (le TUI applique son defaut)
   * ou si la lecture a echoue.
   */
  activeModel?: (sessionKey: string) => Promise<string | null>;
};

export type FreebuffRelayPanelOptions = {
  storage?: FreebuffRelayStorage | null;
  renderIcons?: (root: ParentNode) => void;
  bridge?: FreebuffRelayBridge | null;
  /** Cible imposee avant l'ouverture (bouton du panneau terminal). */
  pendingTargetKey?: string | null;
  /** Etat initial du mode « Valider par Entrée » (defaut : etat vivant du module). */
  enterMode?: boolean;
  /** Seuil de blocage (defaut : 3 minutes sans sortie apres un envoi). */
  stallThresholdMs?: number;
};

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

const normalizeSingleLine = (value: unknown, maxLength: number): string =>
  typeof value === "string"
    ? value.trim().replace(/\s+/g, " ").slice(0, maxLength)
    : "";

const finiteTimestamp = (value: unknown, fallback: number): number =>
  typeof value === "number" && Number.isFinite(value) && value >= 0
    ? Math.floor(value)
    : fallback;

const normalizeRelayMessage = (value: unknown): string =>
  typeof value === "string"
    ? value.replace(/\r\n?/g, "\n").trim().slice(0, FREEBUFF_RELAY_MESSAGE_MAX_LENGTH)
    : "";

const normalizeRelayOutput = (value: unknown): string =>
  typeof value === "string"
    ? value.replace(/\r\n?/g, "\n").slice(0, FREEBUFF_RELAY_STORED_OUTPUT_LIMIT)
    : "";

const normalizeRelayExchangeStatus = (value: unknown): FreebuffRelayExchangeStatus =>
  value === "sending" || value === "failed" || value === "stalled" ? value : "sent";

const normalizeRelayExchange = (
  candidate: unknown,
  fallbackTimestamp: number,
): FreebuffRelayExchange | null => {
  if (!isRecord(candidate)) return null;
  const id = normalizeSingleLine(candidate.id, 180);
  const userText = normalizeRelayMessage(candidate.userText);
  if (!id || !userText) return null;
  const sentAt = finiteTimestamp(candidate.sentAt, fallbackTimestamp);
  return {
    id,
    userText,
    sentAt,
    output: normalizeRelayOutput(candidate.output),
    status: normalizeRelayExchangeStatus(candidate.status),
    error:
      candidate.status === "failed"
        ? normalizeSingleLine(candidate.error, 1_000) || "L'envoi au terminal a échoué."
        : null,
    // Historique anterieur sans champ : toujours valide par Entree.
    withEnter: candidate.withEnter !== false,
  };
};

export const normalizeFreebuffRelayState = (
  value: unknown,
  fallbackTimestamp = Date.now(),
): FreebuffRelayState => {
  if (!isRecord(value)) {
    return { targetKey: null, exchanges: [] };
  }
  const seen = new Set<string>();
  const exchanges: FreebuffRelayExchange[] = [];
  if (Array.isArray(value.exchanges)) {
    value.exchanges.forEach((candidate) => {
      const exchange = normalizeRelayExchange(candidate, fallbackTimestamp);
      if (!exchange || seen.has(exchange.id)) return;
      seen.add(exchange.id);
      exchanges.push(exchange);
    });
  }
  return {
    targetKey: normalizeSingleLine(value.targetKey, 180) || null,
    exchanges: exchanges.slice(-FREEBUFF_RELAY_MAX_EXCHANGES),
  };
};

const browserFreebuffRelayStorage = (): FreebuffRelayStorage | null => {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage;
  } catch {
    return null;
  }
};

const resolveRelayStorage = (
  storage: FreebuffRelayStorage | null | undefined,
): FreebuffRelayStorage | null => storage === undefined ? browserFreebuffRelayStorage() : storage;

export const loadFreebuffRelayState = (
  storage?: FreebuffRelayStorage | null,
): FreebuffRelayState => {
  const target = resolveRelayStorage(storage);
  if (!target) return { targetKey: null, exchanges: [] };
  try {
    const serialized = target.getItem(FREEBUFF_RELAY_STORAGE_KEY);
    return serialized ? normalizeFreebuffRelayState(JSON.parse(serialized)) : { targetKey: null, exchanges: [] };
  } catch {
    return { targetKey: null, exchanges: [] };
  }
};

export const persistFreebuffRelayState = (
  state: FreebuffRelayState,
  storage?: FreebuffRelayStorage | null,
): boolean => {
  const target = resolveRelayStorage(storage);
  if (!target) return false;
  try {
    target.setItem(FREEBUFF_RELAY_STORAGE_KEY, JSON.stringify(normalizeFreebuffRelayState(state)));
    return true;
  } catch {
    return false;
  }
};

const createRelayExchangeId = (timestamp: number): string => {
  const randomId = globalThis.crypto?.randomUUID?.();
  return randomId
    ?? `freebuff-relay-${timestamp.toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
};

export const setFreebuffRelayTarget = (
  state: FreebuffRelayState,
  targetKey: string | null,
): FreebuffRelayState => ({
  ...state,
  targetKey: normalizeSingleLine(targetKey, 180) || null,
});

export const addFreebuffRelayExchange = (
  state: FreebuffRelayState,
  userText: string,
  timestamp = Date.now(),
  id = createRelayExchangeId(timestamp),
  withEnter = true,
): FreebuffRelayState => {
  const normalized = normalizeRelayMessage(userText);
  if (!normalized) return state;
  const uniqueId = state.exchanges.some((exchange) => exchange.id === id)
    ? createRelayExchangeId(timestamp + 1)
    : id;
  const exchange: FreebuffRelayExchange = {
    id: uniqueId,
    userText: normalized,
    sentAt: Math.floor(timestamp),
    output: "",
    status: "sending",
    error: null,
    withEnter,
  };
  return {
    ...state,
    exchanges: [...state.exchanges, exchange].slice(-FREEBUFF_RELAY_MAX_EXCHANGES),
  };
};

/**
 * Construit la charge envoyee au PTY selon le mode choisi :
 * - avec Entree : chaque ligne est validee par un retour chariot, comme une
 *   saisie reelle dans le TUI ;
 * - sans Entree : le texte est ecrit tel quel (champ de saisie, menus
 *   interactifs pilotes pas a pas).
 */
export const freebuffRelayPayload = (text: string, withEnter: boolean): string => {
  const normalized = normalizeRelayMessage(text);
  if (!normalized) return "";
  return withEnter ? `${normalized.replace(/\n/g, "\r")}\r` : normalized;
};

/**
 * Marque un echange « bloque » : le terminal n'a rien emis depuis l'envoi
 * au-dela du seuil. Ne s'applique qu'a un echange en attente de reponse
 * (statut « sent ») : un echec ou un blocage deja signale ne bouge plus.
 */
export const markFreebuffRelayExchangeStalled = (
  state: FreebuffRelayState,
  id: string,
  timestamp = Date.now(),
): FreebuffRelayState => ({
  ...state,
  exchanges: state.exchanges.map((exchange) =>
    exchange.id === id && exchange.status === "sent"
      ? {
          ...exchange,
          status: "stalled",
          sentAt: Math.floor(timestamp),
          error: "Aucune activité du terminal depuis plusieurs minutes : il est peut-être bloqué.",
        }
      : exchange,
  ),
});

/**
 * Decide si l'echange doit passer en « bloque » : aucune sortie du terminal
 * depuis `lastActivityAt` au-dela du seuil. Fonction pure (testable) utilisee
 * par le chien de garde du panneau.
 */
export const resolveFreebuffRelayStall = (
  state: FreebuffRelayState,
  exchangeId: string,
  lastActivityAt: number | null,
  now = Date.now(),
  thresholdMs = FREEBUFF_RELAY_STALL_THRESHOLD_MS,
): FreebuffRelayState => {
  if (lastActivityAt === null || !Number.isFinite(lastActivityAt)) return state;
  if (now - lastActivityAt < thresholdMs) return state;
  return markFreebuffRelayExchangeStalled(state, exchangeId, now);
};

export const markFreebuffRelayExchangeSent = (
  state: FreebuffRelayState,
  id: string,
  timestamp = Date.now(),
): FreebuffRelayState => ({
  ...state,
  exchanges: state.exchanges.map((exchange) =>
    exchange.id === id
      ? { ...exchange, status: "sent", sentAt: Math.floor(timestamp), error: null }
      : exchange,
  ),
});

export const markFreebuffRelayExchangeFailed = (
  state: FreebuffRelayState,
  id: string,
  error: unknown,
  timestamp = Date.now(),
): FreebuffRelayState => ({
  ...state,
  exchanges: state.exchanges.map((exchange) =>
    exchange.id === id
      ? {
          ...exchange,
          status: "failed",
          sentAt: Math.floor(timestamp),
          error:
            normalizeSingleLine(error instanceof Error ? error.message : String(error), 1_000)
            || "L'envoi au terminal a échoué.",
        }
      : exchange,
  ),
});

export const appendFreebuffRelayOutput = (
  state: FreebuffRelayState,
  id: string,
  data: string,
): FreebuffRelayState => ({
  ...state,
  exchanges: state.exchanges.map((exchange) => {
    if (exchange.id !== id) return exchange;
    const output = `${exchange.output}${data}`.replace(/\r\n?/g, "\n");
    return {
      ...exchange,
      output: output.slice(-FREEBUFF_RELAY_STORED_OUTPUT_LIMIT),
    };
  }),
});

export const clearFreebuffRelayHistory = (
  state: FreebuffRelayState,
): FreebuffRelayState => ({ ...state, exchanges: [] });

export const latestFreebuffRelayExchange = (
  state: FreebuffRelayState,
): FreebuffRelayExchange | null => state.exchanges[state.exchanges.length - 1] ?? null;

/**
 * Nettoie la sortie brute d'un TUI pour l'affichage : les sequences ANSI
 * (couleurs, curseur, OSC) sont retirees, les retours a la ligne normalises.
 */
export const cleanFreebuffRelayOutput = (value: string): string =>
  value
    .replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, "")
    .replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, "")
    .replace(/\x1b[()][0-9A-Za-z]/g, "")
    .replace(/\x1b[=>]/g, "")
    .replace(/\r\n/g, "\n")
    .replace(/\r/g, "\n")
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, "");

const escapeHtml = (value: string): string =>
  value.replace(/[&<>"']/g, (character) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[character] ?? character);

const escapeAttr = (value: string): string =>
  value.replace(/[&"']/g, (character) => ({
    "&": "&amp;",
    '"': "&quot;",
    "'": "&#39;",
  })[character] ?? character);

const formatRelayTime = (timestamp: number): string => {
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return "";
  return new Intl.DateTimeFormat("fr-FR", {
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
};

const relayStatusPresentation: Record<FreebuffRelayExchangeStatus, {
  label: string;
  icon: string;
  tone: string;
}> = {
  sending: { label: "Envoi…", icon: "loader-circle", tone: "is-sending" },
  sent: { label: "Envoyé", icon: "circle-check", tone: "is-sent" },
  failed: { label: "Échec", icon: "triangle-alert", tone: "is-failed" },
  stalled: { label: "Bloqué · silencieux", icon: "triangle-alert", tone: "is-stalled" },
};

const renderRelayExchange = (exchange: FreebuffRelayExchange): string => {
  const status = relayStatusPresentation[exchange.status];
  const output = cleanFreebuffRelayOutput(exchange.output);
  return `
    <article class="freebuff-relay-exchange status-${exchange.status}" data-freebuff-relay-exchange="${escapeAttr(exchange.id)}">
      <div class="freebuff-relay-user">
        <span class="freebuff-relay-user-label"><i data-lucide="user-round"></i>Vous · ${formatRelayTime(exchange.sentAt)}${exchange.withEnter ? "" : `<span class="freebuff-relay-raw-badge" title="Envoyé sans validation : pour les menus interactifs">sans Entrée</span>`}</span>
        <p>${escapeHtml(exchange.userText)}</p>
      </div>
      <div class="freebuff-relay-output">
        <header>
          <span><i data-lucide="square-terminal"></i>Sortie du terminal</span>
          <span class="freebuff-relay-status ${status.tone}"><i data-lucide="${status.icon}"></i>${status.label}</span>
        </header>
        ${exchange.error ? `<div class="freebuff-relay-error"><i data-lucide="triangle-alert"></i><span>${escapeHtml(exchange.error)}</span></div>` : ""}
        ${output
          ? `<pre>${escapeHtml(output.slice(-FREEBUFF_RELAY_RENDERED_OUTPUT_LIMIT))}</pre>`
          : `<p class="freebuff-relay-output-empty">${exchange.status === "sending" ? "Envoi en cours…" : "Aucune sortie reçue."}</p>`}
      </div>
    </article>`;
};

const renderRelayTargetSelect = (
  state: FreebuffRelayState,
  sessions: readonly FreebuffRelaySession[],
): string => {
  const options = sessions.map((session) => {
    const label = session.ptyId !== null
      ? `${session.title} · ${session.accountLabel}`
      : `${session.title} · ${session.accountLabel} (en démarrage)`;
    return `<option value="${escapeAttr(session.key)}" ${session.key === state.targetKey ? "selected" : ""}>${escapeHtml(label)}</option>`;
  });
  return `<select id="freebuffRelayTarget" aria-label="Terminal Freebuff cible">
    ${options.length ? options.join("") : `<option value="">Aucun terminal Freebuff ouvert</option>`}
  </select>`;
};

const renderRelayModelSelect = (
  state: FreebuffRelayState,
  sessions: readonly FreebuffRelaySession[],
  models: readonly string[],
  applying: boolean,
): string => {
  if (!models.length) return "";
  const target = sessions.find((session) => session.key === state.targetKey) ?? null;
  if (!target) return "";
  const options = models.map((model) =>
    `<option value="${escapeAttr(model)}" ${model === target.model ? "selected" : ""}>${escapeHtml(model)}</option>`,
  ).join("");
  return `<label class="freebuff-relay-model-field" title="Modèle Freebuff de ce terminal : le choix enregistre le modèle et redémarre le TUI sur la conversation en cours">
    <span><i data-lucide="sparkles"></i>Modèle</span>
    <select data-freebuff-relay-model aria-label="Modèle Freebuff du terminal cible" ${applying ? "disabled" : ""}>${options}</select>
  </label>`;
};

const renderRelayTargetState = (
  state: FreebuffRelayState,
  sessions: readonly FreebuffRelaySession[],
): string => {
  const target = sessions.find((session) => session.key === state.targetKey) ?? null;
  if (!target) {
    return `<span class="freebuff-relay-target-state is-off"><i data-lucide="circle-slash"></i><span>Choisis un terminal Freebuff ouvert pour commencer.</span></span>`;
  }
  if (!target.running || target.ptyId === null) {
    return `<span class="freebuff-relay-target-state is-off"><i data-lucide="loader-circle"></i><span>Terminal en cours de démarrage…</span></span>`;
  }
  return `<span class="freebuff-relay-target-state is-on"><i data-lucide="radio"></i><span>${escapeHtml(target.title)} · PTY ${target.ptyId} · prêt à recevoir</span></span>`;
};

/**
 * Modele effectivement actif sur le TUI, relu depuis le settings.json du
 * canal manicode. Signale quand le binaire a normalise un modele different
 * de celui demande cote Switch.
 */
export const renderRelayActiveModel = (
  target: FreebuffRelaySession | null,
  active: string | null,
  pending: boolean,
  available: boolean,
): string => {
  if (!available || !target) return "";
  if (pending) {
    return `<span class="freebuff-relay-active-model is-pending"><i data-lucide="loader-circle"></i><span>Lecture du modèle actif…</span></span>`;
  }
  if (active) {
    const normalized = active !== target.model;
    return `<span class="freebuff-relay-active-model ${normalized ? "is-warning" : "is-ok"}"><i data-lucide="${normalized ? "triangle-alert" : "circle-check"}"></i><span>${normalized
      ? `Modèle actif : <strong>${escapeHtml(active)}</strong> — le TUI a normalisé le modèle demandé (<em>${escapeHtml(target.model)}</em>).`
      : `Modèle actif : <strong>${escapeHtml(active)}</strong> — confirmé dans le TUI.`}</span></span>`;
  }
  return `<span class="freebuff-relay-active-model is-off"><i data-lucide="info"></i><span>Modèle actif : défaut du TUI (non épinglé dans settings.json).</span></span>`;
};

export const renderFreebuffRelayPanel = (
  options: FreebuffRelayPanelOptions = {},
): string => {
  const state = loadFreebuffRelayState(options.storage);
  const sessions = options.bridge?.sessions() ?? [];
  const pendingTarget = options.pendingTargetKey?.trim() || null;
  const storedTarget = sessions.some((session) => session.key === state.targetKey)
    ? state.targetKey
    : null;
  const effectiveTarget = pendingTarget ?? storedTarget ?? sessions[0]?.key ?? null;
  const hasSessions = sessions.length > 0;
  const targetReady = hasSessions && sessions.some(
    (session) => session.key === effectiveTarget && session.running && session.ptyId !== null,
  );
  const renderedState = effectiveTarget === state.targetKey
    ? state
    : setFreebuffRelayTarget(state, effectiveTarget);
  const enterMode = options.enterMode ?? relayEnterMode;
  const latestExchange = latestFreebuffRelayExchange(renderedState);
  const stalled = latestExchange?.status === "stalled";
  const relayModels = options.bridge?.models?.() ?? [];
  const activeTarget = sessions.find((session) => session.key === effectiveTarget) ?? null;
  const activeModelAvailable = typeof options.bridge?.activeModel === "function";

  return `
    <section id="freebuffRelayPanel" class="freebuff-relay-panel" aria-labelledby="freebuffRelayTitle">
      <div class="freebuff-relay-shell">
        <header class="freebuff-relay-hero">
          <div class="freebuff-relay-heading">
            <span class="freebuff-relay-mark" aria-hidden="true"><i data-lucide="message-square-reply"></i></span>
            <div>
              <p>Chat relié au terminal Freebuff</p>
              <h2 id="freebuffRelayTitle">Relais Freebuff</h2>
              <span>Chaque message que tu écris ici est envoyé dans le TUI Freebuff ouvert, et sa sortie revient dans ce fil.</span>
            </div>
          </div>
          <button type="button" class="freebuff-relay-clear" data-freebuff-relay-clear ${renderedState.exchanges.length ? "" : "disabled"} title="Effacer l'historique du relais"><i data-lucide="trash-2"></i><span>Effacer</span></button>
        </header>

        <div class="freebuff-relay-target">
          <label>
            <span><i data-lucide="target"></i>Terminal cible</span>
            ${renderRelayTargetSelect(renderedState, sessions)}
          </label>
          ${renderRelayModelSelect(renderedState, sessions, relayModels, relayModelApplying)}
          ${renderRelayTargetState(renderedState, sessions)}
          ${renderRelayActiveModel(
            activeTarget,
            relayActiveModel,
            relayActiveModelPending || relayActiveModelTargetKey !== effectiveTarget,
            activeModelAvailable,
          )}
        </div>

        ${!hasSessions ? `
          <div class="freebuff-relay-empty">
            <span><i data-lucide="square-terminal"></i></span>
            <h3>Aucun terminal Freebuff ouvert</h3>
            <p>Ouvre un terminal Freebuff dans la vue Terminaux (compte Freebuff), puis reviens ici : le relais y enverra tes messages.</p>
            <button type="button" class="tool-button primary" data-freebuff-relay-open-terminal><i data-lucide="folder-open"></i><span>Ouvrir les terminaux</span></button>
          </div>` : ""}

        ${stalled ? `
          <div class="freebuff-relay-stall-banner" role="status">
            <i data-lucide="triangle-alert"></i>
            <span><strong>Terminal sans activité</strong> · Aucune sortie du TUI depuis plusieurs minutes après l'envoi : il est peut-être bloqué. Vérifie le terminal, envoie « Entrée » ou renvoie le message.</span>
          </div>` : ""}

        ${renderedState.exchanges.length ? `
          <div class="freebuff-relay-thread" data-freebuff-relay-thread>
            ${renderedState.exchanges.map(renderRelayExchange).join("")}
          </div>` : `
          <div class="freebuff-relay-welcome">
            <span><i data-lucide="message-square-reply"></i></span>
            <h3>Commence la conversation</h3>
            <p>Écris un message ci-dessous : il sera tapé dans le TUI Freebuff et validé par Entrée. Décoche « Valider par Entrée » pour piloter les menus interactifs pas à pas.</p>
          </div>`}

        <form id="freebuffRelayComposer" class="freebuff-relay-composer ${enterMode ? "" : "is-raw"}">
          <textarea id="freebuffRelayMessage" maxlength="${FREEBUFF_RELAY_MESSAGE_MAX_LENGTH}" rows="3" placeholder="Message à envoyer dans le TUI Freebuff…" ${targetReady ? "" : "disabled"} required></textarea>
          <footer>
            <label class="freebuff-relay-enter-toggle" title="Validé par Entrée : comme une saisie réelle. Sans Entrée : texte écrit tel quel, pour les champs et menus du TUI.">
              <input type="checkbox" id="freebuffRelayEnterMode" ${enterMode ? "checked" : ""} ${targetReady ? "" : "disabled"} />
              <span><i data-lucide="corner-down-left"></i>Valider par Entrée</span>
            </label>
            <button type="button" class="freebuff-relay-enter-key" data-freebuff-relay-enter ${targetReady ? "" : "disabled"} title="Envoyer seulement la touche Entrée au terminal"><i data-lucide="corner-down-left"></i>Entrée</button>
            <p class="freebuff-relay-hint"><i data-lucide="info"></i><span>La sortie du terminal apparaît dans le fil au fur et à mesure.</span></p>
            <button type="submit" class="freebuff-relay-primary" ${targetReady ? "" : "disabled"}><i data-lucide="send"></i><span>Envoyer au terminal</span></button>
          </footer>
        </form>

        <footer class="freebuff-relay-footer">
          <p id="freebuffRelayFeedback" aria-live="polite"></p>
          <span><i data-lucide="hard-drive"></i>Historique enregistré sur cet appareil</span>
        </footer>
      </div>
    </section>`;
};

// --- Etat vivant du module (persiste entre deux montages de main) ----------
let relayTargetKey: string | null = null;
// Mode d'envoi courant : Entree valide le texte (defaut) ou le texte est
// envoye tel quel pour piloter les menus interactifs du TUI pas a pas.
let relayEnterMode = true;
// Application d'un nouveau modele en cours : le selecteur est desactive et le
// TUI redemarre (la cible peut changer apres le redemarrage).
let relayModelApplying = false;
// Modele effectivement actif sur le TUI (lu dans settings.json du canal
// manicode) : permet de voir si le binaire a normalise le modele demande.
let relayActiveModel: string | null = null;
let relayActiveModelPending = false;
let relayActiveModelTargetKey: string | null = null;
let relayRawBuffer = "";
let relayFlushTimer: number | null = null;
let relaySubscribed = false;
let relayMounted = false;
let relayPanelOptions: FreebuffRelayPanelOptions = {};
let relayLastExchangeId: string | null = null;
// Chien de garde de blocage : horodatage de la derniere activite du terminal
// (envoi ou sortie). Non null tant qu'on attend une reponse apres un envoi.
let relayStallClock: number | null = null;
let relayStallTimer: number | null = null;

const relayCurrentState = (): FreebuffRelayState => {
  const state = loadFreebuffRelayState(relayPanelOptions.storage);
  if (relayTargetKey === null || state.targetKey !== relayTargetKey) {
    return setFreebuffRelayTarget(state, relayTargetKey);
  }
  return state;
};

const relaySave = (state: FreebuffRelayState): void => {
  persistFreebuffRelayState(state, relayPanelOptions.storage);
  relayRefreshIfMounted();
};

const relayRefreshIfMounted = (): void => {
  if (!relayMounted || typeof document === "undefined") return;
  const root = document.querySelector<HTMLElement>("#freebuffRelayPanel");
  if (!root) return;
  const draft = root.querySelector<HTMLTextAreaElement>("#freebuffRelayMessage")?.value ?? "";
  root.outerHTML = renderFreebuffRelayPanel(relayPanelOptions);
  const fresh = document.querySelector<HTMLTextAreaElement>("#freebuffRelayMessage");
  if (fresh) fresh.value = draft;
  mountFreebuffRelayPanel(relayPanelOptions);
};

/**
 * Met a jour uniquement le fil (dernier echange) pendant le streaming : le
 * composeur reste intact pour ne pas interrompre une saisie en cours.
 */
const relayUpdateLiveThread = (): void => {
  if (!relayMounted || typeof document === "undefined") return;
  const root = document.querySelector<HTMLElement>("#freebuffRelayPanel");
  if (!root) return;
  const state = relayCurrentState();
  const latest = latestFreebuffRelayExchange(state);
  if (!latest || !relayLastExchangeId || latest.id !== relayLastExchangeId) {
    relayRefreshIfMounted();
    return;
  }
  const existing = root.querySelector<HTMLElement>(
    `[data-freebuff-relay-exchange="${CSS.escape(latest.id)}"]`,
  );
  if (existing) {
    existing.outerHTML = renderRelayExchange(latest);
  } else {
    relayRefreshIfMounted();
  }
  const thread = root.querySelector<HTMLElement>("[data-freebuff-relay-thread]");
  thread?.scrollTo({ top: thread.scrollHeight });
};

const relayFlushOutput = (): void => {
  relayFlushTimer = null;
  if (!relayLastExchangeId || !relayRawBuffer) return;
  const cleaned = cleanFreebuffRelayOutput(relayRawBuffer);
  relayRawBuffer = "";
  const next = appendFreebuffRelayOutput(relayCurrentState(), relayLastExchangeId, cleaned);
  persistFreebuffRelayState(next, relayPanelOptions.storage);
  relayUpdateLiveThread();
};

const relayScheduleOutputFlush = (): void => {
  if (relayFlushTimer !== null) return;
  relayFlushTimer = window.setTimeout(() => relayFlushOutput(), 150);
};

const relayDisarmStallCheck = (): void => {
  relayStallClock = null;
  if (relayStallTimer !== null) {
    window.clearTimeout(relayStallTimer);
    relayStallTimer = null;
  }
};

const relayStallTick = (): void => {
  relayStallTimer = null;
  if (relayStallClock === null || !relayLastExchangeId) return;
  const thresholdMs = relayPanelOptions.stallThresholdMs
    ?? FREEBUFF_RELAY_STALL_THRESHOLD_MS;
  const current = relayCurrentState();
  const next = resolveFreebuffRelayStall(
    current,
    relayLastExchangeId,
    relayStallClock,
    Date.now(),
    thresholdMs,
  );
  if (next !== current) relaySave(next);
  // Tant que l'echange attend toujours une reponse (pas encore bloque), on
  // poursuit la surveillance.
  const latest = latestFreebuffRelayExchange(next);
  if (latest?.status === "sent") {
    relayStallTimer = window.setTimeout(
      relayStallTick,
      FREEBUFF_RELAY_STALL_CHECK_INTERVAL_MS,
    );
  }
};

/**
 * Arme le compte a rebours du blocage apres un envoi : toute sortie du
 * terminal avant le seuil desarme la detection (le TUI repond).
 */
const relayArmStallCheck = (): void => {
  relayStallClock = Date.now();
  if (relayStallTimer === null) {
    relayStallTimer = window.setTimeout(
      relayStallTick,
      FREEBUFF_RELAY_STALL_CHECK_INTERVAL_MS,
    );
  }
};

const relayOnOutput = (key: string, data: string): void => {
  if (!relayTargetKey || key !== relayTargetKey || !data) return;
  relayRawBuffer = `${relayRawBuffer}${data}`.slice(-FREEBUFF_RELAY_RAW_BUFFER_LIMIT);
  // Une sortie arrive : le terminal est vivant. On desarme le blocage et on
  // retire le statut « bloque » de l'echange en cours s'il avait ete signale.
  relayDisarmStallCheck();
  if (relayLastExchangeId) {
    const current = relayCurrentState();
    const latest = latestFreebuffRelayExchange(current);
    if (latest?.id === relayLastExchangeId && latest.status === "stalled") {
      const unStalled = markFreebuffRelayExchangeSent(current, relayLastExchangeId);
      persistFreebuffRelayState(unStalled, relayPanelOptions.storage);
      relayUpdateLiveThread();
    }
  }
  // Affichage immediat (sans persister a chaque fragment), puis persistance
  // regroupee pour ne pas marteler le localStorage pendant le streaming.
  relayScheduleOutputFlush();
};

const ensureRelayOutputSubscription = (bridge: FreebuffRelayBridge | null | undefined): void => {
  if (relaySubscribed || !bridge) return;
  relaySubscribed = true;
  bridge.subscribeOutput(relayOnOutput);
};

/**
 * Relit le modele effectivement actif du TUI cible (settings.json du canal
 * manicode). Ignore la relecture si une lecture est deja en cours ou si la
 * valeur en cache concerne deja cette cible ; `force` reinterroge quand meme
 * (apres un redemarrage, la valeur peut mettre un instant a se stabiliser).
 */
const relayRefreshActiveModel = (force = false): void => {
  const bridge = relayPanelOptions.bridge;
  const targetKey = relayTargetKey;
  if (!targetKey || !bridge?.activeModel) return;
  if (!force && (relayActiveModelPending || relayActiveModelTargetKey === targetKey)) return;
  relayActiveModelPending = true;
  relayRefreshIfMounted();
  void bridge.activeModel(targetKey)
    .then((active) => {
      // La cible a change pendant la lecture : la valeur est perimee.
      if (relayTargetKey !== targetKey) return;
      relayActiveModel = active ?? null;
      relayActiveModelPending = false;
      relayActiveModelTargetKey = targetKey;
      relayRefreshIfMounted();
    })
    .catch(() => {
      if (relayTargetKey !== targetKey) return;
      relayActiveModel = null;
      relayActiveModelPending = false;
      relayActiveModelTargetKey = targetKey;
      relayRefreshIfMounted();
    });
};

export const setFreebuffRelayPendingTarget = (targetKey: string | null): void => {
  relayTargetKey = targetKey?.trim() || null;
  relayRawBuffer = "";
  relayLastExchangeId = null;
  relayDisarmStallCheck();
};

const focusRelayComposer = (): void => {
  document
    .querySelector<HTMLTextAreaElement>("#freebuffRelayMessage")
    ?.focus();
};

export const mountFreebuffRelayPanel = (
  options: FreebuffRelayPanelOptions = {},
): void => {
  const root = document.querySelector<HTMLElement>("#freebuffRelayPanel");
  if (!root) return;
  relayPanelOptions = { ...options, pendingTargetKey: options.pendingTargetKey?.trim() || null };
  relayMounted = true;
  ensureRelayOutputSubscription(relayPanelOptions.bridge);

  const state = loadFreebuffRelayState(relayPanelOptions.storage);
  if (relayPanelOptions.pendingTargetKey) {
    relayTargetKey = relayPanelOptions.pendingTargetKey;
    relayRawBuffer = "";
    relayLastExchangeId = null;
    relayDisarmStallCheck();
    // Consommee une seule fois : les rafraichissements suivants ne la
    // re-appliquent plus (sinon le choix de l'utilisateur serait ecrase).
    relayPanelOptions = { ...relayPanelOptions, pendingTargetKey: null };
    persistFreebuffRelayState(setFreebuffRelayTarget(state, relayTargetKey), relayPanelOptions.storage);
  }
  if (relayTargetKey === null) relayTargetKey = state.targetKey;
  // Une cible enregistree dont le terminal a ete ferme retombe sur le premier
  // terminal Freebuff encore ouvert.
  const sessions = relayPanelOptions.bridge?.sessions() ?? [];
  if (relayTargetKey && !sessions.some((session) => session.key === relayTargetKey)) {
    relayTargetKey = sessions[0]?.key ?? null;
  }

  // Relit le modele actif du TUI cible au (re)montage du panneau.
  relayRefreshActiveModel();

  // Rattache le tampon en cours a la derniere sortie connue.
  const latest = latestFreebuffRelayExchange(relayCurrentState());
  relayLastExchangeId = latest?.status === "sending" ? latest.id : null;

  options.renderIcons?.(root);

  const targetSelect = root.querySelector<HTMLSelectElement>("#freebuffRelayTarget");
  targetSelect?.addEventListener("change", () => {
    setFreebuffRelayPendingTarget(targetSelect.value || null);
    relaySave(setFreebuffRelayTarget(relayCurrentState(), targetSelect.value || null));
    relayRefreshActiveModel();
  });

  const composer = root.querySelector<HTMLFormElement>("#freebuffRelayComposer");
  const messageInput = root.querySelector<HTMLTextAreaElement>("#freebuffRelayMessage");
  composer?.addEventListener("submit", (event) => {
    event.preventDefault();
    if (!messageInput) return;
    const text = normalizeRelayMessage(messageInput.value);
    const bridge = options.bridge;
    const sessions = bridge?.sessions() ?? [];
    const target = sessions.find((session) => session.key === relayTargetKey) ?? null;
    if (!text) {
      messageInput.setCustomValidity("Écris d'abord un message à transmettre.");
      messageInput.reportValidity();
      return;
    }
    if (!target || !target.running || target.ptyId === null) {
      const feedback = document.querySelector<HTMLElement>("#freebuffRelayFeedback");
      if (feedback) feedback.textContent = "Aucun terminal Freebuff prêt à recevoir : ouvre ou relance le terminal cible.";
      return;
    }
    messageInput.setCustomValidity("");
    // La sortie en attente de l'echange precedent est d'abord fige puis
    // persistee : le nouveau tampon commence proprement.
    if (relayFlushTimer !== null) {
      window.clearTimeout(relayFlushTimer);
      relayFlushTimer = null;
      relayFlushOutput();
    }
    const enterCheckbox = root.querySelector<HTMLInputElement>("#freebuffRelayEnterMode");
    const withEnter = enterCheckbox ? enterCheckbox.checked : relayEnterMode;
    relayRawBuffer = "";
    const current = relayCurrentState();
    const next = addFreebuffRelayExchange(current, text, undefined, undefined, withEnter);
    const exchangeId = latestFreebuffRelayExchange(next)?.id ?? null;
    relayLastExchangeId = exchangeId;
    relaySave(next);
    // Le compte a rebours du blocage demarre a l'envoi : si le TUI ne repond
    // pas avant le seuil, l'echange passera en « bloque ».
    relayArmStallCheck();
    messageInput.value = "";
    void (async () => {
      if (!exchangeId || !target.ptyId) return;
      const payload = freebuffRelayPayload(text, withEnter);
      const ok = await bridge?.write(target.ptyId, payload);
      const latestState = relayCurrentState();
      if (ok) {
        relaySave(markFreebuffRelayExchangeSent(latestState, exchangeId));
      } else {
        relaySave(markFreebuffRelayExchangeFailed(
          latestState,
          exchangeId,
          "Impossible d'écrire dans le terminal : il est peut-être fermé.",
        ));
        if (relayLastExchangeId === exchangeId) relayLastExchangeId = null;
      }
    })();
    queueMicrotask(focusRelayComposer);
  });

  root.querySelector<HTMLButtonElement>("[data-freebuff-relay-clear]")?.addEventListener("click", () => {
    relayRawBuffer = "";
    relayLastExchangeId = null;
    relayDisarmStallCheck();
    relaySave(clearFreebuffRelayHistory(relayCurrentState()));
    focusRelayComposer();
  });

  root.querySelector<HTMLSelectElement>("[data-freebuff-relay-model]")?.addEventListener("change", (event) => {
    const model = (event.target as HTMLSelectElement).value.trim();
    const bridge = options.bridge;
    if (!model || !relayTargetKey || !bridge?.applyModel) return;
    relayModelApplying = true;
    const feedback = document.querySelector<HTMLElement>("#freebuffRelayFeedback");
    if (feedback) feedback.textContent = `Application du modèle ${model}… le TUI va redémarrer.`;
    relayRefreshIfMounted();
    void (async () => {
      const result = await bridge.applyModel?.(relayTargetKey!, model);
      relayModelApplying = false;
      const nextFeedback = document.querySelector<HTMLElement>("#freebuffRelayFeedback");
      if (nextFeedback) {
        nextFeedback.textContent = result?.ok
          ? `Modèle ${model} appliqué : le TUI redémarre avec ce modèle.`
          : (result?.error ?? "Changement de modèle impossible.");
      }
      // Le redemarrage cree un nouveau terminal : la cible enregistree peut
      // ne plus exister, le remontage retombe alors sur le premier terminal
      // Freebuff ouvert.
      relayRefreshIfMounted();
      // Le TUI relit la config au demarrage : la valeur du modele actif peut
      // mettre un instant a se stabiliser, on la relit plusieurs fois.
      relayActiveModelTargetKey = null;
      relayActiveModelPending = false;
      relayRefreshActiveModel();
      const reread = (attempt: number): void => {
        if (attempt <= 1) return;
        window.setTimeout(() => {
          relayRefreshActiveModel(true);
          reread(attempt - 1);
        }, 2_000);
      };
      reread(3);
    })();
  });

  root.querySelector<HTMLInputElement>("#freebuffRelayEnterMode")?.addEventListener("change", (event) => {
    relayEnterMode = (event.target as HTMLInputElement).checked;
    composer?.classList.toggle("is-raw", !relayEnterMode);
  });

  root.querySelector<HTMLButtonElement>("[data-freebuff-relay-enter]")?.addEventListener("click", () => {
    const bridge = options.bridge;
    const target = (bridge?.sessions() ?? [])
      .find((session) => session.key === relayTargetKey) ?? null;
    if (!target || !target.running || target.ptyId === null) {
      const feedback = document.querySelector<HTMLElement>("#freebuffRelayFeedback");
      if (feedback) feedback.textContent = "Aucun terminal Freebuff prêt à recevoir : ouvre ou relance le terminal cible.";
      return;
    }
    void (async () => {
      const ok = await bridge?.write(target.ptyId!, "\r");
      const feedback = document.querySelector<HTMLElement>("#freebuffRelayFeedback");
      if (feedback) feedback.textContent = ok
        ? "Entrée envoyée au terminal."
        : "Échec de l'envoi de la touche Entrée au terminal.";
    })();
    queueMicrotask(focusRelayComposer);
  });

  root.querySelector<HTMLButtonElement>("[data-freebuff-relay-open-terminal]")?.addEventListener("click", () => {
    options.bridge?.onOpenTerminalView?.();
  });

  queueMicrotask(focusRelayComposer);
};
