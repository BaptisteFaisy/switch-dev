import { invoke, isRemoteMode } from "./platform";
import { authenticatedUser } from "./user-auth";
import {
  chatImageAttachmentPayloads,
  clipboardChatImageFiles,
  disposeChatImagePreviews,
  MAX_CHAT_IMAGE_ATTACHMENTS,
  readChatImageAttachments,
  type ChatImageAttachment,
} from "./chat/image-attachments";
import {
  PRIVATE_MESSAGE_CAMPAIGN_MAX_RECIPIENTS,
  PRIVATE_MESSAGE_CAMPAIGN_MAX_VARIANTS,
  PRIVATE_MESSAGE_CAMPAIGN_MIN_INTERVAL_SECONDS,
  PRIVATE_MESSAGE_MAX_CHARS,
  filterPrivateMessageUsers,
  privateMessageCampaignCounts,
  privateMessagingUnreadCount,
  renderPrivateMessageCampaignTemplate,
  sortPrivateConversations,
  sortPrivateMessageUsers,
  type CreatePrivateMessageCampaignRequest,
  type PrivateConversation,
  type PrivateConversationSummary,
  type PrivateMessage,
  type PrivateMessageCampaign,
  type PrivateMessageCampaignStatus,
  type PrivateMessageImage,
  type PrivateMessageImageContent,
  type PrivateMessageUser,
} from "./messaging-model";
import "./messaging.css";

const MESSAGING_POLL_INTERVAL_MS = 8_000;
// Hors de la vue Messagerie, seul le badge de non-lus est utile : les listes
// d'utilisateurs, campagnes et conversations n'ont pas besoin d'etre relues
// aussi souvent.
const MESSAGING_BACKGROUND_POLL_INTERVAL_MS = 30_000;

export type MessagingUiOptions = {
  rerender: () => void;
  setStatus?: (message: string) => void;
};

let messagingUsers: PrivateMessageUser[] = [];
let messagingConversations: PrivateConversationSummary[] = [];
let messagingConversation: PrivateConversation | null = null;
let messagingSelectedUserId: string | null = null;
let messagingLoaded = false;
let messagingLoading = false;
let messagingThreadLoading = false;
let messagingSubmitting = false;
let messagingError = "";
let messagingSearch = "";
let messagingDraft = "";
let messagingImageAttachments: ChatImageAttachment[] = [];
let messagingBrowseUsers = false;
let messagingMobileDetailOpen = false;
let messagingRefreshPromise: Promise<boolean> | null = null;
let messagingDetailRequest = 0;
let messagingPollTimer: number | null = null;
let messagingPollRerender: (() => void) | null = null;
let messagingRealtimeAvailable = false;
let messagingVisible = false;
let messagingOpenImageId: string | null = null;
let messagingImageViewerScrollTop: number | null = null;
const messagingImageUrls = new Map<string, string>();
const messagingImageLoads = new Set<string>();
const messagingImageFailures = new Set<string>();
let messagingImageObserver: IntersectionObserver | null = null;
let messagingCampaigns: PrivateMessageCampaign[] = [];
let messagingCampaignsOpen = false;
let messagingCampaignSubmitting = false;
let messagingCampaignError = "";
let messagingCampaignName = "";
let messagingCampaignVariants = "Bonjour {{username}},";
let messagingCampaignIntervalSeconds = 30;
let messagingCampaignConsent = false;
let messagingCampaignPreview = false;
let messagingCampaignRecipientIds = new Set<string>();

const escapeHtml = (value: string) =>
  value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");

const escapeAttr = escapeHtml;

const errorMessage = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

const currentMessagingUserId = () =>
  authenticatedUser()?.id ?? (isRemoteMode() ? "server-admin" : "local-user");

const messagingSnapshot = () => JSON.stringify({
  messagingUsers,
  messagingConversations,
  messagingConversation,
  messagingSelectedUserId,
  messagingLoaded,
  messagingError,
  messagingCampaigns,
});

const safeAvatarUrl = (value: string | null | undefined) => {
  if (!value) return null;
  try {
    const url = new URL(value, window.location.origin);
    return url.protocol === "https:" || url.protocol === "http:" ? url.href : null;
  } catch {
    return null;
  }
};

const userInitials = (user: PrivateMessageUser) => {
  const parts = user.username.trim().split(/[\s._-]+/).filter(Boolean);
  return (parts.slice(0, 2).map((part) => part[0]?.toUpperCase() ?? "").join("") || "U")
    .slice(0, 2);
};

const renderMessagingAvatar = (
  user: PrivateMessageUser,
  size: "small" | "large" = "small",
) => {
  const avatar = safeAvatarUrl(user.avatarUrl);
  return avatar
    ? `<img class="messaging-avatar ${size}" src="${escapeAttr(avatar)}" alt="" referrerpolicy="no-referrer" />`
    : `<span class="messaging-avatar ${size}" aria-hidden="true">${escapeHtml(userInitials(user))}</span>`;
};

