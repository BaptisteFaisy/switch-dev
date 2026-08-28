// Onglet Mail : liaison de boites Gmail et Outlook au compte utilisateur (via
// le serveur, /api/gmail et /api/microsoft) et affichage des messages avec
// actualisation. Multi-comptes : autant de boites que souhaite, par
// fournisseur, chacune avec ses propres messages.
//
// Etat au niveau module, comme messaging.ts : main.ts insere les chaines
// produites ici dans son propre rendu puis rappelle bindGmailUi apres chaque
// passe pour reattacher les ecouteurs.

import "./gmail.css";
import { isRemoteMode, remoteBaseUrl } from "./platform";

export type GmailAccount = {
  /** Identifiant de la liaison (`link` des routes /api/gmail). */
  linkId: string;
  email: string;
  needsRelink: boolean;
  scopes: string[];
  linkedAt: number;
  isDefault: boolean;
};

export type GmailConnectionView = {
  configured: boolean;
  connected: boolean;
  email: string | null;
  needsRelink: boolean;
  scopes: string[];
  linkedAt: number | null;
  accounts: GmailAccount[];
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
type OutlookAccount = {
  oid: string;
  email: string;
  displayName: string | null;
  isDefault: boolean;
  needsRelink: boolean;
  scopes: string[];
  linkedAt: number;
};

type OutlookConnection = {
  configured: boolean;
  connected: boolean;
  email: string | null;
  loginUrl: string | null;
  redirectUri: string | null;
  accounts: OutlookAccount[];
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
const OUTLOOK_MESSAGES_REQUESTED = 25;

// Le noeud local (Tauri) n'a pas de session nominative ni d'API Gmail : la
// vue affiche une carte « disponible sur le serveur », comme Microsoft 365.
const OFFLINE_CONNECTION: GmailConnectionView = {
  configured: false,
  connected: false,
  email: null,
  needsRelink: false,
  scopes: [],
  linkedAt: null,
  accounts: [],
  redirectUri: "",
  clientId: null,
  loginUrl: null,
};

let connection: GmailConnectionView | null = null;
let connectionLoaded = false;
let connectionLoading = false;
let messagesLoading = false;
let refreshInFlight = false;
let feedback: Feedback | null = null;
let lastError: string | null = null;

let outlookConnection: OutlookConnection | null = null;
let outlookLoaded = false;
let outlookLoading = false;
let outlookError: string | null = null;

/** Messages Gmail par `linkId`. */
let gmailMessages: Record<string, GmailMessage[]> = {};
/** Messages Outlook par `oid`. */
let outlookMessages: Record<string, OutlookMessage[]> = {};
let fetchedAt: number | null = null;

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
      typeof value?.error === "string" ? value.error : `Erreur Mail (${response.status})`,
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
      outlookConnection = { configured: false, connected: false, email: null, loginUrl: null, redirectUri: null, accounts: [] };
      connectionLoaded = true;
      return;
    }
    await refreshConnections(true);
  } catch (error) {
    lastError = errorMessage(error);
  } finally {
    connectionLoading = false;
    connectionLoaded = true;
    rerender();
  }
};

/** Relit les deux etats de liaison et les messages de chaque boite liee. */
const refreshConnections = async (silent = false): Promise<void> => {
  const [gmailView, outlookView] = await Promise.all([
    gmailApi<GmailConnectionView>("/connection"),
    microsoftApi<OutlookConnection>("/connection"),
  ]);
  connection = gmailView;
  outlookConnection = outlookView;
  connectionLoaded = true;
  outlookLoaded = true;

  // Nettoyer les messages des boites disparues (deconnectees ailleurs).
  const gmailIds = new Set(gmailView.accounts.map((account) => account.linkId));
  for (const key of Object.keys(gmailMessages)) {
    if (!gmailIds.has(key)) delete gmailMessages[key];
  }
  const outlookIds = new Set(outlookView.accounts.map((account) => account.oid));
  for (const key of Object.keys(outlookMessages)) {
    if (!outlookIds.has(key)) delete outlookMessages[key];
  }

  const tasks: Promise<void>[] = [];
  for (const account of gmailView.accounts) {
    if (!account.needsRelink) tasks.push(refreshGmailMessages(account.linkId, true));
  }
  for (const account of outlookView.accounts) {
    if (!account.needsRelink) tasks.push(refreshOutlookMessages(account.oid, true));
  }
  await Promise.all(tasks);
};

