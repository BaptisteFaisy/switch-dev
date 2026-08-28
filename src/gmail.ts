// Onglet Mail : liaison d'une boite Gmail au compte utilisateur (via le
// serveur, /api/gmail) et affichage des messages avec actualisation.
//
// Etat au niveau module, comme messaging.ts : main.ts insere les chaines
// produites ici dans son propre rendu puis rappelle bindGmailUi apres chaque
// passe pour reattacher les ecouteurs.

import "./gmail.css";
import { isRemoteMode, remoteBaseUrl } from "./platform";

export type GmailConnectionView = {
  configured: boolean;
  connected: boolean;
  email: string | null;
  needsRelink: boolean;
  scopes: string[];
  linkedAt: number | null;
  /** URI de redirection a declarer dans Google Cloud Console. */
  redirectUri: string;
  clientId: string | null;
  loginUrl: string | null;
};

export type GmailMessage = {
  id: string;
  threadId: string;
  from: string | null;
  to: string[];
  subject: string;
  snippet: string;
  date: string | null;
  unread: boolean;
  labels: string[];
};

export type GmailMessagesView = {
  email: string;
  fetchedAt: number;
  messages: GmailMessage[];
};

/** Vue de liaison Microsoft 365 telle que renvoyee par /api/microsoft. */
type OutlookConnection = {
  configured: boolean;
  connected: boolean;
  email: string | null;
  loginUrl: string | null;
  redirectUri: string | null;
};

type OutlookMessage = {
  id: string;
  subject: string;
  from: string;
  fromName: string;
  receivedAt: string;
  preview: string;
  isRead: boolean;
  hasAttachments: boolean;
};

type OutlookMessagesView = {
  messages: OutlookMessage[];
  mailbox: string;
};

type Feedback = { tone: "success" | "error"; message: string };

const GMAIL_POLL_INTERVAL_MS = 60_000;
const MESSAGES_REQUESTED = 50;

// Le noeud local (Tauri) n'a pas de session nominative ni d'API Gmail : la
// vue affiche une carte « disponible sur le serveur », comme Microsoft 365.
const OFFLINE_CONNECTION: GmailConnectionView = {
  configured: false,
  connected: false,
  email: null,
  needsRelink: false,
  scopes: [],
  linkedAt: null,
  redirectUri: "",
  clientId: null,
  loginUrl: null,
};

let connection: GmailConnectionView | null = null;
let connectionLoaded = false;
let connectionLoading = false;
let messages: GmailMessage[] = [];
let messagesEmail: string | null = null;
let fetchedAt: number | null = null;
let messagesLoading = false;
let refreshInFlight = false;
let feedback: Feedback | null = null;
let lastError: string | null = null;

let outlookConnection: OutlookConnection | null = null;
let outlookLoaded = false;
let outlookLoading = false;
let outlookMessages: OutlookMessage[] = [];
let outlookMailbox: string | null = null;
let outlookError: string | null = null;

let visible = false;
let pollTimer: number | null = null;
let rerender: () => void = () => {};
let setStatus: (message: string) => void = () => {};

const escapeHtml = (value: unknown): string =>
  String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");

const errorMessage = (error: unknown): string =>
  (error instanceof Error ? error.message : String(error)).replace(/^Error:\s*/i, "");

class GmailApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

const serverApi = async <T>(
  apiBase: string,
  path: string,
  options: { method?: string; body?: unknown } = {},
): Promise<T> => {
  const response = await fetch(`${remoteBaseUrl()}${apiBase}${path}`, {
    method: options.method ?? "GET",
    credentials: "include",
    headers: options.body === undefined ? {} : { "Content-Type": "application/json" },
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
  });
  const text = await response.text();
  let value: any = null;
  if (text) {
    try {
      value = JSON.parse(text);
    } catch {
      value = null;
    }
  }
  if (!response.ok) {
    throw new GmailApiError(
      typeof value?.error === "string" ? value.error : `Erreur Gmail (${response.status})`,
      response.status,
    );
  }
  return value as T;
};

const gmailApi = <T>(
  path: string,
  options: { method?: string; body?: unknown } = {},
): Promise<T> => serverApi<T>("/api/gmail", path, options);

const microsoftApi = <T>(
  path: string,
  options: { method?: string; body?: unknown } = {},
): Promise<T> => serverApi<T>("/api/microsoft", path, options);

