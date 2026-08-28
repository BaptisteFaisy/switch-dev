import { hasRemoteAuth, invoke, remoteBaseUrl, saveRemoteConfig } from "./platform";
import "./tracking-view.css";

type DuelloTrackingWallet = {
  publicId: string;
  referralCode: string;
  displayName: string;
  email: string;
  clickCount: number;
  creditedClickCount: number;
  availableMinor: number;
  currency: string;
};

type DuelloTrackingSnapshot = {
  configured: boolean;
  configurationMessage: string | null;
  dashboardUrl: string;
  wallets: DuelloTrackingWallet[];
};

type TrackingPanelBindings = {
  rerender: () => void;
  renderIcons: (root?: ParentNode) => void;
};

let snapshot: DuelloTrackingSnapshot | null = null;
let loading = false;
let error = "";
let toast = "";
let toastTimer: number | null = null;

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
    return "Le suivi Duello est momentanément indisponible.";
  }
  return redacted;
};

const initials = (name: string) =>
  name
    .trim()
    .split(/\s+/)
    .slice(0, 2)
    .map((part) => part[0]?.toLocaleUpperCase("fr-FR") ?? "")
    .join("");

const trackingUrl = (referralCode: string) =>
  `https://duello.fr/l/${encodeURIComponent(referralCode)}`;

const safeDashboardUrl = (value: string) => {
  try {
    const url = new URL(value);
    return url.protocol === "https:" && !url.username && !url.password ? url.href : "";
  } catch {
    return "";
  }
};

const formatMoney = (minor: number, currency: string) => {
  if (!Number.isSafeInteger(minor) || minor < 0 || !/^[A-Z]{3}$/.test(currency)) return "—";
  try {
    return new Intl.NumberFormat("fr-FR", {
      style: "currency",
      currency,
    }).format(minor / 100);
  } catch {
    return "—";
  }
};

const totalAvailableBalance = (wallets: DuelloTrackingWallet[]) => {
  if (wallets.some((wallet) => wallet.currency !== "EUR")) {
    return { value: "—", detail: "Devises multiples" };
  }
  let total = 0;
  for (const wallet of wallets) {
    if (!Number.isSafeInteger(wallet.availableMinor) || wallet.availableMinor < 0) {
      return { value: "—", detail: "Solde indisponible" };
    }
    total += wallet.availableMinor;
    if (!Number.isSafeInteger(total)) return { value: "—", detail: "Solde indisponible" };
  }
  return { value: formatMoney(total, "EUR"), detail: "Tous les comptes EUR" };
};

const showToast = (message: string, rerender: () => void) => {
  toast = message;
  if (toastTimer !== null) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toast = "";
    toastTimer = null;
    rerender();
  }, 2_200);
};

const adminUnlockForm = () => `<form class="tracking-admin-unlock" data-tracking-admin-unlock>
  <div><i data-lucide="lock-keyhole"></i><span><strong>Jeton administrateur requis</strong><small>Les comptes, clics et soldes Duello sont réservés à l’administration de Switch.</small></span></div>
  <label><span>Jeton admin Switch</span><input name="adminToken" type="password" autocomplete="current-password" required /></label>
  <button type="submit" class="tool-button primary"><i data-lucide="lock-open"></i><span>Déverrouiller</span></button>
</form>`;

export async function refreshTrackingPanel(rerender: () => void, silent = false) {
  if (loading || !hasRemoteAuth()) return;
  loading = true;
  if (!silent) error = "";
  if (!silent) rerender();
  try {
    snapshot = await invoke<DuelloTrackingSnapshot>("duello_bank_snapshot", {});
    error = "";
  } catch (cause) {
    error = readableError(cause);
  } finally {
    loading = false;
    rerender();
  }
}

