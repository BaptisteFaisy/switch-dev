import { hasRemoteAuth, invoke, remoteBaseUrl, saveRemoteConfig } from "./platform";
import {
  connectPhantom,
  disconnectPhantom,
  formatSolanaAddress,
  formatUsdcMinor,
  getUsdcBalance,
  phantomErrorMessage,
  sendUsdc,
  waitForTransactionConfirmation,
  SOLSCAN_ADDRESS_PREFIX,
  type SendUsdcResult,
} from "./phantom-pay";
import "./duello-bank.css";

type DuelloBankWallet = {
  publicId: string;
  referralCode: string;
  displayName: string;
  email: string;
  clickCount: number;
  creditedClickCount: number;
  availableMinor: number;
  currency: string;
  stripeStatus: string | null;
  stripePayoutsEnabled: boolean | null;
  canWithdraw: boolean | null;
  solanaAddress: string | null;
};

type DuelloBankSnapshot = {
  configured: boolean;
  configurationMessage: string | null;
  dashboardUrl: string;
  stripeDashboardUrl: string;
  wallets: DuelloBankWallet[];
};

type CreditDuelloWalletResult = {
  success: boolean;
  idempotent: boolean;
};

type DuelloBankPanelBindings = {
  rerender: () => void;
  renderIcons: (root?: ParentNode) => void;
};

const MAX_CREDIT_MINOR = 10_000_000;

let snapshot: DuelloBankSnapshot | null = null;
let loading = false;
let loadError = "";
let modalOpen = false;
let selectedWalletId = "";
let draftAmount = "";
let draftReason = "";
let draftConfirmed = false;
let modalError = "";
let saving = false;
let pendingReference = "";
let pendingFingerprint = "";
let toast = "";
let toastTimer: number | null = null;

let phantomConnected = false;
let phantomAddress = "";
let phantomBalanceMinor: bigint | null = null;
let payModalOpen = false;
let payWalletId = "";
let payAmountDraft = "";
let payConfirmed = false;
let payBusy = false;
let payError = "";
let payResult: SendUsdcResult | null = null;
let payConfirmation: "none" | "broadcast" | "confirmed" | "failed" | "unknown" = "none";

const escapeHtml = (value: unknown) =>
  String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");

const readableError = (cause: unknown) => {
  const raw = cause instanceof Error ? cause.message : String(cause ?? "");
  const redacted = raw
    .replace(/Bearer\s+[^\s,;]+/gi, "Bearer [masqué]")
    .replace(/((?:token|secret|authorization)\s*[:=]\s*)[^\s,;]+/gi, "$1[masqué]")
    .trim();
  if (!redacted || redacted.length > 320) {
    return "La Banque Duello est momentanément indisponible.";
  }
  return redacted;
};

const isAdminTokenError = (cause: unknown) =>
  /token admin|authentification|unauthorized|401/i.test(String(cause instanceof Error ? cause.message : cause));

const safeHttpsUrl = (value: string, expectedHost?: string) => {
  try {
    const url = new URL(value);
    if (
      url.protocol !== "https:"
      || url.username
      || url.password
      || (expectedHost && url.hostname.toLocaleLowerCase("en-US") !== expectedHost)
    ) {
      return "";
    }
    return url.href;
  } catch {
    return "";
  }
};

const formatEuroMinor = (minor: number) => {
  if (!Number.isSafeInteger(minor) || minor < 0) return "—";
  const euros = Math.floor(minor / 100).toLocaleString("fr-FR");
  const cents = String(minor % 100).padStart(2, "0");
  return `${euros},${cents}\u00a0€`;
};

const formatCount = (value: number) =>
  Number.isSafeInteger(value) && value >= 0 ? value.toLocaleString("fr-FR") : "—";

const parseEuroAmountToMinor = (raw: string) => {
  const compact = raw.trim().replace(/[\s\u00a0\u202f]/g, "");
  if (!/^\d{1,9}(?:[,.]\d{1,2})?$/.test(compact)) return null;
  const [euros, cents = ""] = compact.split(/[,.]/);
  const minor = Number(euros) * 100 + Number(cents.padEnd(2, "0"));
  if (!Number.isSafeInteger(minor) || minor < 1 || minor > MAX_CREDIT_MINOR) return null;
  return minor;
};

const normalizeReason = (value: string) => value.trim().replace(/\s+/g, " ");

const initials = (name: string) =>
  name
    .trim()
    .split(/\s+/)
    .slice(0, 2)
    .map((part) => part[0]?.toLocaleUpperCase("fr-FR") ?? "")
    .join("");

const createReference = () => {
  const entropy = globalThis.crypto?.randomUUID?.()
    ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  return `switch-duello-${Date.now().toString(36)}-${entropy.replaceAll("-", "").slice(0, 32)}`;
};