const messageDate = (timestamp: number) => {
  const date = new Date(timestamp * 1_000);
  if (!Number.isFinite(date.getTime())) return "Date inconnue";
  return new Intl.DateTimeFormat("fr-FR", {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
};

const messageTime = (timestamp: number) => {
  const date = new Date(timestamp * 1_000);
  if (!Number.isFinite(date.getTime())) return "";
  const today = new Date();
  const sameDay = date.toDateString() === today.toDateString();
  return new Intl.DateTimeFormat("fr-FR", sameDay
    ? { hour: "2-digit", minute: "2-digit" }
    : { day: "2-digit", month: "short", hour: "2-digit", minute: "2-digit" })
    .format(date);
};

const messageDateTime = (timestamp: number) => {
  const date = new Date(timestamp * 1_000);
  return Number.isFinite(date.getTime()) ? date.toISOString() : "";
};

const compactMessage = (body: string, max = 84) => {
  const value = body.replace(/\s+/g, " ").trim();
  return [...value].length > max ? `${[...value].slice(0, max).join("")}…` : value;
};

const privateMessageImages = (message: PrivateMessage): PrivateMessageImage[] =>
  Array.isArray(message.images) ? message.images : [];

const privateMessagePreview = (message: PrivateMessage) => {
  if (message.body.trim()) return compactMessage(message.body);
  const imageCount = privateMessageImages(message).length;
  return imageCount > 1 ? `${imageCount} images` : imageCount === 1 ? "Une image" : "Message";
};

const renderMessagingDraftImages = () => {
  if (!messagingImageAttachments.length) return "";
  return `<div class="messaging-draft-images" aria-label="Images à envoyer">
    ${messagingImageAttachments.map((image) => `<figure>
      <img src="${escapeAttr(image.previewUrl)}" alt="${escapeAttr(image.name)}" />
      <figcaption title="${escapeAttr(image.name)}">${escapeHtml(image.name)}</figcaption>
      <button type="button" data-messaging-remove-image="${escapeAttr(image.id)}" ${messagingSubmitting ? "disabled" : ""} aria-label="Retirer ${escapeAttr(image.name)}"><i data-lucide="x"></i></button>
    </figure>`).join("")}
  </div>`;
};

const renderPrivateMessageImage = (image: PrivateMessageImage) => {
  const url = messagingImageUrls.get(image.id);
  if (url) {
    return `<button type="button" class="messaging-message-image" data-messaging-open-image="${escapeAttr(image.id)}" aria-label="Agrandir ${escapeAttr(image.name)}">
      <img src="${escapeAttr(url)}" alt="${escapeAttr(image.name)}" loading="lazy" />
    </button>`;
  }
  if (messagingImageFailures.has(image.id)) {
    return `<button type="button" class="messaging-message-image is-error" data-messaging-retry-image="${escapeAttr(image.id)}">
      <i data-lucide="image-off"></i><span>Réessayer</span>
    </button>`;
  }
  return `<span class="messaging-message-image is-loading" data-messaging-pending-image="${escapeAttr(image.id)}" role="status" aria-label="Chargement de ${escapeAttr(image.name)}"><i data-lucide="image"></i></span>`;
};

const findPrivateMessageImage = (imageId: string): PrivateMessageImage | null => {
  for (const message of messagingConversation?.messages ?? []) {
    const image = privateMessageImages(message).find((entry) => entry.id === imageId);
    if (image) return image;
  }
  return null;
};

const renderMessagingImageViewer = () => {
  if (!messagingOpenImageId) return "";
  const image = findPrivateMessageImage(messagingOpenImageId);
  const url = image ? messagingImageUrls.get(image.id) : null;
  if (!image || !url) return "";
  return `<div class="messaging-image-viewer" role="dialog" aria-modal="true" aria-label="${escapeAttr(image.name)}" data-messaging-image-viewer>
    <figure>
      <img src="${escapeAttr(url)}" alt="${escapeAttr(image.name)}" />
      <figcaption>${escapeHtml(image.name)}</figcaption>
    </figure>
    <button type="button" data-messaging-close-image aria-label="Fermer l’image"><i data-lucide="x"></i></button>
  </div>`;
};

const campaignStatusLabel = (status: PrivateMessageCampaignStatus) => ({
  draft: "Brouillon",
  running: "En cours",
  paused: "En pause",
  completed: "Terminée",
  cancelled: "Annulée",
  failed: "Échec",
})[status];

const campaignVariants = () => messagingCampaignVariants
  .split(/\n\s*---+\s*\n/g)
  .map((variant) => variant.trim())
  .filter(Boolean)
  .slice(0, PRIVATE_MESSAGE_CAMPAIGN_MAX_VARIANTS);

const renderMessagingCampaignCard = (campaign: PrivateMessageCampaign) => {
  const counts = privateMessageCampaignCounts(campaign);
  const progress = counts.total ? Math.round((counts.sent / counts.total) * 100) : 0;
  const action = campaign.status === "running"
    ? `<button type="button" data-campaign-action="pause" data-campaign-id="${escapeAttr(campaign.id)}"><i data-lucide="pause"></i>Pause</button>`
    : campaign.status === "paused"
      ? `<button type="button" data-campaign-action="resume" data-campaign-id="${escapeAttr(campaign.id)}"><i data-lucide="play"></i>Reprendre</button>`
      : campaign.status === "draft"
        ? `<button type="button" data-campaign-action="start" data-campaign-id="${escapeAttr(campaign.id)}"><i data-lucide="play"></i>Démarrer</button>`
        : "";
  const cancellable = ["draft", "running", "paused"].includes(campaign.status)
    ? `<button type="button" class="danger" data-campaign-action="cancel" data-campaign-id="${escapeAttr(campaign.id)}"><i data-lucide="circle-x"></i>Annuler</button>`
    : "";
  return `<article class="messaging-campaign-card">
    <header>
      <span class="messaging-campaign-status is-${escapeAttr(campaign.status)}">${escapeHtml(campaignStatusLabel(campaign.status))}</span>
      <small>${campaign.createdBy === "autonomous_agent" ? "Agent autonome" : "Créée manuellement"}</small>
    </header>
    <h4>${escapeHtml(campaign.name)}</h4>
    <p>${counts.sent}/${counts.total} envoyés${counts.failed ? ` · ${counts.failed} échec(s)` : ""} · cadence ${campaign.intervalSeconds}s</p>
    <div class="messaging-campaign-progress"><span style="width:${progress}%"></span></div>
    <footer>${action}${cancellable}</footer>
  </article>`;
};

const renderMessagingCampaignPreview = () => {
  if (!messagingCampaignPreview) return "";
  const recipients = messagingUsers.filter((user) => messagingCampaignRecipientIds.has(user.id));
  const variants = campaignVariants();
  if (!recipients.length || !variants.length) {
    return `<div class="messaging-campaign-preview is-empty">Sélectionnez des destinataires et ajoutez au moins un message.</div>`;
  }
  return `<section class="messaging-campaign-preview">
    <header><strong>Aperçu personnalisé</strong><small>${recipients.length} destinataire(s) · ${variants.length} variante(s)</small></header>
    ${recipients.slice(0, 3).map((user, index) => `<article>
      ${renderMessagingAvatar(user)}
      <span><strong>${escapeHtml(user.username)}</strong><p>${escapeHtml(renderPrivateMessageCampaignTemplate(
        variants[index % variants.length],
        user,
        index + 1,
        recipients.length,
      ))}</p></span>
    </article>`).join("")}
  </section>`;
};

const renderMessagingCampaigns = () => {
  if (!messagingCampaignsOpen) return "";
  const selectedCount = messagingCampaignRecipientIds.size;
  return `<div class="messaging-campaign-backdrop" data-campaign-close>
    <section class="messaging-campaign-dialog" role="dialog" aria-modal="true" aria-labelledby="messagingCampaignTitle">
      <header>
        <div><span>Diffusion consentie</span><h3 id="messagingCampaignTitle">Campagnes de messages</h3><p>Personnalisez, cadrez et suivez chaque envoi depuis un seul endroit.</p></div>
        <button type="button" data-campaign-close aria-label="Fermer"><i data-lucide="x"></i></button>
      </header>
      ${messagingCampaignError ? `<div class="messaging-error" role="alert"><i data-lucide="circle-alert"></i><span>${escapeHtml(messagingCampaignError)}</span></div>` : ""}
      <div class="messaging-campaign-dialog-grid">
        <form id="messagingCampaignForm" class="messaging-campaign-form">
          <label><span>Nom de campagne</span><input id="campaignName" maxlength="120" value="${escapeAttr(messagingCampaignName)}" placeholder="Ex. Invitation bêta juillet" /></label>
          <label><span>Messages <small>séparez les variantes par ---</small></span><textarea id="campaignVariants" maxlength="${PRIVATE_MESSAGE_MAX_CHARS * PRIVATE_MESSAGE_CAMPAIGN_MAX_VARIANTS}" rows="6" placeholder="Bonjour {{username}},…">${escapeHtml(messagingCampaignVariants)}</textarea></label>
          <div class="messaging-campaign-tokens"><span>Variables :</span><code>{{username}}</code><code>{{index}}</code><code>{{total}}</code></div>
          <fieldset>
            <legend><span>Destinataires <b>${selectedCount}/${PRIVATE_MESSAGE_CAMPAIGN_MAX_RECIPIENTS}</b></span><button type="button" data-campaign-select-all>Tout sélectionner</button></legend>
            <div class="messaging-campaign-recipients">
              ${messagingUsers.length ? messagingUsers.map((user) => `<label class="${messagingCampaignRecipientIds.has(user.id) ? "is-selected" : ""}">
                <input type="checkbox" data-campaign-recipient="${escapeAttr(user.id)}" ${messagingCampaignRecipientIds.has(user.id) ? "checked" : ""} />
                ${renderMessagingAvatar(user)}<span>${escapeHtml(user.username)}</span><i data-lucide="check"></i>
              </label>`).join("") : `<p>Aucun autre utilisateur disponible.</p>`}
            </div>
          </fieldset>
          <label class="messaging-campaign-interval"><span>Cadence entre deux messages</span><input id="campaignInterval" type="number" min="${PRIVATE_MESSAGE_CAMPAIGN_MIN_INTERVAL_SECONDS}" max="3600" value="${messagingCampaignIntervalSeconds}" /><small>secondes</small></label>
          <label class="messaging-campaign-consent"><input id="campaignConsent" type="checkbox" ${messagingCampaignConsent ? "checked" : ""} /><span><strong>Audience autorisée</strong><small>Je confirme que ces destinataires ont accepté ces messages et qu’une demande d’arrêt sera respectée.</small></span></label>
          ${renderMessagingCampaignPreview()}
          <footer>
            <button type="button" class="tool-button" data-campaign-preview><i data-lucide="scan-eye"></i><span>Aperçu</span></button>
            <span>
              <button type="submit" class="tool-button" data-campaign-submit="draft" ${messagingCampaignSubmitting ? "disabled" : ""}><i data-lucide="save"></i><span>Brouillon</span></button>
              <button type="submit" class="tool-button primary" data-campaign-submit="start" ${messagingCampaignSubmitting ? "disabled" : ""}><i data-lucide="send"></i><span>${messagingCampaignSubmitting ? "Création…" : "Lancer"}</span></button>
            </span>
          </footer>
        </form>
        <aside class="messaging-campaign-history">
          <header><div><strong>Activité</strong><small>${messagingCampaigns.length} campagne(s)</small></div><button type="button" data-campaign-refresh aria-label="Actualiser"><i data-lucide="refresh-ccw"></i></button></header>
          <div>${messagingCampaigns.length
            ? messagingCampaigns.map(renderMessagingCampaignCard).join("")
            : `<div class="messaging-campaign-empty"><i data-lucide="send"></i><strong>Aucune campagne</strong><p>Votre première campagne apparaîtra ici avec sa progression.</p></div>`}</div>
        </aside>
      </div>
    </section>
  </div>`;
};

const conversationByUserId = (userId: string) =>
  messagingConversations.find((conversation) => conversation.user.id === userId) ?? null;

const renderConversationRows = () => {
  if (messagingLoading && !messagingLoaded) {
    return `<div class="messaging-list-loading" aria-label="Chargement des conversations">
      ${Array.from({ length: 5 }, () => '<span class="messaging-row-skeleton"></span>').join("")}
    </div>`;
  }

  const query = messagingSearch.trim().toLocaleLowerCase("fr");
  const conversations = sortPrivateConversations(messagingConversations).filter((conversation) =>
    !query || conversation.user.username.toLocaleLowerCase("fr").includes(query)
      || conversation.lastMessage.body.toLocaleLowerCase("fr").includes(query)
      || privateMessageImages(conversation.lastMessage).some((image) =>
        image.name.toLocaleLowerCase("fr").includes(query)),
  );
  if (!conversations.length) {
    return `<div class="messaging-list-empty">
      <span><i data-lucide="mail"></i></span>
      <strong>${messagingSearch ? "Aucun résultat" : "Aucune conversation"}</strong>
      <p>${messagingSearch ? "Essayez un autre nom ou démarrez un nouveau message." : "Choisissez un utilisateur pour démarrer un échange privé."}</p>
      <button type="button" class="tool-button" data-messaging-browse-users><i data-lucide="user-plus"></i><span>Nouveau message</span></button>
    </div>`;
  }

  return conversations.map((conversation) => {
    const active = conversation.user.id === messagingSelectedUserId;
    const mine = conversation.lastMessage.sender.id === currentMessagingUserId();
    return `<button
      type="button"
      class="messaging-conversation-row${active ? " active" : ""}${conversation.unreadCount ? " unread" : ""}"
      data-private-message-user-id="${escapeAttr(conversation.user.id)}"
      aria-current="${active ? "true" : "false"}"
    >
      ${renderMessagingAvatar(conversation.user, "large")}
      <span class="messaging-row-copy">
        <span><strong>${escapeHtml(conversation.user.username)}</strong><time datetime="${escapeAttr(messageDateTime(conversation.lastMessage.createdAt))}" title="${escapeAttr(messageDate(conversation.lastMessage.createdAt))}">${escapeHtml(messageTime(conversation.lastMessage.createdAt))}</time></span>
        <small>${mine ? "Vous : " : ""}${privateMessageImages(conversation.lastMessage).length ? '<i data-lucide="image"></i>' : ""}${escapeHtml(privateMessagePreview(conversation.lastMessage))}</small>
      </span>
      ${conversation.unreadCount ? `<b class="messaging-unread-count" aria-label="${conversation.unreadCount} message${conversation.unreadCount === 1 ? "" : "s"} non lu${conversation.unreadCount === 1 ? "" : "s"}">${conversation.unreadCount > 99 ? "99+" : conversation.unreadCount}</b>` : ""}
    </button>`;
  }).join("");
};

const renderUserRows = () => {
  const users = filterPrivateMessageUsers(messagingUsers, messagingSearch);
  if (messagingLoading && !messagingLoaded) {
    return `<div class="messaging-list-loading" aria-label="Chargement des utilisateurs">
      ${Array.from({ length: 5 }, () => '<span class="messaging-row-skeleton compact"></span>').join("")}
    </div>`;
  }
  if (!users.length) {
    return `<div class="messaging-list-empty">
      <span><i data-lucide="users"></i></span>
      <strong>${messagingSearch ? "Utilisateur introuvable" : "Aucun autre utilisateur"}</strong>
      <p>${isRemoteMode()
        ? "Les nouveaux comptes apparaîtront ici dès leur inscription."
        : "Connectez cette application au serveur web pour échanger entre utilisateurs."}</p>
    </div>`;
  }
  return users.map((user) => {
    const active = user.id === messagingSelectedUserId;
    const existing = conversationByUserId(user.id);
    return `<button
      type="button"
      class="messaging-user-row${active ? " active" : ""}"
      data-private-message-user-id="${escapeAttr(user.id)}"
      aria-current="${active ? "true" : "false"}"
    >
      ${renderMessagingAvatar(user, "large")}
      <span><strong>${escapeHtml(user.username)}</strong><small>${existing ? "Conversation existante" : "Démarrer une conversation"}</small></span>
      <i data-lucide="${existing ? "mail" : "message-square-plus"}"></i>
    </button>`;
  }).join("");
};

const renderMessageBubble = (message: PrivateMessage) => {
  const mine = message.sender.id === currentMessagingUserId();
  const images = privateMessageImages(message);
  return `<article class="messaging-message${mine ? " mine" : " theirs"}">
    <div class="messaging-message-bubble${images.length ? " has-images" : ""}">
      ${images.length ? `<div class="messaging-message-images count-${Math.min(images.length, 4)}">${images.map(renderPrivateMessageImage).join("")}</div>` : ""}
      ${message.body ? `<p>${escapeHtml(message.body)}</p>` : ""}
    </div>
    <footer>
      <time datetime="${escapeAttr(messageDateTime(message.createdAt))}" title="${escapeAttr(messageDate(message.createdAt))}">${escapeHtml(messageTime(message.createdAt))}</time>
      ${mine ? `<span title="${message.readAt ? "Lu" : "Envoyé"}" aria-label="${message.readAt ? "Message lu" : "Message envoyé"}"><i data-lucide="${message.readAt ? "check-check" : "check"}"></i></span>` : ""}
    </footer>
  </article>`;
};

const renderMessagingThread = () => {
  if (!messagingSelectedUserId) {
    return `<div class="messaging-thread-empty">
      <span><i data-lucide="mail"></i></span>
      <h3>Vos messages privés</h3>
      <p>Sélectionnez une conversation ou choisissez un utilisateur. Seuls vous et votre destinataire pourrez lire les messages.</p>
      <button type="button" class="tool-button primary" data-messaging-browse-users><i data-lucide="user-plus"></i><span>Nouveau message</span></button>
    </div>`;
  }
  if (messagingThreadLoading || !messagingConversation) {
    return `<div class="messaging-thread-loading" aria-label="Chargement de la conversation">
      <span></span><span></span><span></span>
    </div>`;
  }

  const conversation = messagingConversation;
  const messages = [...conversation.messages].sort((left, right) =>
    left.sequence - right.sequence || left.createdAt - right.createdAt,
  );
  return `<section class="messaging-thread" aria-labelledby="messagingThreadTitle">
    <header class="messaging-thread-head">
      <button type="button" class="messaging-mobile-back" data-messaging-back aria-label="Retour aux conversations"><i data-lucide="arrow-left"></i></button>
      ${renderMessagingAvatar(conversation.user, "large")}
      <div><h3 id="messagingThreadTitle" tabindex="-1">${escapeHtml(conversation.user.username)}</h3><p><i data-lucide="lock-keyhole"></i> Conversation privée</p></div>
    </header>
    <div class="messaging-thread-feed" id="messagingThreadFeed" aria-live="polite">
      ${messages.length
        ? messages.map(renderMessageBubble).join("")
        : `<div class="messaging-first-message"><span><i data-lucide="mail"></i></span><strong>Commencez la conversation</strong><p>Envoyez votre premier message privé à ${escapeHtml(conversation.user.username)}.</p></div>`}
    </div>
    <form id="privateMessageForm" class="messaging-composer">
      <div>
        <textarea id="privateMessageBody" maxlength="${PRIVATE_MESSAGE_MAX_CHARS}" rows="3" placeholder="Écrire un message privé…">${escapeHtml(messagingDraft)}</textarea>
        ${renderMessagingDraftImages()}
        <footer>
          <label class="messaging-attach-image${messagingSubmitting ? " is-disabled" : ""}" title="Joindre des images">
            <input id="privateMessageImages" type="file" accept="image/png,image/jpeg,image/webp" multiple ${messagingSubmitting ? "disabled" : ""} />
            <i data-lucide="image"></i><span>Image</span>
          </label>
          <small><span><kbd>Ctrl</kbd> + <kbd>Entrée</kbd> pour envoyer</span><b data-private-message-count>${[...messagingDraft].length}</b>/${PRIVATE_MESSAGE_MAX_CHARS}</small>
          <button type="submit" class="tool-button primary" ${messagingSubmitting ? "disabled" : ""} aria-label="Envoyer le message">
            ${messagingSubmitting ? '<i data-lucide="loader-circle" class="is-spinning"></i><span>Envoi…</span>' : '<i data-lucide="send"></i><span>Envoyer</span>'}
          </button>
        </footer>
      </div>
    </form>
  </section>`;
};

export const renderMessagingPanel = (): string => {
  const unreadCount = privateMessagingUnreadCount(messagingConversations);
  const browseUsers = messagingBrowseUsers || (!messagingConversations.length && messagingLoaded);
  return `
    <section id="messagingPanel" class="messaging-panel${messagingMobileDetailOpen ? " is-detail-open" : ""}" aria-labelledby="messagingPanelTitle">
      <header class="messaging-hero">
        <div class="messaging-hero-mark"><i data-lucide="mail"></i>${unreadCount ? `<b>${unreadCount > 99 ? "99+" : unreadCount}</b>` : ""}</div>
        <div><span>Échanges directs</span><h2 id="messagingPanelTitle">Messagerie</h2><p>Discutez en privé avec les autres utilisateurs.</p></div>
        <span class="messaging-privacy"><i data-lucide="shield-check"></i><span><strong>Privé</strong><small>Deux participants</small></span></span>
        <div class="messaging-hero-actions">
          <button type="button" id="messagingCampaigns" class="tool-button"><i data-lucide="send"></i><span>Campagnes</span></button>
          <button type="button" id="messagingNew" class="tool-button primary"><i data-lucide="message-square-plus"></i><span>Nouveau message</span></button>
        </div>
      </header>
      ${messagingError ? `<div class="messaging-error" role="alert"><i data-lucide="circle-alert"></i><span>${escapeHtml(messagingError)}</span><button type="button" data-messaging-dismiss-error aria-label="Fermer"><i data-lucide="x"></i></button></div>` : ""}
      <div class="messaging-layout">
        <aside class="messaging-list" aria-label="${browseUsers ? "Utilisateurs" : "Conversations privées"}">
          <header>
            <div class="messaging-list-tabs">
              <button type="button" class="${browseUsers ? "" : "active"}" data-messaging-show-conversations>Messages${unreadCount ? `<b>${unreadCount > 99 ? "99+" : unreadCount}</b>` : ""}</button>
              <button type="button" class="${browseUsers ? "active" : ""}" data-messaging-browse-users>Utilisateurs</button>
            </div>
            <button type="button" id="messagingRefresh" aria-label="Actualiser la messagerie" title="Actualiser"><i data-lucide="refresh-ccw"></i></button>
          </header>
          <label class="messaging-search"><i data-lucide="search"></i><input id="messagingSearch" type="search" value="${escapeAttr(messagingSearch)}" placeholder="${browseUsers ? "Rechercher un utilisateur" : "Rechercher un message"}" /></label>
          <div class="messaging-list-scroll" aria-live="polite">${browseUsers ? renderUserRows() : renderConversationRows()}</div>
        </aside>
        <main class="messaging-detail">${renderMessagingThread()}</main>
      </div>
      ${renderMessagingImageViewer()}
      ${renderMessagingCampaigns()}
    </section>`;
};

const scrollMessagingFeedToBottom = () => window.requestAnimationFrame(() => {
  const feed = document.querySelector<HTMLElement>("#messagingThreadFeed");
  if (feed) feed.scrollTop = feed.scrollHeight;
});

const restoreMessagingFeedScroll = (clearAfterRestore = false) => {
  const scrollTop = messagingImageViewerScrollTop;
  if (clearAfterRestore) messagingImageViewerScrollTop = null;
  if (scrollTop === null) return;
  window.requestAnimationFrame(() => {
    const feed = document.querySelector<HTMLElement>("#messagingThreadFeed");
    if (feed) feed.scrollTop = scrollTop;
  });
};

const focusMessagingSearch = () => window.requestAnimationFrame(() => {
  const input = document.querySelector<HTMLInputElement>("#messagingSearch");
  input?.focus();
  input?.setSelectionRange(input.value.length, input.value.length);
});

const focusMessagingComposer = () => window.requestAnimationFrame(() => {
  const input = document.querySelector<HTMLTextAreaElement>("#privateMessageBody");
  input?.focus();
  input?.setSelectionRange(input.value.length, input.value.length);
});

const currentMessagingImageIds = () => new Set(
  (messagingConversation?.messages ?? [])
    .flatMap((message) => privateMessageImages(message))
    .map((image) => image.id),
);

const retainCurrentMessagingImages = () => {
  const activeIds = currentMessagingImageIds();
  for (const [imageId, url] of messagingImageUrls) {
    if (activeIds.has(imageId)) continue;
    URL.revokeObjectURL(url);
    messagingImageUrls.delete(imageId);
  }
  for (const imageId of messagingImageFailures) {
    if (!activeIds.has(imageId)) messagingImageFailures.delete(imageId);
  }
  if (messagingOpenImageId && !activeIds.has(messagingOpenImageId)) {
    messagingOpenImageId = null;
  }
};

const privateMessageImageObjectUrl = (
  image: PrivateMessageImage,
  content: PrivateMessageImageContent,
) => {
  if (content.mimeType !== image.mimeType
    || !["image/png", "image/jpeg", "image/webp"].includes(content.mimeType)) {
    throw new Error("Format d’image inattendu.");
  }
  const encoded = content.dataBase64.trim();
  if (!encoded || encoded.length > 11_184_812 || !/^[A-Za-z0-9+/]*={0,2}$/.test(encoded)) {
    throw new Error("Données d’image invalides.");
  }
  let binary = "";
  try {
    binary = atob(encoded);
  } catch {
    throw new Error("Données d’image illisibles.");
  }
  if (!binary.length || binary.length > 8 * 1024 * 1024 || binary.length !== image.size) {
    throw new Error("Taille d’image invalide.");
  }
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return URL.createObjectURL(new Blob([bytes], { type: content.mimeType }));
};

const loadMessagingImages = (
  imageIds: readonly string[],
  options: MessagingUiOptions,
) => {
  const images = imageIds
    .map(findPrivateMessageImage)
    .filter((image): image is PrivateMessageImage => !!image)
    .filter((image) => !messagingImageUrls.has(image.id)
      && !messagingImageLoads.has(image.id)
      && !messagingImageFailures.has(image.id));
  if (!images.length) return;

  images.forEach((image) => messagingImageLoads.add(image.id));
  void Promise.all(images.map(async (image) => {
    try {
      const content = await invoke<PrivateMessageImageContent>("get_private_message_image", {
        imageId: image.id,
      });
      const url = privateMessageImageObjectUrl(image, content);
      if (currentMessagingImageIds().has(image.id)) {
        const previous = messagingImageUrls.get(image.id);
        if (previous) URL.revokeObjectURL(previous);
        messagingImageUrls.set(image.id, url);
      } else {
        URL.revokeObjectURL(url);
      }
    } catch {
      if (currentMessagingImageIds().has(image.id)) messagingImageFailures.add(image.id);
    } finally {
      messagingImageLoads.delete(image.id);
    }
  })).finally(() => {
    const feed = document.querySelector<HTMLElement>("#messagingThreadFeed");
    const scrollTop = feed?.scrollTop ?? 0;
    const stayedAtBottom = !feed
      || feed.scrollHeight - feed.scrollTop - feed.clientHeight <= 24;
    options.rerender();
    if (stayedAtBottom) {
      scrollMessagingFeedToBottom();
    } else {
      window.requestAnimationFrame(() => {
        const nextFeed = document.querySelector<HTMLElement>("#messagingThreadFeed");
        if (nextFeed) nextFeed.scrollTop = scrollTop;
      });
    }
  });
};

const observeVisibleMessagingImages = (options: MessagingUiOptions) => {
  messagingImageObserver?.disconnect();
  messagingImageObserver = null;
  const feed = document.querySelector<HTMLElement>("#messagingThreadFeed");
  const placeholders = Array.from(
    document.querySelectorAll<HTMLElement>("[data-messaging-pending-image]"),
  );
  if (!feed || !placeholders.length) return;

  if (!("IntersectionObserver" in window)) {
    loadMessagingImages(
      placeholders.map((placeholder) => placeholder.dataset.messagingPendingImage ?? ""),
      options,
    );
    return;
  }
  messagingImageObserver = new IntersectionObserver((entries, observer) => {
    const imageIds = entries
      .filter((entry) => entry.isIntersecting)
      .map((entry) => {
        observer.unobserve(entry.target);
        return (entry.target as HTMLElement).dataset.messagingPendingImage ?? "";
      })
      .filter(Boolean);
    if (imageIds.length) loadMessagingImages(imageIds, options);
  }, {
    root: feed,
    rootMargin: "320px 0px",
  });
  placeholders.forEach((placeholder) => messagingImageObserver?.observe(placeholder));
};

const loadMessagingThread = async (userId: string, rerender: () => void) => {
  const request = ++messagingDetailRequest;
  messagingSelectedUserId = userId;
  messagingBrowseUsers = false;
  messagingMobileDetailOpen = true;
  messagingThreadLoading = true;
  messagingError = "";
  if (messagingConversation?.user.id !== userId) {
    messagingConversation = null;
    retainCurrentMessagingImages();
  }
  rerender();
  try {
    const [thread, conversations] = await Promise.all([
      invoke<PrivateConversation>("get_private_message_conversation", { userId }),
      invoke<PrivateConversationSummary[]>("list_private_message_conversations"),
    ]);
    if (request !== messagingDetailRequest || messagingSelectedUserId !== userId) return;
    messagingConversation = thread;
    retainCurrentMessagingImages();
    messagingConversations = sortPrivateConversations(conversations).map((conversation) =>
      conversation.user.id === userId ? { ...conversation, unreadCount: 0 } : conversation,
    );
  } catch (error) {
    if (request !== messagingDetailRequest) return;
    messagingError = errorMessage(error);
  } finally {
    if (request === messagingDetailRequest) {
      messagingThreadLoading = false;
      rerender();
      scrollMessagingFeedToBottom();
      focusMessagingComposer();
    }
  }
};

export const refreshMessaging = (
  rerender: () => void,
  options: { silent?: boolean } = {},
): Promise<boolean> => {
  if (messagingRefreshPromise) return messagingRefreshPromise;
  const before = messagingSnapshot();
  const initialLoading = !messagingLoaded && !options.silent;
  if (initialLoading) {
    messagingLoading = true;
    rerender();
  }

  const refresh = (async () => {
    let success = false;
    try {
      const selectedId = messagingSelectedUserId;
      const threadIsVisible = messagingVisible
        && !!selectedId
        && (window.innerWidth > 860 || messagingMobileDetailOpen);
      const usersPromise = invoke<PrivateMessageUser[]>("list_private_message_users");
      const campaignsPromise = invoke<PrivateMessageCampaign[]>(
        "list_private_message_campaigns",
      );
      const threadPromise = selectedId && threadIsVisible
        ? invoke<PrivateConversation>("get_private_message_conversation", { userId: selectedId })
        : Promise.resolve(messagingConversation);
      const [users, thread, campaigns] = await Promise.all([
        usersPromise,
        threadPromise,
        campaignsPromise,
      ]);
      const conversations = await invoke<PrivateConversationSummary[]>(
        "list_private_message_conversations",
      );
      messagingUsers = sortPrivateMessageUsers(users);
      messagingConversations = sortPrivateConversations(conversations);
      messagingConversation = thread;
      messagingCampaigns = campaigns;
      retainCurrentMessagingImages();
      messagingLoaded = true;
      messagingError = "";

      if (messagingSelectedUserId && !users.some((user) => user.id === messagingSelectedUserId)
        && !conversations.some((conversation) => conversation.user.id === messagingSelectedUserId)) {
        messagingSelectedUserId = null;
        messagingConversation = null;
        messagingMobileDetailOpen = false;
      }
      if (!messagingSelectedUserId && conversations.length) {
        messagingSelectedUserId = conversations[0].user.id;
        if (messagingVisible && window.innerWidth > 860) {
          messagingConversation = await invoke<PrivateConversation>(
            "get_private_message_conversation",
            { userId: messagingSelectedUserId },
          );
          retainCurrentMessagingImages();
          messagingConversations = messagingConversations.map((conversation) =>
            conversation.user.id === messagingSelectedUserId
              ? { ...conversation, unreadCount: 0 }
              : conversation,
          );
        }
      }
      success = true;
    } catch (error) {
      messagingError = errorMessage(error);
      messagingLoaded = true;
    } finally {
      messagingLoading = false;
      messagingRefreshPromise = null;
      if (initialLoading || messagingSnapshot() !== before) {
        rerender();
        scrollMessagingFeedToBottom();
      }
    }
    return success;
  })();
  messagingRefreshPromise = refresh;
  return refresh;
};

export const messagingUnreadCount = () =>
  privateMessagingUnreadCount(messagingConversations);

export const setMessagingVisible = (visible: boolean) => {
  const changed = messagingVisible !== visible;
  messagingVisible = visible;
  if (!visible) {
    messagingImageObserver?.disconnect();
    messagingImageObserver = null;
  }
  if (changed) {
    clearMessagingPollTimer();
    syncMessagingFallbackPolling();
  }
};

export const openMessagingComposer = (rerender: () => void) => {
  messagingBrowseUsers = true;
  messagingMobileDetailOpen = false;
  messagingSearch = "";
  rerender();
  focusMessagingSearch();
};

const sendPrivateMessage = async (
  options: MessagingUiOptions,
  form: HTMLFormElement,
) => {
  if (!messagingSelectedUserId) return;
  messagingDraft = form.querySelector<HTMLTextAreaElement>("#privateMessageBody")?.value
    ?? messagingDraft;
  if (!messagingDraft.trim() && !messagingImageAttachments.length) {
    messagingError = "Écrivez un message ou joignez une image.";
    options.rerender();
    focusMessagingComposer();
    return;
  }

  const userId = messagingSelectedUserId;
  const submittedImages = [...messagingImageAttachments];
  messagingSubmitting = true;
  messagingError = "";
  options.rerender();
  try {
    await invoke<PrivateMessage>("send_private_message", {
      userId,
      body: messagingDraft,
      images: chatImageAttachmentPayloads(submittedImages),
    });
    messagingDraft = "";
    disposeChatImagePreviews(submittedImages);
    messagingImageAttachments = [];
    const [thread, conversations] = await Promise.all([
      invoke<PrivateConversation>("get_private_message_conversation", { userId }),
      invoke<PrivateConversationSummary[]>("list_private_message_conversations"),
    ]);
    messagingConversation = thread;
    retainCurrentMessagingImages();
    messagingConversations = sortPrivateConversations(conversations).map((conversation) =>
      conversation.user.id === userId ? { ...conversation, unreadCount: 0 } : conversation,
    );
    options.setStatus?.(`Message privé envoyé à ${thread.user.username}`);
  } catch (error) {
    messagingError = errorMessage(error);
  } finally {
    messagingSubmitting = false;
    options.rerender();
    scrollMessagingFeedToBottom();
    focusMessagingComposer();
  }
};

const addMessagingImages = async (
  files: readonly File[],
  options: MessagingUiOptions,
) => {
  if (!files.length || messagingSubmitting) return;
  if (messagingImageAttachments.length + files.length > MAX_CHAT_IMAGE_ATTACHMENTS) {
    messagingError = `Vous pouvez joindre jusqu’à ${MAX_CHAT_IMAGE_ATTACHMENTS} images par message.`;
    options.rerender();
    focusMessagingComposer();
    return;
  }
  try {
    const additions = await readChatImageAttachments(files, messagingImageAttachments);
    messagingImageAttachments = [...messagingImageAttachments, ...additions];
    messagingError = "";
  } catch (error) {
    messagingError = errorMessage(error);
  }
  options.rerender();
  focusMessagingComposer();
};

const rememberMessagingCampaignForm = () => {
  messagingCampaignName =
    document.querySelector<HTMLInputElement>("#campaignName")?.value ?? messagingCampaignName;
  messagingCampaignVariants =
    document.querySelector<HTMLTextAreaElement>("#campaignVariants")?.value
    ?? messagingCampaignVariants;
  messagingCampaignIntervalSeconds = Number(
    document.querySelector<HTMLInputElement>("#campaignInterval")?.value
      ?? messagingCampaignIntervalSeconds,
  );
  messagingCampaignConsent =
    document.querySelector<HTMLInputElement>("#campaignConsent")?.checked
    ?? messagingCampaignConsent;
  messagingCampaignRecipientIds = new Set(
    Array.from(document.querySelectorAll<HTMLInputElement>("[data-campaign-recipient]:checked"))
      .map((input) => input.dataset.campaignRecipient ?? "")
      .filter(Boolean),
  );
};

const refreshMessagingCampaigns = async (options: MessagingUiOptions) => {
  try {
    messagingCampaigns = await invoke<PrivateMessageCampaign[]>(
      "list_private_message_campaigns",
    );
    messagingCampaignError = "";
  } catch (error) {
    messagingCampaignError = errorMessage(error);
  }
  options.rerender();
};

const submitMessagingCampaign = async (
  options: MessagingUiOptions,
  startImmediately: boolean,
) => {
  rememberMessagingCampaignForm();
  const variants = campaignVariants();
  if (!messagingCampaignRecipientIds.size) {
    messagingCampaignError = "Sélectionnez au moins un destinataire.";
  } else if (messagingCampaignRecipientIds.size > PRIVATE_MESSAGE_CAMPAIGN_MAX_RECIPIENTS) {
    messagingCampaignError =
      `Une campagne accepte au maximum ${PRIVATE_MESSAGE_CAMPAIGN_MAX_RECIPIENTS} destinataires.`;
  } else if (!variants.length) {
    messagingCampaignError = "Ajoutez au moins une variante de message.";
  } else if (variants.some((variant) => [...variant].length > PRIVATE_MESSAGE_MAX_CHARS)) {
    messagingCampaignError =
      `Chaque variante doit contenir au maximum ${PRIVATE_MESSAGE_MAX_CHARS} caractères.`;
  } else if (!Number.isInteger(messagingCampaignIntervalSeconds)
    || messagingCampaignIntervalSeconds < PRIVATE_MESSAGE_CAMPAIGN_MIN_INTERVAL_SECONDS
    || messagingCampaignIntervalSeconds > 3_600) {
    messagingCampaignError = "La cadence doit être comprise entre 5 et 3 600 secondes.";
  } else if (startImmediately && !messagingCampaignConsent) {
    messagingCampaignError = "Confirmez l’autorisation de l’audience avant le lancement.";
  } else {
    messagingCampaignError = "";
  }
  if (messagingCampaignError) {
    options.rerender();
    return;
  }

  messagingCampaignSubmitting = true;
  options.rerender();
  const request: CreatePrivateMessageCampaignRequest = {
    name: messagingCampaignName,
    recipientIds: [...messagingCampaignRecipientIds],
    messageVariants: variants,
    intervalSeconds: messagingCampaignIntervalSeconds,
    startImmediately,
    consentConfirmed: messagingCampaignConsent,
  };
  try {
    const campaign = await invoke<PrivateMessageCampaign>(
      "create_private_message_campaign",
      { request },
    );
    messagingCampaigns = [campaign, ...messagingCampaigns];
    messagingCampaignName = "";
    messagingCampaignPreview = false;
    messagingCampaignRecipientIds = new Set();
    options.setStatus?.(
      startImmediately
        ? `Campagne « ${campaign.name} » lancée`
        : `Brouillon « ${campaign.name} » enregistré`,
    );
  } catch (error) {
    messagingCampaignError = errorMessage(error);
  } finally {
    messagingCampaignSubmitting = false;
    options.rerender();
  }
};

const controlMessagingCampaign = async (
  options: MessagingUiOptions,
  campaignId: string,
  action: "start" | "pause" | "resume" | "cancel",
) => {
  messagingCampaignError = "";
  const current = messagingCampaigns.find((campaign) => campaign.id === campaignId);
  const consentConfirmed = action === "start" && !current?.consentConfirmed
    ? window.confirm(
      "Confirmez-vous que tous les destinataires de ce brouillon ont accepté ces messages ?",
    )
    : false;
  if (action === "start" && !current?.consentConfirmed && !consentConfirmed) return;
  try {
    const campaign = await invoke<PrivateMessageCampaign>(
      "control_private_message_campaign",
      { campaignId, action, consentConfirmed },
    );
    messagingCampaigns = messagingCampaigns.map((entry) =>
      entry.id === campaign.id ? campaign : entry);
    options.setStatus?.(`Campagne « ${campaign.name} » mise à jour`);
  } catch (error) {
    messagingCampaignError = errorMessage(error);
  }
  options.rerender();
};

export const bindMessagingUi = (options: MessagingUiOptions) => {
  document.querySelector<HTMLButtonElement>("#messagingNew")?.addEventListener("click", () =>
    openMessagingComposer(options.rerender));
  document.querySelector<HTMLButtonElement>("#messagingCampaigns")?.addEventListener("click", () => {
    messagingCampaignsOpen = true;
    messagingCampaignError = "";
    options.rerender();
    void refreshMessagingCampaigns(options);
  });
  const closeMessagingCampaigns = (event?: Event) => {
    if (event?.target instanceof HTMLElement
      && event.target.closest(".messaging-campaign-dialog")
      && !event.target.matches("[data-campaign-close]")) return;
    rememberMessagingCampaignForm();
    messagingCampaignsOpen = false;
    messagingCampaignPreview = false;
    options.rerender();
  };
  document.querySelectorAll<HTMLElement>("[data-campaign-close]").forEach((element) => {
    element.addEventListener("click", closeMessagingCampaigns);
  });
  document.querySelector<HTMLButtonElement>("[data-campaign-refresh]")?.addEventListener("click", () => {
    rememberMessagingCampaignForm();
    void refreshMessagingCampaigns(options);
  });
  document.querySelector<HTMLButtonElement>("[data-campaign-select-all]")?.addEventListener("click", () => {
    rememberMessagingCampaignForm();
    const selectable = messagingUsers.slice(0, PRIVATE_MESSAGE_CAMPAIGN_MAX_RECIPIENTS);
    messagingCampaignRecipientIds = messagingCampaignRecipientIds.size === selectable.length
      ? new Set()
      : new Set(selectable.map((user) => user.id));
    options.rerender();
  });
  document.querySelectorAll<HTMLInputElement>("[data-campaign-recipient]").forEach((input) => {
    input.addEventListener("change", () => {
      rememberMessagingCampaignForm();
      if (messagingCampaignRecipientIds.size > PRIVATE_MESSAGE_CAMPAIGN_MAX_RECIPIENTS) {
        messagingCampaignRecipientIds.delete(input.dataset.campaignRecipient ?? "");
        messagingCampaignError =
          `Limite de ${PRIVATE_MESSAGE_CAMPAIGN_MAX_RECIPIENTS} destinataires atteinte.`;
      } else {
        messagingCampaignError = "";
      }
      options.rerender();
    });
  });
  document.querySelector<HTMLButtonElement>("[data-campaign-preview]")?.addEventListener("click", () => {
    rememberMessagingCampaignForm();
    messagingCampaignPreview = true;
    options.rerender();
  });
  document.querySelectorAll<HTMLButtonElement>("[data-campaign-action]").forEach((button) => {
    button.addEventListener("click", () => {
      const campaignId = button.dataset.campaignId;
      const action = button.dataset.campaignAction as "start" | "pause" | "resume" | "cancel";
      if (campaignId && action) void controlMessagingCampaign(options, campaignId, action);
    });
  });
  document.querySelector<HTMLFormElement>("#messagingCampaignForm")?.addEventListener("submit", (event) => {
    event.preventDefault();
    const submitter = (event as SubmitEvent).submitter as HTMLButtonElement | null;
    void submitMessagingCampaign(options, submitter?.dataset.campaignSubmit === "start");
  });
  document.querySelectorAll<HTMLButtonElement>("[data-messaging-browse-users]").forEach((button) => {
    button.addEventListener("click", () => openMessagingComposer(options.rerender));
  });
  document.querySelector<HTMLButtonElement>("[data-messaging-show-conversations]")?.addEventListener("click", () => {
    messagingBrowseUsers = false;
    messagingSearch = "";
    options.rerender();
  });
  document.querySelector<HTMLButtonElement>("#messagingRefresh")?.addEventListener("click", () => {
    void refreshMessaging(options.rerender);
  });
  document.querySelector<HTMLInputElement>("#messagingSearch")?.addEventListener("input", (event) => {
    messagingSearch = (event.currentTarget as HTMLInputElement).value;
    options.rerender();
    focusMessagingSearch();
  });
  document.querySelectorAll<HTMLButtonElement>("[data-private-message-user-id]").forEach((button) => {
    button.addEventListener("click", () => {
      const userId = button.dataset.privateMessageUserId;
      if (userId) void loadMessagingThread(userId, options.rerender);
    });
  });
  document.querySelector<HTMLButtonElement>("[data-messaging-back]")?.addEventListener("click", () => {
    messagingMobileDetailOpen = false;
    options.rerender();
  });
  document.querySelector<HTMLButtonElement>("[data-messaging-dismiss-error]")?.addEventListener("click", () => {
    messagingError = "";
    options.rerender();
  });
  document.querySelectorAll<HTMLButtonElement>("[data-messaging-remove-image]").forEach((button) => {
    button.addEventListener("click", () => {
      const imageId = button.dataset.messagingRemoveImage;
      const image = messagingImageAttachments.find((entry) => entry.id === imageId);
      if (image) disposeChatImagePreviews([image]);
      messagingImageAttachments = messagingImageAttachments.filter((entry) => entry.id !== imageId);
      options.rerender();
      focusMessagingComposer();
    });
  });
  document.querySelectorAll<HTMLButtonElement>("[data-messaging-open-image]").forEach((button) => {
    button.addEventListener("click", () => {
      messagingImageViewerScrollTop = document.querySelector<HTMLElement>("#messagingThreadFeed")?.scrollTop ?? null;
      messagingOpenImageId = button.dataset.messagingOpenImage ?? null;
      options.rerender();
      restoreMessagingFeedScroll();
    });
  });
  document.querySelectorAll<HTMLButtonElement>("[data-messaging-retry-image]").forEach((button) => {
    button.addEventListener("click", () => {
      const imageId = button.dataset.messagingRetryImage;
      if (imageId) messagingImageFailures.delete(imageId);
      options.rerender();
    });
  });
  const imageViewer = document.querySelector<HTMLElement>("[data-messaging-image-viewer]");
  const closeImageViewer = () => {
    const imageId = messagingOpenImageId;
    messagingOpenImageId = null;
    options.rerender();
    restoreMessagingFeedScroll(true);
    window.requestAnimationFrame(() => {
      const imageButton = Array.from(
        document.querySelectorAll<HTMLButtonElement>("[data-messaging-open-image]"),
      ).find((button) => button.dataset.messagingOpenImage === imageId);
      imageButton?.focus();
    });
  };
  imageViewer?.addEventListener("click", (event) => {
    if (event.target === imageViewer) closeImageViewer();
  });
  imageViewer?.addEventListener("keydown", (event) => {
    if (event.key === "Escape") closeImageViewer();
    if (event.key === "Tab") {
      event.preventDefault();
      document.querySelector<HTMLButtonElement>("[data-messaging-close-image]")?.focus();
    }
  });
  document.querySelector<HTMLButtonElement>("[data-messaging-close-image]")?.addEventListener("click", closeImageViewer);
  if (imageViewer) {
    imageViewer.tabIndex = -1;
    imageViewer.focus();
  }

  const form = document.querySelector<HTMLFormElement>("#privateMessageForm");
  const textarea = form?.querySelector<HTMLTextAreaElement>("#privateMessageBody");
  const imageInput = form?.querySelector<HTMLInputElement>("#privateMessageImages");
  imageInput?.addEventListener("change", () => {
    const files = Array.from(imageInput.files ?? []);
    imageInput.value = "";
    void addMessagingImages(files, options);
  });
  textarea?.addEventListener("input", () => {
    messagingDraft = textarea.value;
    const count = form?.querySelector<HTMLElement>("[data-private-message-count]");
    if (count) count.textContent = String([...messagingDraft].length);
  });
  textarea?.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" || (!event.ctrlKey && !event.metaKey)) return;
    event.preventDefault();
    form?.requestSubmit();
  });
  textarea?.addEventListener("paste", (event) => {
    const files = clipboardChatImageFiles(event.clipboardData);
    if (!files.length) return;
    event.preventDefault();
    void addMessagingImages(files, options);
  });
  form?.addEventListener("dragover", (event) => {
    if (!event.dataTransfer?.types.includes("Files")) return;
    event.preventDefault();
  });
  form?.addEventListener("drop", (event) => {
    const files = Array.from(event.dataTransfer?.files ?? [])
      .filter((file) => file.type.toLowerCase().startsWith("image/"));
    if (!files.length) return;
    event.preventDefault();
    void addMessagingImages(files, options);
  });
  form?.addEventListener("submit", (event) => {
    event.preventDefault();
    void sendPrivateMessage(options, form);
  });
  observeVisibleMessagingImages(options);
};

const clearMessagingPollTimer = () => {
  if (messagingPollTimer === null) return;
  clearInterval(messagingPollTimer);
  messagingPollTimer = null;
};

const syncMessagingFallbackPolling = () => {
  if (messagingRealtimeAvailable || !messagingPollRerender) {
    clearMessagingPollTimer();
    return;
  }
  if (messagingPollTimer !== null) return;
  messagingPollTimer = window.setInterval(() => {
    if (document.visibilityState === "visible" && messagingPollRerender) {
      void refreshMessaging(messagingPollRerender, { silent: true });
    }
  }, messagingVisible ? MESSAGING_POLL_INTERVAL_MS : MESSAGING_BACKGROUND_POLL_INTERVAL_MS);
};

export const setMessagingRealtimeAvailable = (available: boolean) => {
  messagingRealtimeAvailable = available;
  syncMessagingFallbackPolling();
};

export const startMessagingPolling = (rerender: () => void) => {
  messagingPollRerender = rerender;
  syncMessagingFallbackPolling();
};

export const stopMessagingPolling = () => {
  clearMessagingPollTimer();
  messagingPollRerender = null;
};