// ---------------------------------------------------------------------------
// Chargement
// ---------------------------------------------------------------------------

const ensureConnectionLoaded = async (): Promise<void> => {
  if (connectionLoaded || connectionLoading) return;
  connectionLoading = true;
  try {
    if (!isRemoteMode()) {
      connection = OFFLINE_CONNECTION;
      outlookConnection = { configured: false, connected: false, email: null, loginUrl: null, redirectUri: null };
      connectionLoaded = true;
      return;
    }
    connection = await gmailApi<GmailConnectionView>("/connection");
    if (connection.connected && messagesEmail !== connection.email) {
      await refreshGmailMessages(true);
    }
    await refreshOutlookConnection(true);
  } catch (error) {
    lastError = errorMessage(error);
  } finally {
    connectionLoading = false;
    connectionLoaded = true;
    rerender();
  }
};

export const refreshGmailMessages = async (silent = false): Promise<void> => {
  if (refreshInFlight) return;
  refreshInFlight = true;
  if (!silent) messagesLoading = true;
  lastError = null;
  rerender();
  try {
    const view = await gmailApi<GmailMessagesView>(
      `/messages?max=${MESSAGES_REQUESTED}`,
    );
    messages = view.messages;
    messagesEmail = view.email;
    fetchedAt = view.fetchedAt;
    feedback = null;
  } catch (error) {
    lastError = errorMessage(error);
  } finally {
    refreshInFlight = false;
    messagesLoading = false;
    rerender();
  }
};

const refreshOutlookConnection = async (silent = false): Promise<void> => {
  if (outlookLoading) return;
  outlookLoading = true;
  try {
    const view = await microsoftApi<OutlookConnection>("/connection");
    outlookConnection = view;
    outlookLoaded = true;
    if (view.connected) {
      await refreshOutlookMessages(true);
    } else {
      outlookMessages = [];
      outlookMailbox = null;
    }
  } catch (error) {
    if (!silent) outlookError = errorMessage(error);
    outlookLoaded = true;
  } finally {
    outlookLoading = false;
    rerender();
  }
};

export const refreshOutlookMessages = async (silent = false): Promise<void> => {
  if (!outlookConnection?.connected) return;
  try {
    const view = await microsoftApi<OutlookMessagesView>("/messages?max=25");
    outlookMessages = view.messages;
    outlookMailbox = view.mailbox;
    outlookError = null;
  } catch (error) {
    if (!silent) outlookError = errorMessage(error);
  }
  rerender();
};

export const refreshAllMails = async (silent = false): Promise<void> => {
  await Promise.all([refreshGmailMessages(silent), refreshOutlookMessages(silent)]);
};

export const disconnectGmail = async (): Promise<void> => {
  try {
    await gmailApi("/connection", { method: "DELETE" });
    messages = [];
    messagesEmail = null;
    fetchedAt = null;
    connection = await gmailApi<GmailConnectionView>("/connection");
    feedback = { tone: "success", message: "Boîte Gmail déliée." };
  } catch (error) {
    feedback = { tone: "error", message: errorMessage(error) };
  }
  rerender();
};

export const disconnectOutlook = async (): Promise<void> => {
  try {
    await microsoftApi("/connection", { method: "DELETE" });
    outlookMessages = [];
    outlookMailbox = null;
    await refreshOutlookConnection(true);
  } catch (error) {
    outlookError = errorMessage(error);
  }
  rerender();
};

export const gmailUnreadCount = (): number =>
  messages.filter((message) => message.unread).length +
  outlookMessages.filter((message) => !message.isRead).length;

// ---------------------------------------------------------------------------
// Polling d'actualisation
// ---------------------------------------------------------------------------

export const startGmailPolling = (render: () => void): void => {
  rerender = render;
  if (pollTimer !== null) return;
  void ensureConnectionLoaded();
  pollTimer = window.setInterval(() => {
    if (visible && !refreshInFlight && connectionLoaded) {
      void refreshAllMails(true);
    }
  }, GMAIL_POLL_INTERVAL_MS);
};

export const stopGmailPolling = (): void => {
  if (pollTimer !== null) {
    window.clearInterval(pollTimer);
    pollTimer = null;
  }
};

export const setGmailVisible = (value: boolean): void => {
  visible = value;
  if (value && !connectionLoaded) void ensureConnectionLoaded();
};