export const refreshGmailMessages = async (linkId: string, silent = false): Promise<void> => {
  if (!silent) messagesLoading = true;
  lastError = null;
  rerender();
  try {
    const view = await gmailApi<GmailMessagesView>(
      `/messages?max=${MESSAGES_REQUESTED}&link=${encodeURIComponent(linkId)}`,
    );
    gmailMessages[linkId] = view.messages;
    fetchedAt = view.fetchedAt;
    feedback = null;
  } catch (error) {
    lastError = errorMessage(error);
  } finally {
    messagesLoading = false;
    rerender();
  }
};

export const refreshOutlookMessages = async (oid: string, silent = false): Promise<void> => {
  try {
    const view = await microsoftApi<OutlookMessagesView>(
      `/messages?max=${OUTLOOK_MESSAGES_REQUESTED}&account=${encodeURIComponent(oid)}`,
    );
    outlookMessages[oid] = view.messages;
    outlookError = null;
  } catch (error) {
    if (!silent) outlookError = errorMessage(error);
  }
  rerender();
};

/** Actualise tout : etats de liaison puis messages de chaque boite. */
export const refreshAllMails = async (silent = false): Promise<void> => {
  if (refreshInFlight) return;
  refreshInFlight = true;
  try {
    await refreshConnections(silent);
  } catch (error) {
    if (!silent) lastError = errorMessage(error);
  } finally {
    refreshInFlight = false;
  }
};

export const disconnectGmail = async (linkId: string): Promise<void> => {
  try {
    await gmailApi(`/connection?link=${encodeURIComponent(linkId)}`, { method: "DELETE" });
    delete gmailMessages[linkId];
    await refreshConnections(true);
    feedback = { tone: "success", message: "Boîte Gmail déliée." };
  } catch (error) {
    feedback = { tone: "error", message: errorMessage(error) };
  }
  rerender();
};

export const disconnectOutlook = async (oid: string): Promise<void> => {
  try {
    await microsoftApi(`/connection/${encodeURIComponent(oid)}`, { method: "DELETE" });
    delete outlookMessages[oid];
    await refreshConnections(true);
    feedback = { tone: "success", message: "Boîte Outlook déliée." };
  } catch (error) {
    feedback = { tone: "error", message: errorMessage(error) };
  }
  rerender();
};

export const gmailUnreadCount = (): number => {
  let count = 0;
  for (const messages of Object.values(gmailMessages)) {
    count += messages.filter((message) => message.unread).length;
  }
  for (const messages of Object.values(outlookMessages)) {
    count += messages.filter((message) => !message.isRead).length;
  }
  return count;
};

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

const formatLinkedAt = (linkedAt: number): string =>
  new Date(linkedAt * 1000).toLocaleDateString("fr-FR");

const accountInitial = (email: string): string => (email.trim()[0] ?? "?").toUpperCase();

const renderNotConfiguredCard = (options: {
  title: string;
  detail: string;
  hint?: string;
}): string => `
  <div class="gmail-card">
    <div class="gmail-card-icon"><i data-lucide="circle-alert"></i></div>
    <div class="gmail-card-copy">
      <strong>${options.title}</strong>
      <p>${options.detail}</p>
      ${options.hint ? `<p class="gmail-hint">${options.hint}</p>` : ""}
    </div>
  </div>`;

const renderConnectCard = (options: {
  title: string;
  detail: string;
  hint?: string;
  href: string;
  label: string;
}): string => `
  <div class="gmail-card">
    <div class="gmail-card-icon"><i data-lucide="mail-plus"></i></div>
    <div class="gmail-card-copy">
      <strong>${options.title}</strong>
      <p>${options.detail}</p>
      ${options.hint ? `<p class="gmail-hint">${options.hint}</p>` : ""}
    </div>
    <a class="tool-button primary" href="${escapeHtml(options.href)}">${options.label}</a>
  </div>`;