const showToast = (message: string, rerender: () => void) => {
  toast = message;
  if (toastTimer !== null) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toast = "";
    toastTimer = null;
    rerender();
  }, 3_200);
};

const resetCreditDraft = () => {
  selectedWalletId = "";
  draftAmount = "";
  draftReason = "";
  draftConfirmed = false;
  modalError = "";
  pendingReference = "";
  pendingFingerprint = "";
};

const resetPayDraft = () => {
  payWalletId = "";
  payAmountDraft = "";
  payConfirmed = false;
  payError = "";
  payResult = null;
  payConfirmation = "none";
};

const parseUsdcAmountToMinor = (raw: string) => {
  const compact = raw.trim().replace(/[\s\u00a0\u202f]/g, "").replace(",", ".");
  if (!/^\d{1,9}(?:\.\d{1,6})?$/.test(compact)) return null;
  const [units, fraction = ""] = compact.split(".");
  const minor = BigInt(units) * 1_000_000n + BigInt(fraction.padEnd(6, "0"));
  if (minor <= 0n || minor > 1_000_000_000_000n) return null;
  return minor;
};

const updatePhantomBalance = async () => {
  if (!phantomAddress) {
    phantomBalanceMinor = null;
    return;
  }
  try {
    phantomBalanceMinor = (await getUsdcBalance(phantomAddress)).minor;
  } catch {
    phantomBalanceMinor = null;
  }
};

const phantomConnectWidget = () => {
  if (!phantomConnected || !phantomAddress) {
    return `<button type="button" class="duello-bank-button secondary" data-connect-phantom><i data-lucide="wallet"></i><span>Connecter Phantom</span></button>`;
  }
  return `<span class="duello-bank-phantom-chip" title="${escapeHtml(phantomAddress)}"><i data-lucide="wallet"></i><span>Phantom · ${escapeHtml(formatSolanaAddress(phantomAddress))}</span><small>${phantomBalanceMinor === null ? "solde inconnu" : escapeHtml(formatUsdcMinor(phantomBalanceMinor))}</small></span><button type="button" class="duello-bank-button secondary" data-connect-phantom><i data-lucide="plug-zap"></i><span>Déconnecter</span></button>`;
};

const phantomFlowStatus = () => {
  if (!phantomConnected || !phantomAddress) {
    return `<button type="button" class="duello-bank-flow-action" data-connect-phantom><span>Connecter mon wallet Phantom</span><i data-lucide="wallet"></i></button>`;
  }
  const balance = phantomBalanceMinor === null ? "solde USDC inconnu" : `Solde : ${formatUsdcMinor(phantomBalanceMinor)}`;
  return `<div class="duello-bank-flow-status"><span><i data-lucide="circle-check"></i></span><div><strong>Wallet connecté</strong><small>${escapeHtml(formatSolanaAddress(phantomAddress))} · ${escapeHtml(balance)}</small></div></div>`;
};

const walletStripeState = (wallet: DuelloBankWallet) => {
  if (wallet.stripePayoutsEnabled === true && wallet.canWithdraw !== false) {
    return { label: "Prêt au retrait", tone: "ready", detail: "Stripe Connect actif" };
  }
  if (wallet.canWithdraw === false) {
    return { label: "Retrait indisponible", tone: "blocked", detail: "Action requise sur Duello" };
  }
  if (wallet.stripeStatus) {
    return { label: "Configuration en cours", tone: "pending", detail: "Stripe Connect" };
  }
  return { label: "Non connecté", tone: "neutral", detail: "À activer sur Duello" };
};

const totalAvailableMinor = (wallets: DuelloBankWallet[]) => {
  let total = 0;
  for (const wallet of wallets) {
    if (!Number.isSafeInteger(wallet.availableMinor) || wallet.availableMinor < 0) return null;
    total += wallet.availableMinor;
    if (!Number.isSafeInteger(total)) return null;
  }
  return total;
};

const externalLink = (
  url: string,
  label: string,
  icon: string,
  className = "duello-bank-link",
) => url
  ? `<a class="${className}" href="${escapeHtml(url)}" target="_blank" rel="noopener noreferrer"><span>${escapeHtml(label)}</span><i data-lucide="${escapeHtml(icon)}"></i></a>`
  : `<span class="${className} is-disabled" aria-disabled="true"><span>${escapeHtml(label)}</span><i data-lucide="circle-slash-2"></i></span>`;

const adminUnlockForm = () => `<form class="duello-bank-admin-unlock" data-duello-bank-admin-unlock>
  <div><i data-lucide="lock-keyhole"></i><span><strong>Déverrouiller avec le jeton administrateur</strong><small>Cette vue exige le jeton administrateur du serveur Switch. Il reste dans la configuration locale.</small></span></div>
  <label><span>Jeton admin</span><input name="adminToken" type="password" autocomplete="current-password" required /></label>
  <button type="submit" class="duello-bank-button primary"><i data-lucide="lock-open"></i><span>Déverrouiller</span></button>
</form>`;