export const deactivateGmailPanel = (): void => {
  stopGmailPolling();
  visible = false;
};

// ---------------------------------------------------------------------------
// Rendu
// ---------------------------------------------------------------------------

const formatRelativeTime = (timestamp: number): string => {
  const delta = Math.max(0, Date.now() / 1000 - timestamp);
  if (delta < 60) return "à l'instant";
  if (delta < 3600) return `il y a ${Math.floor(delta / 60)} min`;
  if (delta < 86_400) return `il y a ${Math.floor(delta / 3600)} h`;
  if (delta < 172_800) return "hier";
  const date = new Date(timestamp * 1000);
  return date.toLocaleDateString("fr-FR", { day: "numeric", month: "short" });
};

const renderConnectionCard = (): string => {
  const view = connection ?? OFFLINE_CONNECTION;
  if (!isRemoteMode()) {
    return `
      <div class="gmail-card">
        <div class="gmail-card-icon"><i data-lucide="mail"></i></div>
        <div class="gmail-card-copy">
          <strong>Disponible sur le serveur</strong>
          <p>L'onglet Mail lit la boite Gmail du compte connecté sur le nœud serveur. Ouvre Switch sur le serveur (interface web) pour lier ta boîte.</p>
        </div>
      </div>`;
  }
  if (!view.configured) {
    return `
      <div class="gmail-card">
        <div class="gmail-card-icon"><i data-lucide="circle-alert"></i></div>
        <div class="gmail-card-copy">
          <strong>Intégration Gmail non configurée</strong>
          <p>Définis <code>CST_GOOGLE_CLIENT_ID</code> et <code>CST_GOOGLE_CLIENT_SECRET</code> sur le serveur, puis déclare l'URI de redirection dans Google Cloud Console :</p>
          <code class="gmail-redirect-uri">${escapeHtml(view.redirectUri)}</code>
        </div>
      </div>`;
  }
  if (!view.connected) {
    return `
      <div class="gmail-card">
        <div class="gmail-card-icon"><i data-lucide="mail-plus"></i></div>
        <div class="gmail-card-copy">
          <strong>${view.needsRelink ? "Autorisation à renouveler" : "Connecter une boîte Gmail"}</strong>
          <p>${view.needsRelink ? "L'accès Google a expiré. Relie à nouveau ta boîte." : "L'onglet lit les messages (lecture seule). Aucun mot de passe n'est stocké ici."}</p>
          ${view.redirectUri ? `<p class="gmail-hint">URI de redirection à déclarer dans Google Cloud Console : <code>${escapeHtml(view.redirectUri)}</code></p>` : ""}
        </div>
        <a class="tool-button primary" href="${escapeHtml(view.loginUrl ?? `${remoteBaseUrl()}/api/gmail/start`)}">${view.needsRelink ? "Relier à nouveau" : "Se connecter à Gmail"}</a>
      </div>`;
  }
  return `
    <div class="gmail-card">
      <div class="gmail-card-icon"><i data-lucide="check-circle-2"></i></div>
      <div class="gmail-card-copy">
        <strong>Boîte liée : ${escapeHtml(view.email)}</strong>
        <p>Lecture seule · ${view.linkedAt ? `liée le ${new Date(view.linkedAt * 1000).toLocaleDateString("fr-FR")}` : "connectée"}</p>
      </div>
      <button type="button" id="gmailDisconnect" class="tool-button" title="Délier cette boîte">Déconnecter</button>
    </div>`;
};

const renderMessageRow = (message: GmailMessage): string => {
  const from = message.from ?? "Expéditeur inconnu";
  const openUrl = `https://mail.google.com/mail/u/0/#inbox/${encodeURIComponent(message.threadId)}`;
  return `
    <article class="gmail-message${message.unread ? " is-unread" : ""}">
      <span class="gmail-message-dot" aria-hidden="true"></span>
      <div class="gmail-message-body">
        <div class="gmail-message-head">
          <strong>${escapeHtml(from)}</strong>
          <time>${message.date ? formatRelativeTime(Date.parse(message.date) / 1000 || Date.now() / 1000) : ""}</time>
        </div>
        <h3>${escapeHtml(message.subject)}</h3>
        <p>${escapeHtml(message.snippet)}</p>
      </div>
      <a class="gmail-message-open" href="${escapeHtml(openUrl)}" target="_blank" rel="noopener noreferrer" title="Ouvrir dans Gmail"><i data-lucide="external-link"></i></a>
    </article>`;
};