const renderAccountCard = (options: {
  provider: "gmail" | "outlook";
  email: string;
  needsRelink: boolean;
  linkedAt: number | null;
  disconnectId: string;
  relinkHref: string;
  defaultLabel: string | null;
}): string => {
  const { provider, email, needsRelink, linkedAt, disconnectId, relinkHref, defaultLabel } = options;
  return `
    <div class="gmail-account">
      <span class="gmail-account-avatar" aria-hidden="true">${escapeHtml(accountInitial(email))}</span>
      <div class="gmail-account-copy">
        <strong>${escapeHtml(email)}</strong>
        <small>${needsRelink ? "Autorisation à renouveler" : linkedAt ? `Liée le ${formatLinkedAt(linkedAt)}` : "Connectée"}${defaultLabel ? ` · ${defaultLabel}` : ""}</small>
      </div>
      ${needsRelink ? `<a class="tool-button primary" href="${escapeHtml(relinkHref)}">Relier</a>` : ""}
      <button type="button" class="tool-button ${provider === "gmail" ? "gmail-account-disconnect" : "gmail-outlook-account-disconnect"}" data-id="${escapeHtml(disconnectId)}" title="Délier cette boîte">Déconnecter</button>
    </div>`;
};

const renderAccountMessages = (options: {
  count: number;
  loading: boolean;
  error: string | null;
  emptyLabel: string;
  rows: string;
}): string => {
  const { count, loading, error, emptyLabel, rows } = options;
  if (error) {
    return `<div class="gmail-error" role="alert"><i data-lucide="circle-alert"></i><span>${escapeHtml(error)}</span></div>`;
  }
  if (loading && count === 0) {
    return `<div class="gmail-loading" role="status"><i data-lucide="loader-circle" class="is-spinning"></i><span>Chargement des messages…</span></div>`;
  }
  if (count === 0) {
    return `<div class="gmail-empty"><i data-lucide="inbox"></i><span>${emptyLabel}</span></div>`;
  }
  return `<div class="gmail-messages">${rows}</div>`;
};

const renderGmailAccounts = (): string => {
  const view = connection ?? OFFLINE_CONNECTION;
  if (!view.connected && view.accounts.length === 0) {
    if (!view.configured) {
      return renderNotConfiguredCard({
        title: "Intégration Gmail non configurée",
        detail:
          "Définis CST_GOOGLE_CLIENT_ID et CST_GOOGLE_CLIENT_SECRET sur le serveur, puis déclare l'URI de redirection dans Google Cloud Console :",
        hint: view.redirectUri ? `<code>${escapeHtml(view.redirectUri)}</code>` : undefined,
      });
    }
    return renderConnectCard({
      title: view.needsRelink ? "Autorisation à renouveler" : "Connecter une boîte Gmail",
      detail: view.needsRelink
        ? "L'accès Google a expiré. Relie à nouveau ta boîte."
        : "L'onglet lit les messages (lecture seule). Aucun mot de passe n'est stocké ici.",
      hint: view.redirectUri
        ? `URI de redirection à déclarer dans Google Cloud Console : <code>${escapeHtml(view.redirectUri)}</code>`
        : undefined,
      href: `${remoteBaseUrl()}/api/gmail/start`,
      label: view.needsRelink ? "Relier à nouveau" : "Se connecter à Gmail",
    });
  }
  const startHref = `${remoteBaseUrl()}/api/gmail/start`;
  return `
    ${view.accounts
      .map(
        (account) => `
        <div class="gmail-account-block">
          ${renderAccountCard({
            provider: "gmail",
            email: account.email,
            needsRelink: account.needsRelink,
            linkedAt: account.linkedAt,
            disconnectId: account.linkId,
            relinkHref: startHref,
            defaultLabel: account.isDefault ? "boîte par défaut" : null,
          })}
          ${account.needsRelink ? "" : renderAccountMessages({
            count: (gmailMessages[account.linkId] ?? []).length,
            loading: messagesLoading,
            error: lastError,
            emptyLabel: "Aucun message dans la boîte de réception.",
            rows: (gmailMessages[account.linkId] ?? [])
              .map(renderMessageRow)
              .join(""),
          })}
        </div>`,
      )
      .join("")}
    <a class="gmail-add-account" href="${escapeHtml(startHref)}"><i data-lucide="plus"></i><span>Ajouter un compte Gmail</span></a>`;
};

