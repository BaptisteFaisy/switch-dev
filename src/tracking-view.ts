import { invoke } from "./platform";
import "./tracking-view.css";

export type TrackingLink = {
  id: string;
  userName: string;
  slug: string;
  destinationUrl: string;
  clickCount: number;
  createdAt: number;
};

type TrackingSnapshot = {
  clicksPerDay: number;
  links: TrackingLink[];
};

type TrackingPanelBindings = {
  rerender: () => void;
  renderIcons: (root?: ParentNode) => void;
};

let snapshot: TrackingSnapshot | null = null;
let loading = false;
let error = "";
let modalOpen = false;
let saving = false;
let toast = "";
let toastTimer: number | null = null;

const escapeHtml = (value: string) =>
  value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");

const initials = (name: string) =>
  name
    .trim()
    .split(/\s+/)
    .slice(0, 2)
    .map((part) => part[0]?.toLocaleUpperCase("fr-FR") ?? "")
    .join("");

const trackingUrl = (slug: string) => `${window.location.origin}/t/${encodeURIComponent(slug)}`;

const showToast = (message: string, rerender: () => void) => {
  toast = message;
  if (toastTimer !== null) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toast = "";
    toastTimer = null;
    rerender();
  }, 2_200);
};

export async function refreshTrackingPanel(rerender: () => void, silent = false) {
  if (loading) return;
  loading = true;
  if (!silent) error = "";
  if (!silent) rerender();
  try {
    snapshot = await invoke<TrackingSnapshot>("tracking_links");
    error = "";
  } catch (refreshError) {
    error = String(refreshError instanceof Error ? refreshError.message : refreshError);
  } finally {
    loading = false;
    rerender();
  }
}