export async function refreshDuelloBankPanel(rerender: () => void, silent = false) {
  if (loading) return;
  if (!hasRemoteAuth()) {
    // Sans jeton administrateur, la route renverrait 401 en boucle. On affiche
    // directement le formulaire de déverrouillage, comme l'onglet VPS.
    loading = false;
    loadError = "";
    if (!silent) rerender();
    return;
  }
  loading = true;
  if (!silent) loadError = "";
  if (!silent) rerender();
  try {
    snapshot = await invoke<DuelloBankSnapshot>("duello_bank_snapshot", {});
    loadError = "";
  } catch (cause) {
    loadError = readableError(cause);
  } finally {
    loading = false;
    rerender();
  }
}

const renderLoading = () => `<section class="duello-bank-dashboard duello-bank-state" aria-busy="true" aria-live="polite">
  <span class="duello-bank-state-icon"><i data-lucide="loader-circle"></i></span>
  <strong>Chargement de la Banque Duello</strong>
  <small>Lecture sécurisée des soldes Duello…</small>
</section>`;

const renderLoadFailure = () => `<section class="duello-bank-dashboard duello-bank-state" role="alert">
  <span class="duello-bank-state-icon"><i data-lucide="circle-alert"></i></span>
  <strong>Banque Duello indisponible</strong>
  <small>${escapeHtml(loadError)}</small>
  ${isAdminTokenError(loadError) ? adminUnlockForm() : ""}
  <button type="button" class="duello-bank-button primary" data-refresh-duello-bank><i data-lucide="refresh-cw"></i><span>Réessayer</span></button>
</section>`;

const renderAdminUnlock = () => `<section class="duello-bank-dashboard duello-bank-state" role="status">
  <span class="duello-bank-state-icon"><i data-lucide="lock-keyhole"></i></span>
  <strong>Jeton administrateur requis</strong>
  <small>Déverrouille la Banque Duello avec le jeton administrateur du serveur Switch.</small>
  ${adminUnlockForm()}
</section>`;

const renderCreditModal = (wallets: DuelloBankWallet[]) => {
  if (!modalOpen) return "";
  const selectedWallet = wallets.find((wallet) => wallet.publicId === selectedWalletId) ?? null;
  const options = wallets.map((wallet) => `<option value="${escapeHtml(wallet.publicId)}" ${wallet.publicId === selectedWalletId ? "selected" : ""}>${escapeHtml(wallet.displayName)} · ${escapeHtml(formatEuroMinor(wallet.availableMinor))}</option>`).join("");

  return `<div class="duello-bank-modal-backdrop" data-duello-bank-modal-backdrop>
    <section class="duello-bank-modal" role="dialog" aria-modal="true" aria-labelledby="duelloBankModalTitle" aria-describedby="duelloBankModalConsequence">
      <button type="button" class="duello-bank-modal-close" data-close-duello-bank-modal aria-label="Fermer" ${saving ? "disabled" : ""}><i data-lucide="x"></i></button>
      <span class="duello-bank-eyebrow">Grand livre Duello</span>
      <h2 id="duelloBankModalTitle">Ajouter un crédit</h2>
      <p id="duelloBankModalConsequence">Cette opération augmente le solde affiché dans <strong>duello/dashboard</strong>. Elle n’ajoute pas d’argent au solde de la plateforme Stripe.</p>
      <form data-duello-bank-credit-form novalidate>
        <label>
          <span>Membre à créditer</span>
          <select name="publicId" required ${saving ? "disabled" : ""}>
            <option value="">Choisir un membre…</option>
            ${options}
          </select>
        </label>
        ${selectedWallet ? `<div class="duello-bank-current-balance"><span>Solde Duello actuel</span><strong>${escapeHtml(formatEuroMinor(selectedWallet.availableMinor))}</strong></div>` : ""}
        <label>
          <span>Montant en euros</span>
          <span class="duello-bank-amount-field"><input name="amount" type="text" inputmode="decimal" autocomplete="off" maxlength="12" pattern="[0-9]+([,.][0-9]{1,2})?" placeholder="Ex. 25,00" value="${escapeHtml(draftAmount)}" required autofocus ${saving ? "disabled" : ""} /><b aria-hidden="true">€</b></span>
          <small>De 0,01 € à 100 000,00 €, calculé exactement en centimes.</small>
        </label>
        <label>
          <span>Motif du crédit</span>
          <input name="reason" type="text" autocomplete="off" minlength="3" maxlength="240" placeholder="Ex. Régularisation du solde partenaire" value="${escapeHtml(draftReason)}" required ${saving ? "disabled" : ""} />
        </label>
        <label class="duello-bank-confirmation">
          <input name="confirmCredit" type="checkbox" value="yes" ${draftConfirmed ? "checked" : ""} ${saving ? "disabled" : ""} />
          <span><strong>Je confirme ce crédit du grand livre Duello.</strong><small>Le membre pourra voir ce solde sur Duello et le retirer via Stripe Connect s’il est éligible. Ceci ne constitue pas un approvisionnement Stripe.</small></span>
        </label>
        ${pendingReference ? `<div class="duello-bank-reference" role="status"><span>Référence conservée pour les nouvelles tentatives</span><code>${escapeHtml(pendingReference)}</code></div>` : ""}
        ${modalError ? `<div class="duello-bank-form-error" role="alert"><i data-lucide="circle-alert"></i><span>${escapeHtml(modalError)}</span></div>` : ""}
        <div class="duello-bank-modal-actions">
          <button type="button" class="duello-bank-button secondary" data-close-duello-bank-modal ${saving ? "disabled" : ""}>Annuler</button>
          <button type="submit" class="duello-bank-button primary" ${saving ? "disabled" : ""}><span>${saving ? "Enregistrement…" : "Confirmer le crédit"}</span><i data-lucide="${saving ? "loader-circle" : "arrow-right"}" class="${saving ? "is-spinning" : ""}"></i></button>
        </div>
      </form>
    </section>
  </div>`;
};