const renderOutlookAccounts = (): string => {
  const view = outlookConnection;
  if (!view?.configured) {
    return renderNotConfiguredCard({
      title: "Outlook non configuré",
      detail:
        "Définis CST_MICROSOFT_CLIENT_ID et CST_MICROSOFT_CLIENT_SECRET sur le serveur (voir le guide Microsoft 365).",
      hint: view?.redirectUri ? `<code>${escapeHtml(view.redirectUri)}</code>` : undefined,
    });
  }
  const accounts = view.accounts ?? [];
  const startHref = view.loginUrl ?? `${remoteBaseUrl()}/api/microsoft/start`;
  if (accounts.length === 0) {
    return renderConnectCard({
      title: "Connecter une boîte Outlook",
      detail: "L'onglet lit les messages de ta boîte Microsoft 365.",
      href: startHref,
      label: "Se connecter à Outlook",
    });
  }
  return `
    ${accounts
      .map(
        (account) => `
        <div class="gmail-account-block">
          ${renderAccountCard({
            provider: "outlook",
            email: account.email,
            needsRelink: account.needsRelink,
            linkedAt: account.linkedAt,
            disconnectId: account.oid,
            relinkHref: startHref,
            defaultLabel: account.isDefault ? "boîte par défaut" : null,
          })}
          ${account.needsRelink ? "" : renderAccountMessages({
            count: (outlookMessages[account.oid] ?? []).length,
            loading: outlookLoading,
            error: outlookError,
            emptyLabel: "Aucun message dans la boîte de réception.",
            rows: (outlookMessages[account.oid] ?? [])
              .map(renderOutlookMessageRow)
              .join(""),
          })}
        </div>`,
      )
      .join("")}
    <a class="gmail-add-account" href="${escapeHtml(startHref)}"><i data-lucide="plus"></i><span>Ajouter un compte Outlook</span></a>`;
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
  body: string;
}): string => {
  const { id, title, unread, body } = options;
  return `
    <section class="gmail-provider" aria-labelledby="${id}Title">
      <header class="gmail-provider-head">
        <div class="gmail-provider-title"><span class="gmail-provider-logo">${title === "Gmail" ? "G" : "O"}</span><h3 id="${id}Title">${title}</h3>${unread ? `<b>${unread > 99 ? "99+" : unread}</b>` : ""}</div>
        ${fetchedAt ? `<span class="gmail-refreshed">Actualisé ${formatRelativeTime(fetchedAt)}</span>` : ""}
      </header>
      ${body}
    </section>`;
};

export const renderGmailPanel = (): string => {
  if (!connectionLoaded && !connectionLoading) void ensureConnectionLoaded();
  const unreadCount = gmailUnreadCount();
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
          unread: Object.values(gmailMessages)
            .flat()
            .filter((message) => message.unread).length,
          body: renderGmailAccounts(),
        })}
        ${renderProviderSection({
          id: "outlookProvider",
          title: "Outlook",
          unread: Object.values(outlookMessages)
            .flat()
            .filter((message) => !message.isRead).length,
          body: renderOutlookAccounts(),
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
  document.querySelectorAll<HTMLButtonElement>(".gmail-account-disconnect").forEach((button) => {
    button.addEventListener("click", () => {
      const linkId = button.dataset.id;
      if (!linkId) return;
      if (window.confirm("Délier cette boîte Gmail ? Les messages restent dans ta boîte.")) {
        void disconnectGmail(linkId);
      }
    });
  });
  document.querySelectorAll<HTMLButtonElement>(".gmail-outlook-account-disconnect").forEach((button) => {
    button.addEventListener("click", () => {
      const oid = button.dataset.id;
      if (!oid) return;
      if (window.confirm("Délier cette boîte Outlook ? Les messages restent dans ta boîte.")) {
        void disconnectOutlook(oid);
      }
    });
  });
};