export function renderTrackingPanel(): string {
  if (!hasRemoteAuth()) {
    return `<section class="tracking-dashboard tracking-locked" aria-labelledby="trackingTitle">
      <span class="tracking-state-icon"><i data-lucide="lock-keyhole"></i></span>
      <span class="tracking-eyebrow">Duello · VPS</span>
      <h1 id="trackingTitle">Liens de tracking</h1>
      <p>Déverrouillez la vue pour analyser tous les comptes Duello et leurs soldes.</p>
      ${adminUnlockForm()}
    </section>`;
  }

  if (!snapshot && loading) {
    return `<section class="tracking-dashboard tracking-loading" aria-busy="true"><span><i data-lucide="loader-circle"></i></span><strong>Analyse des comptes Duello</strong><small>Lecture sécurisée des liens, clics et soldes sur le VPS…</small></section>`;
  }

  if (!snapshot && error) {
    return `<section class="tracking-dashboard tracking-locked" role="alert"><span class="tracking-state-icon"><i data-lucide="circle-alert"></i></span><h1>Suivi Duello indisponible</h1><p>${escapeHtml(error)}</p><button type="button" class="tool-button primary" data-refresh-tracking><i data-lucide="refresh-cw"></i><span>Réessayer</span></button></section>`;
  }

  const current = snapshot;
  const wallets = current?.configured && Array.isArray(current.wallets) ? current.wallets : [];
  const dashboardUrl = safeDashboardUrl(current?.dashboardUrl ?? "");
  const totalClicks = wallets.reduce((total, wallet) => total + wallet.clickCount, 0);
  const totalCreditedClicks = wallets.reduce(
    (total, wallet) => total + wallet.creditedClickCount,
    0,
  );
  const totalBalance = totalAvailableBalance(wallets);

  const rows = wallets.length
    ? wallets.map((wallet) => `<tr>
        <td><span class="tracking-user"><span class="tracking-avatar">${escapeHtml(initials(wallet.displayName))}</span><span><strong>${escapeHtml(wallet.displayName)}</strong><small>${escapeHtml(wallet.email)}</small></span></span></td>
        <td><a class="tracking-personal-link" href="${escapeHtml(trackingUrl(wallet.referralCode))}" target="_blank" rel="noopener noreferrer"><code>duello.fr/l/${escapeHtml(wallet.referralCode)}</code><i data-lucide="external-link"></i></a></td>
        <td><strong>${wallet.clickCount.toLocaleString("fr-FR")}</strong><small>${wallet.creditedClickCount.toLocaleString("fr-FR")} crédités</small></td>
        <td><span class="tracking-balance">${escapeHtml(formatMoney(wallet.availableMinor, wallet.currency))}</span></td>
        <td><span class="tracking-row-actions"><button type="button" data-copy-tracking="${escapeHtml(wallet.referralCode)}" title="Copier le lien"><i data-lucide="copy"></i></button></span></td>
      </tr>`).join("")
    : `<tr><td colspan="5"><div class="tracking-empty"><span><i data-lucide="users"></i></span><strong>Aucun compte Duello</strong><small>Le backend du VPS n'a renvoyé aucun portefeuille affilié.</small></div></td></tr>`;

  if (current && !current.configured) {
    return `<section class="tracking-dashboard tracking-locked" role="status"><span class="tracking-state-icon"><i data-lucide="unplug"></i></span><span class="tracking-eyebrow">Duello · VPS</span><h1>Connexion Duello à terminer</h1><p>${escapeHtml(current.configurationMessage || "Le backend Duello n'est pas configuré dans Switch développement.")}</p><button type="button" class="tool-button primary" data-refresh-tracking><i data-lucide="refresh-cw"></i><span>Revérifier</span></button></section>`;
  }

  return `<section class="tracking-dashboard" aria-labelledby="trackingTitle">
    <header class="tracking-hero">
      <div><span class="tracking-eyebrow">Duello · Backend VPS</span><h1 id="trackingTitle">Liens de tracking</h1><p>Tous les comptes Duello sont analysés depuis la source de vérité du VPS, avec leur lien personnel, leurs clics et leur solde disponible.</p></div>
      <div class="tracking-hero-actions"><button type="button" class="tool-button primary" data-refresh-tracking><i data-lucide="refresh-cw" class="${loading ? "is-spinning" : ""}"></i><span>Actualiser</span></button></div>
    </header>

    ${error ? `<div class="tracking-error" role="alert"><i data-lucide="circle-alert"></i><span>${escapeHtml(error)}</span><button type="button" data-refresh-tracking>Réessayer</button></div>` : ""}

    <div class="tracking-metrics" aria-label="Indicateurs du tracking Duello">
      <article><span><i data-lucide="mouse-pointer-click"></i></span><div><small>Clics au total</small><strong>${totalClicks.toLocaleString("fr-FR")}</strong><em>${totalCreditedClicks.toLocaleString("fr-FR")} clics crédités</em></div></article>
      <article class="is-accent"><span><i data-lucide="wallet-cards"></i></span><div><small>Solde disponible</small><strong>${escapeHtml(totalBalance.value)}</strong><em>${escapeHtml(totalBalance.detail)}</em></div></article>
      <article><span><i data-lucide="users"></i></span><div><small>Comptes analysés</small><strong>${wallets.length.toLocaleString("fr-FR")}</strong><em>Backend Duello du VPS</em></div></article>
    </div>

    <div class="tracking-content-grid">
      <article class="tracking-table-card">
        <header><div><span>Source Duello</span><strong>Comptes, tracking et soldes</strong></div><button type="button" data-refresh-tracking title="Actualiser"><i data-lucide="refresh-cw" class="${loading ? "is-spinning" : ""}"></i></button></header>
        <div class="tracking-table-wrap"><table><thead><tr><th>Compte</th><th>Lien personnel</th><th>Clics</th><th>Solde disponible</th><th><span class="sr-only">Actions</span></th></tr></thead><tbody>${rows}</tbody></table></div>
      </article>

      <aside class="tracking-rule-card">
        <span class="tracking-eyebrow">Connexion active</span>
        <h2>Backend Duello VPS</h2>
        <p>Les valeurs affichées ne viennent plus du fichier local de Switch. Chaque actualisation relit directement les portefeuilles affiliés Duello.</p>
        <div class="tracking-source-summary"><span>Comptes analysés</span><strong>${wallets.length.toLocaleString("fr-FR")}</strong></div>
        <div class="tracking-source-summary"><span>Clics remontés</span><strong>${totalClicks.toLocaleString("fr-FR")}</strong></div>
        ${dashboardUrl ? `<a href="${escapeHtml(dashboardUrl)}" target="_blank" rel="noopener noreferrer"><span>Ouvrir Duello</span><i data-lucide="arrow-up-right"></i></a>` : ""}
      </aside>
    </div>

    ${toast ? `<div class="tracking-toast" role="status"><i data-lucide="check"></i><span>${escapeHtml(toast)}</span></div>` : ""}
  </section>`;
}