const renderPhantomPayModal = (wallets: DuelloBankWallet[]) => {
  if (!payModalOpen) return "";
  const wallet = wallets.find((candidate) => candidate.publicId === payWalletId) ?? null;
  const walletLabel = wallet ? `${wallet.displayName} · ${formatEuroMinor(wallet.availableMinor)}` : "";
  const balanceText = phantomBalanceMinor === null ? "inconnu" : formatUsdcMinor(phantomBalanceMinor);
  const resultLabel = payConfirmation === "confirmed"
    ? "Transaction confirmée"
    : payConfirmation === "failed"
      ? "Transaction échouée"
      : payConfirmation === "unknown"
        ? "Confirmation à vérifier"
        : "Transaction diffusée · confirmation en cours";
  const submitLabel = payBusy
    ? payResult ? "Confirmation Solana…" : "Signature dans Phantom…"
    : payConfirmation === "confirmed"
      ? "USDC confirmés"
      : payResult
        ? "Transaction diffusée"
        : "Envoyer les USDC";
  return `<div class="duello-bank-modal-backdrop" data-phantom-pay-backdrop>
    <section class="duello-bank-modal" role="dialog" aria-modal="true" aria-labelledby="phantomPayTitle" aria-describedby="phantomPayConsequence">
      <button type="button" class="duello-bank-modal-close" data-close-phantom-pay-modal aria-label="Fermer" ${payBusy ? "disabled" : ""}><i data-lucide="x"></i></button>
      <span class="duello-bank-eyebrow">Solana · USDC</span>
      <h2 id="phantomPayTitle">Payer le retrait via Phantom</h2>
      <p id="phantomPayConsequence">Envoie des USDC depuis votre wallet Phantom vers l’adresse Solana du membre. La transaction est signée dans l’extension Phantom : Switch ne voit jamais votre clé privée.</p>
      ${!phantomConnected || !phantomAddress ? `<div class="duello-bank-form-error" role="alert"><i data-lucide="wallet"></i><span>${escapeHtml(phantomErrorMessage())}</span></div>` : ""}
      <form data-phantom-pay-form novalidate>
        <label>
          <span>Membre</span>
          <input type="text" value="${escapeHtml(walletLabel)}" disabled />
        </label>
        ${wallet?.solanaAddress ? `<div class="duello-bank-address-box"><span>Adresse Solana du membre</span><code>${escapeHtml(wallet.solanaAddress)}</code></div>` : ""}
        <div class="duello-bank-current-balance"><span>Votre wallet Phantom (expéditeur)</span><strong>${phantomConnected && phantomAddress ? `${escapeHtml(formatSolanaAddress(phantomAddress))} · ${escapeHtml(balanceText)}` : "Non connecté"}</strong></div>
        <label>
          <span>Montant en USDC</span>
          <span class="duello-bank-amount-field"><input name="amountUsdc" type="text" inputmode="decimal" autocomplete="off" maxlength="12" pattern="[0-9]+([.,][0-9]{1,6})?" placeholder="Ex. 25,00" value="${escapeHtml(payAmountDraft)}" required autofocus ${payBusy ? "disabled" : ""} /><b aria-hidden="true">USDC</b></span>
          <small>De 0,000001 à 1 000 000 USDC, jusqu’à 6 décimales. Le membre reçoit ce montant net.</small>
        </label>
        <label class="duello-bank-confirmation">
          <input name="confirmPay" type="checkbox" value="yes" ${payConfirmed ? "checked" : ""} ${payBusy ? "disabled" : ""} />
          <span><strong>Je confirme l’envoi de ${payAmountDraft ? escapeHtml(payAmountDraft.trim()) : "ce montant"} USDC vers l’adresse Solana du membre.</strong><small>Cette transaction est irréversible et ne peut pas être annulée depuis Switch.</small></span>
        </label>
        ${payResult ? `<div class="duello-bank-reference" role="status"><span>${resultLabel}</span><a href="${escapeHtml(payResult.transactionUrl)}" target="_blank" rel="noopener noreferrer">Voir sur Solscan →</a><code>${escapeHtml(payResult.signature)}</code></div>` : ""}
        ${payError ? `<div class="duello-bank-form-error" role="alert"><i data-lucide="circle-alert"></i><span>${escapeHtml(payError)}</span></div>` : ""}
        <div class="duello-bank-modal-actions">
          <button type="button" class="duello-bank-button secondary" data-close-phantom-pay-modal ${payBusy ? "disabled" : ""}>Fermer</button>
          <button type="submit" class="duello-bank-button primary" ${payBusy || payResult ? "disabled" : ""}><span>${submitLabel}</span><i data-lucide="${payBusy ? "loader-circle" : "send"}" class="${payBusy ? "is-spinning" : ""}"></i></button>
        </div>
      </form>
    </section>
  </div>`;
};