const renderOutlookConnectionCard = (): string => {
  const view = outlookConnection;
  if (!isRemoteMode()) {
    return `
      <div class="gmail-card">
        <div class="gmail-card-icon"><i data-lucide="mail"></i></div>
        <div class="gmail-card-copy"><strong>Disponible sur le serveur</strong><p>Liaison Outlook possible depuis l'interface web du serveur.</p></div>
      </div>`;
  }
  if (!view?.configured) {
    return `
      <div class="gmail-card">
        <div class="gmail-card-icon"><i data-lucide="circle-alert"></i></div>
        <div class="gmail-card-copy">
          <strong>Outlook non configuré</strong>
          <p>Définis <code>CST_MICROSOFT_CLIENT_ID</code> et <code>CST_MICROSOFT_CLIENT_SECRET</code> sur le serveur (voir le guide Microsoft 365).</p>
          ${view?.redirectUri ? `<p class="gmail-hint">URI de redirection : <code>${escapeHtml(view.redirectUri)}</code></p>` : ""}
        </div>
      </div>`;
  }
  if (!view.connected) {
    return `
      <div class="gmail-card">
        <div class="gmail-card-icon"><i data-lucide="mail-plus"></i></div>
        <div class="gmail-card-copy">
          <strong>Connecter une boîte Outlook</strong>
          <p>L'onglet lit les messages de ta boîte Microsoft 365.</p>
        </div>
        <a class="tool-button primary" href="${escapeHtml(view.loginUrl ?? `${remoteBaseUrl()}/api/microsoft/start`)}">Se connecter à Outlook</a>
      </div>`;
  }
  return `
    <div class="gmail-card">
      <div class="gmail-card-icon"><i data-lucide="check-circle-2"></i></div>
      <div class="gmail-card-copy">
        <strong>Boîte liée : ${escapeHtml(view.email)}</strong>
        <p>Lecture seule · Microsoft 365</p>
      </div>
      <button type="button" id="gmailOutlookDisconnect" class="tool-button" title="Délier cette boîte">Déconnecter</button>
    </div>`;
};

const renderOutlookMessageRow = (message: OutlookMessage): string => {
  const from = message.fromName || message.from || "Expéditeur inconnu";
  return `
    <article class="gmail-message${message.isRead ? "" : " is-unread"}">
      <span class="gmail-message-dot" aria-hidden="true"></span>
      <div class="gmail-message-body">
        <div class="gmail-message-head">
          <strong>${escapeHtml(from)}</strong>
          <time>${message.receivedAt ? formatRelativeTime(Date.parse(message.receivedAt) / 1000 || Date.now() / 1000) : ""}</time>
        </div>
        <h3>${escapeHtml(message.subject)}${message.hasAttachments ? " <i data-lucide='paperclip' class='gmail-attachment-icon'></i>" : ""}</h3>
        <p>${escapeHtml(message.preview)}</p>
      </div>
    </article>`;
};

const renderProviderSection = (options: {
  id: string;
  title: string;
  unread: number;
  count: number;
  connectionCard: string;
  connected: boolean;
  loading: boolean;
  error: string | null;
  emptyLabel: string;
  rows: string;
  refreshedAt: number | null;
}): string => {
  const { id, title, unread, count, connectionCard, connected, loading, error, emptyLabel, rows, refreshedAt } = options;
  return `
    <section class="gmail-provider" aria-labelledby="${id}Title">
      <header class="gmail-provider-head">
        <div class="gmail-provider-title"><span class="gmail-provider-logo">${title === "Gmail" ? "G" : "O"}</span><h3 id="${id}Title">${title}</h3>${unread ? `<b>${unread > 99 ? "99+" : unread}</b>` : ""}</div>
        ${refreshedAt ? `<span class="gmail-refreshed">Actualisé ${formatRelativeTime(refreshedAt)}</span>` : ""}
      </header>
      ${connectionCard}
      ${connected ? `
        <div class="gmail-list-head">
          <span>${count ? `${count} message${count > 1 ? "s" : ""}` : ""}</span>
        </div>
        ${error ? `<div class="gmail-error" role="alert"><i data-lucide="circle-alert"></i><span>${escapeHtml(error)}</span></div>` : ""}
        ${loading && !count ? `<div class="gmail-loading" role="status"><i data-lucide="loader-circle" class="is-spinning"></i><span>Chargement des messages…</span></div>` : ""}
        ${!loading && !error && count === 0 ? `<div class="gmail-empty"><i data-lucide="inbox"></i><span>${emptyLabel}</span></div>` : ""}
        ${rows}
      ` : ""}
    </section>`;
};