export function renderTrackingPanel(): string {
  const links = snapshot?.links ?? [];
  const clicksPerDay = snapshot?.clicksPerDay ?? 5;
  const totalClicks = links.reduce((total, link) => total + link.clickCount, 0);
  const totalDays = links.reduce(
    (total, link) => total + Math.floor(link.clickCount / clicksPerDay),
    0,
  );
  const progressLink = links.reduce<TrackingLink | null>((best, link) => {
    if (!best) return link;
    return link.clickCount % clicksPerDay > best.clickCount % clicksPerDay ? link : best;
  }, null);
  const progress = progressLink ? progressLink.clickCount % clicksPerDay : 0;
  const progressPercent = Math.round((progress / clicksPerDay) * 100);

  const rows = links.length
    ? links.map((link) => `<tr>
        <td><span class="tracking-user"><span class="tracking-avatar">${escapeHtml(initials(link.userName))}</span><span><strong>${escapeHtml(link.userName)}</strong><small>Actif</small></span></span></td>
        <td><code>/t/${escapeHtml(link.slug)}</code></td>
        <td><strong>${link.clickCount.toLocaleString("fr-FR")}</strong><small>${link.clickCount % clicksPerDay}/${clicksPerDay}</small></td>
        <td><span class="tracking-day-pill">+${Math.floor(link.clickCount / clicksPerDay)} j</span></td>
        <td><span class="tracking-row-actions"><a href="${escapeHtml(trackingUrl(link.slug))}" target="_blank" rel="noreferrer" title="Ouvrir le lien de tracking"><i data-lucide="external-link"></i></a><button type="button" data-copy-tracking="${escapeHtml(link.slug)}" title="Copier le lien"><i data-lucide="copy"></i></button></span></td>
      </tr>`).join("")
    : `<tr><td colspan="5"><div class="tracking-empty"><span><i data-lucide="mouse-pointer-click"></i></span><strong>Premier lien à créer</strong><small>Associez un utilisateur à sa page de destination.</small><button type="button" class="tool-button primary" data-open-tracking-modal><i data-lucide="plus"></i><span>Créer un lien</span></button></div></td></tr>`;

  if (!snapshot && loading) {
    return `<section class="tracking-dashboard tracking-loading"><span><i data-lucide="loader-circle"></i></span><strong>Chargement du tracking</strong><small>Lecture des liens et des clics enregistrés…</small></section>`;
  }

  return `<section class="tracking-dashboard" aria-labelledby="trackingTitle">
    <header class="tracking-hero">
      <div><span class="tracking-eyebrow">Duello · Tracking</span><h1 id="trackingTitle">Liens de tracking</h1><p>Créez un lien individuel, suivez chaque visite et ajoutez automatiquement un jour après ${clicksPerDay} clics.</p></div>
      <div class="tracking-hero-actions"><button type="button" class="tool-button primary" data-open-tracking-modal><i data-lucide="plus"></i><span>Créer un lien</span></button></div>
    </header>

    ${error ? `<div class="tracking-error" role="alert"><i data-lucide="circle-alert"></i><span>${escapeHtml(error)}</span><button type="button" data-refresh-tracking>Réessayer</button></div>` : ""}

    <div class="tracking-metrics" aria-label="Indicateurs du tracking">
      <article><span><i data-lucide="mouse-pointer-click"></i></span><div><small>Total des clics</small><strong>${totalClicks.toLocaleString("fr-FR")}</strong><em>Tous les liens</em></div></article>
      <article class="is-accent"><span><i data-lucide="calendar-plus"></i></span><div><small>Jours ajoutés</small><strong>${totalDays.toLocaleString("fr-FR")}</strong><em>${clicksPerDay} clics = 1 jour</em></div></article>
      <article><span><i data-lucide="users"></i></span><div><small>Utilisateurs actifs</small><strong>${links.length.toLocaleString("fr-FR")}</strong><em>Avec un lien personnel</em></div></article>
    </div>

    <div class="tracking-content-grid">
      <article class="tracking-table-card">
        <header><div><span>Gestion</span><strong>Utilisateurs et liens</strong></div><button type="button" data-refresh-tracking title="Actualiser"><i data-lucide="refresh-cw" class="${loading ? "is-spinning" : ""}"></i></button></header>
        <div class="tracking-table-wrap"><table><thead><tr><th>Utilisateur</th><th>Lien personnel</th><th>Clics</th><th>Jours gagnés</th><th><span class="sr-only">Actions</span></th></tr></thead><tbody>${rows}</tbody></table></div>
      </article>

      <aside class="tracking-rule-card">
        <span class="tracking-rule-number">${clicksPerDay}</span>
        <span class="tracking-eyebrow">Règle active</span>
        <h2>Règle de calcul</h2>
        <p>Chaque série complète de ${clicksPerDay} visites ajoute un jour à l'utilisateur concerné.</p>
        <div class="tracking-progress-head"><span>${progressLink ? escapeHtml(progressLink.userName) : "Prochain jour"}</span><strong>${progress} / ${clicksPerDay}</strong></div>
        <div class="tracking-progress"><span style="width:${progressPercent}%"></span></div>
        <button type="button" data-open-tracking-modal><span>Nouveau lien</span><i data-lucide="arrow-up-right"></i></button>
      </aside>
    </div>

    ${modalOpen ? `<div class="tracking-modal-backdrop" data-tracking-modal-backdrop><section class="tracking-modal" role="dialog" aria-modal="true" aria-labelledby="trackingModalTitle"><button type="button" class="tracking-modal-close" data-close-tracking-modal aria-label="Fermer"><i data-lucide="x"></i></button><span class="tracking-eyebrow">Nouveau tracking</span><h2 id="trackingModalTitle">Créer un lien utilisateur</h2><p>Le compteur avancera à chaque ouverture puis redirigera vers la destination choisie.</p><form data-tracking-form><label><span>Nom de l'utilisateur</span><input name="userName" maxlength="80" autocomplete="off" placeholder="Ex. Amina Martin" required autofocus /></label><label><span>Page de destination</span><input name="destinationUrl" type="url" placeholder="https://…" required /></label>${error ? `<div class="tracking-form-error">${escapeHtml(error)}</div>` : ""}<button type="submit" class="tool-button primary" ${saving ? "disabled" : ""}><span>${saving ? "Création…" : "Créer le lien"}</span><i data-lucide="arrow-right"></i></button></form></section></div>` : ""}
    ${toast ? `<div class="tracking-toast" role="status"><i data-lucide="check"></i><span>${escapeHtml(toast)}</span></div>` : ""}
  </section>`;
}

export function bindTrackingPanel({ rerender, renderIcons }: TrackingPanelBindings) {
  const root = document.querySelector<HTMLElement>(".tracking-dashboard");
  if (!root) return;

  if (!snapshot && !loading) void refreshTrackingPanel(rerender);

  root.querySelectorAll<HTMLButtonElement>("[data-open-tracking-modal]").forEach((button) => {
    button.addEventListener("click", () => {
      error = "";
      modalOpen = true;
      rerender();
    });
  });
  root.querySelectorAll<HTMLButtonElement>("[data-close-tracking-modal]").forEach((button) => {
    button.addEventListener("click", () => {
      modalOpen = false;
      error = "";
      rerender();
    });
  });
  root.querySelector<HTMLElement>("[data-tracking-modal-backdrop]")?.addEventListener("pointerdown", (event) => {
    if (event.target !== event.currentTarget) return;
    modalOpen = false;
    error = "";
    rerender();
  });
  root.querySelectorAll<HTMLButtonElement>("[data-refresh-tracking]").forEach((button) => {
    button.addEventListener("click", () => void refreshTrackingPanel(rerender));
  });
  root.querySelectorAll<HTMLButtonElement>("[data-copy-tracking]").forEach((button) => {
    button.addEventListener("click", () => {
      const slug = button.dataset.copyTracking;
      if (!slug) return;
      void navigator.clipboard.writeText(trackingUrl(slug)).then(() => {
        showToast("Lien copié", rerender);
        rerender();
      });
    });
  });

  root.querySelector<HTMLFormElement>("[data-tracking-form]")?.addEventListener("submit", (event) => {
    event.preventDefault();
    if (saving) return;
    const form = new FormData(event.currentTarget as HTMLFormElement);
    saving = true;
    error = "";
    rerender();
    void invoke<TrackingLink>("create_tracking_link", {
      request: {
        userName: String(form.get("userName") ?? ""),
        destinationUrl: String(form.get("destinationUrl") ?? ""),
      },
    }).then((link) => {
      const current = snapshot ?? { clicksPerDay: 5, links: [] };
      snapshot = { ...current, links: [link, ...current.links] };
      modalOpen = false;
      showToast("Lien créé", rerender);
    }).catch((createError) => {
      error = String(createError instanceof Error ? createError.message : createError);
    }).finally(() => {
      saving = false;
      rerender();
    });
  });

  renderIcons(root);
}