export function renderDuelloBankPanel(): string {
  if (!hasRemoteAuth()) return renderAdminUnlock();
  if (!snapshot && !loadError) return renderLoading();
  if (!snapshot && loadError) return renderLoadFailure();

  const current = snapshot as DuelloBankSnapshot;
  const wallets = Array.isArray(current.wallets) ? current.wallets : [];
  const duelloDashboardUrl = safeHttpsUrl(current.dashboardUrl);
  const stripeDashboardUrl = safeHttpsUrl(current.stripeDashboardUrl, "dashboard.stripe.com");
  const totalMinor = totalAvailableMinor(wallets);
  const stripeReadyCount = wallets.filter((wallet) => (
    wallet.stripePayoutsEnabled === true && wallet.canWithdraw !== false
  )).length;
  const canCredit = current.configured && wallets.length > 0;

  const rows = wallets.length
    ? wallets.map((wallet) => {
      const stripe = walletStripeState(wallet);
      const canCreditWallet = current.configured && wallet.currency === "EUR";
      return `<tr>
        <td data-label="Membre"><span class="duello-bank-member"><span class="duello-bank-avatar">${escapeHtml(initials(wallet.displayName))}</span><span><strong>${escapeHtml(wallet.displayName)}</strong><small>${escapeHtml(wallet.email)}</small></span></span></td>
        <td data-label="Solde Duello"><strong class="duello-bank-money">${escapeHtml(formatEuroMinor(wallet.availableMinor))}</strong><small>Grand livre Duello</small></td>
        <td data-label="Activité"><strong>${escapeHtml(formatCount(wallet.clickCount))} clics</strong><small>${escapeHtml(formatCount(wallet.creditedClickCount))} déjà crédités</small></td>
        <td data-label="Retrait Stripe"><span class="duello-bank-status is-${stripe.tone}"><i data-lucide="${stripe.tone === "ready" ? "circle-check" : "circle-dot"}"></i><span><strong>${escapeHtml(stripe.label)}</strong><small>${escapeHtml(stripe.detail)}</small></span></span></td>
        <td data-label="Solana">${wallet.solanaAddress ? `<span class="duello-bank-solana"><a href="${SOLSCAN_ADDRESS_PREFIX}${escapeHtml(wallet.solanaAddress)}" target="_blank" rel="noopener noreferrer">${escapeHtml(formatSolanaAddress(wallet.solanaAddress))}</a></span>` : `<span class="duello-bank-muted">—</span>`}</td>
        <td data-label="Action"><span class="duello-bank-row-actions"><button type="button" class="duello-bank-row-button" data-open-duello-bank-credit data-wallet-id="${escapeHtml(wallet.publicId)}" ${canCreditWallet ? "" : "disabled"}><i data-lucide="plus"></i><span>Créditer</span></button><button type="button" class="duello-bank-row-button is-phantom" data-open-phantom-pay data-wallet-id="${escapeHtml(wallet.publicId)}" ${wallet.solanaAddress && phantomConnected ? "" : "disabled"} title="${wallet.solanaAddress ? "" : "Adresse Solana non renseignée sur Duello"}"><i data-lucide="send"></i><span>Payer USDC</span></button></span></td>
      </tr>`;
    }).join("")
    : `<tr><td colspan="6"><div class="duello-bank-empty"><span><i data-lucide="wallet-cards"></i></span><strong>Aucun portefeuille Duello</strong><small>Les membres disposant d’un portefeuille apparaîtront ici.</small></div></td></tr>`;

  return `<section class="duello-bank-dashboard" aria-labelledby="duelloBankTitle">
    <header class="duello-bank-hero">
      <div>
        <span class="duello-bank-eyebrow"><i data-lucide="landmark"></i> Duello · Finance</span>
        <h1 id="duelloBankTitle">Banque Duello</h1>
        <p>Créditez le grand livre des membres Duello, puis gérez séparément les fonds réels de la plateforme dans Stripe.</p>
      </div>
      <div class="duello-bank-hero-actions">
        ${phantomConnectWidget()}
        ${externalLink(duelloDashboardUrl, "Ouvrir Duello", "external-link", "duello-bank-button secondary")}
        <button type="button" class="duello-bank-button primary" data-open-duello-bank-credit ${canCredit ? "" : "disabled"}><i data-lucide="plus"></i><span>Ajouter un crédit</span></button>
      </div>
    </header>

    ${loadError ? `<div class="duello-bank-alert is-error" role="alert"><i data-lucide="circle-alert"></i><span>${escapeHtml(loadError)}</span><button type="button" data-refresh-duello-bank>Réessayer</button></div>` : ""}
    ${!current.configured ? `<div class="duello-bank-alert is-unconfigured" role="status"><i data-lucide="shield-alert"></i><span><strong>Passerelle Duello non configurée</strong><small>${escapeHtml(current.configurationMessage ?? "Configurez la connexion serveur à Duello pour activer les crédits.")}</small></span></div>` : ""}

    <div class="duello-bank-flows" aria-label="Deux opérations financières distinctes">
      <article class="duello-bank-flow is-ledger">
        <span class="duello-bank-flow-step">Opération 1 · Duello</span>
        <div class="duello-bank-flow-heading"><span><i data-lucide="book-open-check"></i></span><div><h2>Crédit du grand livre</h2><p>Ajoute un montant au solde visible dans <strong>duello/dashboard</strong>. Le membre peut ensuite demander son retrait via Stripe Connect.</p></div></div>
        <button type="button" class="duello-bank-flow-action" data-open-duello-bank-credit ${canCredit ? "" : "disabled"}><span>Créditer un membre</span><i data-lucide="arrow-right"></i></button>
      </article>
      <article class="duello-bank-flow is-stripe">
        <span class="duello-bank-flow-step">Opération 2 · Stripe</span>
        <div class="duello-bank-flow-heading"><span><i data-lucide="credit-card"></i></span><div><h2>Fonds réels de la plateforme</h2><p>L’approvisionnement et la vérification du solde Stripe se font dans le Dashboard Stripe. Un crédit Duello ne déplace aucun fonds bancaire.</p></div></div>
        ${externalLink(stripeDashboardUrl, "Gérer le solde Stripe", "external-link", "duello-bank-flow-action")}
      </article>
      <article class="duello-bank-flow is-phantom">
        <span class="duello-bank-flow-step">Opération 3 · Solana</span>
        <div class="duello-bank-flow-heading"><span><i data-lucide="wallet"></i></span><div><h2>Paiement de retrait USDC</h2><p>Envoyez des USDC depuis votre wallet Phantom vers l’adresse Solana du membre (fournie par Duello). La transaction est signée dans l’extension, jamais par Switch.</p></div></div>
        ${phantomFlowStatus()}
      </article>
    </div>

    <div class="duello-bank-metrics" aria-label="Indicateurs Banque Duello">
      <article><span><i data-lucide="circle-dollar-sign"></i></span><div><small>Solde Duello disponible</small><strong>${escapeHtml(totalMinor === null ? "—" : formatEuroMinor(totalMinor))}</strong><em>Grand livre, hors solde Stripe</em></div></article>
      <article><span><i data-lucide="users"></i></span><div><small>Portefeuilles membres</small><strong>${wallets.length.toLocaleString("fr-FR")}</strong><em>Comptes Duello visibles</em></div></article>
      <article><span><i data-lucide="badge-check"></i></span><div><small>Stripe Connect prêt</small><strong>${stripeReadyCount.toLocaleString("fr-FR")}</strong><em>Retrait potentiellement disponible</em></div></article>
    </div>

    <article class="duello-bank-table-card">
      <header><div><span>Grand livre</span><strong>Soldes des membres Duello</strong></div><button type="button" data-refresh-duello-bank title="Actualiser" aria-label="Actualiser la Banque Duello"><i data-lucide="refresh-cw" class="${loading ? "is-spinning" : ""}"></i></button></header>
      <div class="duello-bank-table-wrap"><table><thead><tr><th>Membre</th><th>Solde Duello</th><th>Activité</th><th>Retrait Stripe</th><th>Solana</th><th><span class="duello-bank-sr-only">Action</span></th></tr></thead><tbody>${rows}</tbody></table></div>
    </article>

    <p class="duello-bank-footnote"><i data-lucide="info"></i><span>Le crédit Duello et l’approvisionnement Stripe sont deux opérations indépendantes. Vérifiez le solde réel Stripe avant tout retrait partenaire.</span></p>

    ${renderCreditModal(wallets)}
    ${renderPhantomPayModal(wallets)}
    ${toast ? `<div class="duello-bank-toast" role="status" aria-live="polite"><i data-lucide="circle-check"></i><span>${escapeHtml(toast)}</span></div>` : ""}
  </section>`;
}