export const renderGmailPanel = (): string => {
  if (!connectionLoaded && !connectionLoading) void ensureConnectionLoaded();
  const unreadCount = gmailUnreadCount();
  const gmailConnected = !!connection?.connected && isRemoteMode();
  const outlookConnected = !!outlookConnection?.connected && isRemoteMode();
  const anyLoading = messagesLoading || outlookLoading;
  return `
    <section id="gmailPanel" class="gmail-panel" aria-labelledby="gmailPanelTitle">
      <header class="gmail-hero">
        <div class="gmail-hero-mark"><i data-lucide="inbox"></i>${unreadCount ? `<b>${unreadCount > 99 ? "99+" : unreadCount}</b>` : ""}</div>
        <div><span>Boîte de réception</span><h2 id="gmailPanelTitle">Mail</h2><p>Vos e-mails Gmail et Outlook, actualisés automatiquement.</p></div>
        <div class="gmail-hero-actions">
          <span class="gmail-auto-note" title="Actualisation automatique toutes les 60 secondes"><i data-lucide="refresh-cw"></i><span>Auto</span></span>
          <button type="button" id="gmailRefresh" class="tool-button primary" ${anyLoading ? "disabled" : ""}>
            <i data-lucide="refresh-cw" class="${anyLoading ? "is-spinning" : ""}"></i><span>Actualiser</span>
          </button>
        </div>
      </header>

      ${feedback ? `<div class="gmail-feedback is-${feedback.tone}" role="status"><i data-lucide="${feedback.tone === "success" ? "check" : "circle-alert"}"></i><span>${escapeHtml(feedback.message)}</span></div>` : ""}

      <div class="gmail-provider-grid">
        ${renderProviderSection({
          id: "gmailProvider",
          title: "Gmail",
          unread: messages.filter((message) => message.unread).length,
          count: messages.length,
          connectionCard: renderConnectionCard(),
          connected: gmailConnected,
          loading: messagesLoading,
          error: lastError,
          emptyLabel: "Aucun message dans la boîte de réception.",
          rows: gmailConnected && messages.length ? `<div class="gmail-messages">${messages.map(renderMessageRow).join("")}</div>` : "",
          refreshedAt: fetchedAt,
        })}
        ${renderProviderSection({
          id: "outlookProvider",
          title: "Outlook",
          unread: outlookMessages.filter((message) => !message.isRead).length,
          count: outlookMessages.length,
          connectionCard: renderOutlookConnectionCard(),
          connected: outlookConnected,
          loading: outlookLoading,
          error: outlookError,
          emptyLabel: "Aucun message dans la boîte de réception.",
          rows: outlookConnected && outlookMessages.length ? `<div class="gmail-messages">${outlookMessages.map(renderOutlookMessageRow).join("")}</div>` : "",
          refreshedAt: null,
        })}
      </div>
    </section>`;
};

// ---------------------------------------------------------------------------
// Ecouteurs
// ---------------------------------------------------------------------------

export const bindGmailUi = (deps: {
  rerender: () => void;
  setStatus: (message: string) => void;
}): void => {
  rerender = deps.rerender;
  setStatus = deps.setStatus;

  document.querySelector<HTMLButtonElement>("#gmailRefresh")?.addEventListener("click", () => {
    void refreshAllMails(false);
  });
  document.querySelector<HTMLButtonElement>("#gmailRetry")?.addEventListener("click", () => {
    void refreshGmailMessages(false);
  });
  document.querySelector<HTMLButtonElement>("#gmailDisconnect")?.addEventListener("click", () => {
    if (window.confirm("Délier cette boîte Gmail ? Les messages restent dans ta boîte.")) {
      void disconnectGmail();
    }
  });
  document.querySelector<HTMLButtonElement>("#gmailOutlookDisconnect")?.addEventListener("click", () => {
    if (window.confirm("Délier cette boîte Outlook ? Les messages restent dans ta boîte.")) {
      void disconnectOutlook();
    }
  });
};