export function bindTrackingPanel({ rerender, renderIcons }: TrackingPanelBindings) {
  const root = document.querySelector<HTMLElement>(".tracking-dashboard");
  if (!root) return;

  if (!snapshot && !loading && !error && hasRemoteAuth()) void refreshTrackingPanel(rerender);

  root.querySelector<HTMLFormElement>("[data-tracking-admin-unlock]")?.addEventListener("submit", (event) => {
    event.preventDefault();
    const input = root.querySelector<HTMLInputElement>("[data-tracking-admin-unlock] input[name='adminToken']");
    const token = input?.value.trim() ?? "";
    if (!token) {
      input?.setCustomValidity("Jeton administrateur requis");
      input?.reportValidity();
      return;
    }
    saveRemoteConfig(remoteBaseUrl(), token, undefined);
    error = "";
    void refreshTrackingPanel(rerender);
  });

  root.querySelectorAll<HTMLButtonElement>("[data-refresh-tracking]").forEach((button) => {
    button.addEventListener("click", () => void refreshTrackingPanel(rerender));
  });
  root.querySelectorAll<HTMLButtonElement>("[data-copy-tracking]").forEach((button) => {
    button.addEventListener("click", () => {
      const referralCode = button.dataset.copyTracking;
      if (!referralCode) return;
      void navigator.clipboard.writeText(trackingUrl(referralCode)).then(() => {
        showToast("Lien Duello copié", rerender);
        rerender();
      });
    });
  });

  renderIcons(root);
}