export function bindDuelloBankPanel({ rerender, renderIcons }: DuelloBankPanelBindings) {
  const root = document.querySelector<HTMLElement>(".duello-bank-dashboard");
  if (!root) return;

  // Sans jeton admin, le formulaire de déverrouillage est déjà rendu : ne pas
  // relancer de refresh (il re-rendrerait en boucle). Le déverrouillage, les
  // clics "Réessayer" et le bouton d'actualisation lancent le fetch.
  if (!snapshot && !loading && !loadError && hasRemoteAuth()) void refreshDuelloBankPanel(rerender);

  root.querySelectorAll<HTMLButtonElement>("[data-refresh-duello-bank]").forEach((button) => {
    button.addEventListener("click", () => void refreshDuelloBankPanel(rerender));
  });

  root.querySelector<HTMLFormElement>("[data-duello-bank-admin-unlock]")?.addEventListener("submit", (event) => {
    event.preventDefault();
    const input = root.querySelector<HTMLInputElement>("[data-duello-bank-admin-unlock] input[name='adminToken']");
    const token = input?.value.trim() ?? "";
    if (!token) {
      input?.setCustomValidity("Jeton administrateur requis");
      input?.reportValidity();
      return;
    }
    saveRemoteConfig(remoteBaseUrl(), token, undefined);
    loadError = "";
    void refreshDuelloBankPanel(rerender);
  });

  root.querySelectorAll<HTMLButtonElement>("[data-open-duello-bank-credit]").forEach((button) => {
    button.addEventListener("click", () => {
      if (!snapshot?.configured || !snapshot.wallets.length) return;
      resetCreditDraft();
      selectedWalletId = button.dataset.walletId
        ?? (snapshot.wallets.length === 1 ? snapshot.wallets[0]?.publicId ?? "" : "");
      modalOpen = true;
      rerender();
    });
  });

  const closeModal = () => {
    if (saving) return;
    modalOpen = false;
    resetCreditDraft();
    rerender();
  };

  root.querySelectorAll<HTMLButtonElement>("[data-close-duello-bank-modal]").forEach((button) => {
    button.addEventListener("click", closeModal);
  });
  root.querySelector<HTMLElement>("[data-duello-bank-modal-backdrop]")?.addEventListener("pointerdown", (event) => {
    if (event.target === event.currentTarget) closeModal();
  });

  const closePayModal = () => {
    if (payBusy) return;
    payModalOpen = false;
    resetPayDraft();
    rerender();
  };

  root.querySelectorAll<HTMLButtonElement>("[data-close-phantom-pay-modal]").forEach((button) => {
    button.addEventListener("click", closePayModal);
  });
  root.querySelector<HTMLElement>("[data-phantom-pay-backdrop]")?.addEventListener("pointerdown", (event) => {
    if (event.target === event.currentTarget) closePayModal();
  });

  root.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    if (modalOpen) closeModal();
    else if (payModalOpen) closePayModal();
  });

  root.querySelector<HTMLFormElement>("[data-duello-bank-credit-form]")?.addEventListener("submit", (event) => {
    event.preventDefault();
    if (saving || !snapshot?.configured) return;

    const form = new FormData(event.currentTarget as HTMLFormElement);
    selectedWalletId = String(form.get("publicId") ?? "");
    draftAmount = String(form.get("amount") ?? "");
    draftReason = String(form.get("reason") ?? "");
    draftConfirmed = form.get("confirmCredit") === "yes";
    modalError = "";

    const wallet = snapshot.wallets.find((candidate) => candidate.publicId === selectedWalletId);
    const amountMinor = parseEuroAmountToMinor(draftAmount);
    const reason = normalizeReason(draftReason);
    if (!wallet) {
      modalError = "Choisissez le membre Duello à créditer.";
    } else if (wallet.currency !== "EUR") {
      modalError = "Ce portefeuille n’est pas libellé en euros.";
    } else if (amountMinor === null) {
      modalError = "Saisissez un montant compris entre 0,01 € et 100 000,00 €, avec deux décimales maximum.";
    } else if (Array.from(reason).length < 3 || Array.from(reason).length > 240 || /[\u0000-\u001f\u007f]/.test(reason)) {
      modalError = "Le motif doit contenir entre 3 et 240 caractères sur une seule ligne.";
    } else if (!draftConfirmed) {
      modalError = "Confirmez explicitement les conséquences de ce crédit.";
    }

    if (modalError || !wallet || amountMinor === null) {
      rerender();
      return;
    }

    const fingerprint = `${wallet.publicId}\u0000${amountMinor}\u0000${reason}`;
    if (!pendingReference || pendingFingerprint !== fingerprint) {
      pendingReference = createReference();
      pendingFingerprint = fingerprint;
    }

    saving = true;
    rerender();
    void invoke<CreditDuelloWalletResult>("credit_duello_wallet", {
      request: {
        publicId: wallet.publicId,
        amountMinor,
        reason,
        reference: pendingReference,
      },
    }).then((result) => {
      if (!result?.success) throw new Error("Duello n’a pas confirmé le crédit.");
      saving = false;
      modalOpen = false;
      const message = result.idempotent
        ? "Ce crédit avait déjà été enregistré : aucun doublon créé."
        : `Crédit de ${formatEuroMinor(amountMinor)} enregistré pour ${wallet.displayName}.`;
      resetCreditDraft();
      showToast(message, rerender);
      rerender();
      void refreshDuelloBankPanel(rerender, true);
    }).catch((cause) => {
      saving = false;
      modalError = readableError(cause);
      rerender();
    });
  });

  root.querySelectorAll<HTMLButtonElement>("[data-connect-phantom]").forEach((button) => {
    button.addEventListener("click", () => {
      if (payBusy) return;
      if (phantomConnected) {
        void disconnectPhantom().catch(() => undefined);
        phantomConnected = false;
        phantomAddress = "";
        phantomBalanceMinor = null;
        if (payModalOpen) payError = "Votre wallet Phantom a été déconnecté.";
        rerender();
        return;
      }
      void connectPhantom()
        .then((connection) => {
          phantomConnected = true;
          phantomAddress = connection.address;
          return updatePhantomBalance();
        })
        .then(() => rerender())
        .catch((cause) => {
          showToast(readableError(cause), rerender);
          rerender();
        });
    });
  });

  root.querySelectorAll<HTMLButtonElement>("[data-open-phantom-pay]").forEach((button) => {
    button.addEventListener("click", () => {
      if (!snapshot) return;
      resetPayDraft();
      payWalletId = button.dataset.walletId ?? "";
      payModalOpen = true;
      rerender();
    });
  });

  root.querySelector<HTMLFormElement>("[data-phantom-pay-form]")?.addEventListener("submit", (event) => {
    event.preventDefault();
    if (payBusy || payResult || !snapshot) return;

    const form = new FormData(event.currentTarget as HTMLFormElement);
    payAmountDraft = String(form.get("amountUsdc") ?? "");
    payConfirmed = form.get("confirmPay") === "yes";
    payError = "";

    const wallet = snapshot.wallets.find((candidate) => candidate.publicId === payWalletId) ?? null;
    const amountMinor = parseUsdcAmountToMinor(payAmountDraft);
    if (!phantomConnected || !phantomAddress) {
      payError = "Connectez d’abord votre wallet Phantom.";
    } else if (!wallet?.solanaAddress) {
      payError = "Ce membre n’a pas d’adresse Solana renseignée sur Duello.";
    } else if (amountMinor === null) {
      payError = "Saisissez un montant USDC valide (0,000001 à 1 000 000).";
    } else if (phantomBalanceMinor !== null && amountMinor > phantomBalanceMinor) {
      payError = `Solde USDC insuffisant (${formatUsdcMinor(phantomBalanceMinor)} disponibles).`;
    } else if (!payConfirmed) {
      payError = "Confirmez explicitement cet envoi USDC.";
    }

    if (payError || !wallet?.solanaAddress || amountMinor === null) {
      rerender();
      return;
    }

    payBusy = true;
    rerender();
    void sendUsdc({
      fromAddress: phantomAddress,
      toAddress: wallet.solanaAddress,
      amountMinor,
    })
      .then(async (result) => {
        payResult = result;
        payConfirmation = "broadcast";
        rerender();
        try {
          await waitForTransactionConfirmation(result);
          payConfirmation = "confirmed";
        } catch (cause) {
          const message = readableError(cause);
          payConfirmation = /échoué sur la chaine|echoue sur la chaine/i.test(message)
            ? "failed"
            : "unknown";
          payError = `${message} Vérifiez la signature sur Solscan avant toute nouvelle tentative.`;
        }
        await updatePhantomBalance();
        payBusy = false;
        rerender();
      })
      .catch((cause) => {
        payBusy = false;
        payError = readableError(cause);
        rerender();
      });
  });

  renderIcons(root);
}
