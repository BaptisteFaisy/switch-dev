const numberFormatter = new Intl.NumberFormat("fr-FR");
const compactFormatter = new Intl.NumberFormat("fr-FR", {
  notation: "compact",
  maximumFractionDigits: 1,
});
const dateFormatter = new Intl.DateTimeFormat("fr-FR", { day: "numeric", month: "short" });
const dateLongFormatter = new Intl.DateTimeFormat("fr-FR", {
  weekday: "long",
  day: "numeric",
  month: "long",
});
const colourPalette = ["#f37fd5", "#65e8df", "#f3c66e", "#9aa8ff", "#9ee37d", "#f58b77"];

const state = {
  config: null,
  accounts: [],
  metrics: null,
  media: null,
  range: 7,
  platform: "all",
  selectedAccountIds: new Set(),
  provider: "instagram",
  chartGeometry: null,
  loading: false,
  metricsStatus: "idle",
  metricsRequestId: 0,
  metricsError: null,
  mediaError: null,
  chartFocusIndex: 0,
  chartTooltipPinned: false,
  oauthPending: false,
  oauthRequestId: 0,
  syncPending: false,
};

const elements = {
  socialApp: document.querySelector("#socialApp"),
  syncStatus: document.querySelector("#syncStatus"),
  syncButton: document.querySelector("#syncButton"),
  connectButton: document.querySelector("#connectButton"),
  manageAccountsButton: document.querySelector("#manageAccountsButton"),
  modeBanner: document.querySelector("#modeBanner"),
  modeDetailsButton: document.querySelector("#modeDetailsButton"),
  rangeFilter: document.querySelector("#rangeFilter"),
  platformFilter: document.querySelector("#platformFilter"),
  accountFilter: document.querySelector("#accountFilter"),
  toggleTableButton: document.querySelector("#toggleTableButton"),
  dataTableCard: document.querySelector("#dataTableCard"),
  dataTable: document.querySelector("#dataTable"),
  mediaSection: document.querySelector("#mediaSection"),
  mediaSummary: document.querySelector("#mediaSummary"),
  mediaGrid: document.querySelector("#mediaGrid"),
  totalViews: document.querySelector("#totalViews"),
  totalViewsCaption: document.querySelector("#totalViewsCaption"),
  trendBadge: document.querySelector("#trendBadge"),
  dailyAverage: document.querySelector("#dailyAverage"),
  bestDayViews: document.querySelector("#bestDayViews"),
  bestDayLabel: document.querySelector("#bestDayLabel"),
  selectedCount: document.querySelector("#selectedCount"),
  platformCount: document.querySelector("#platformCount"),
  accountCountCaption: document.querySelector("#accountCountCaption"),
  chartWrap: document.querySelector("#chartWrap"),
  canvas: document.querySelector("#viewsChart"),
  chartTooltip: document.querySelector("#chartTooltip"),
  chartAnnouncement: document.querySelector("#chartAnnouncement"),
  chartEmpty: document.querySelector("#chartEmpty"),
  resetFiltersButton: document.querySelector("#resetFiltersButton"),
  chartSummary: document.querySelector("#chartSummary"),
  chartLegend: document.querySelector("#chartLegend"),
  lastUpdated: document.querySelector("#lastUpdated"),
  accountsGrid: document.querySelector("#accountsGrid"),
  connectModal: document.querySelector("#connectModal"),
  modalCloseButton: document.querySelector("#modalCloseButton"),
  providerPicker: document.querySelector("#providerPicker"),
  liveConnectTitle: document.querySelector("#liveConnectTitle"),
  liveConnectCopy: document.querySelector("#liveConnectCopy"),
  liveConnectButton: document.querySelector("#liveConnectButton"),
  oauthError: document.querySelector("#oauthError"),
  demoAccountForm: document.querySelector("#demoAccountForm"),
  demoDivider: document.querySelector("#demoDivider"),
  demoSubmitButton: document.querySelector("#demoSubmitButton"),
  displayNameInput: document.querySelector("#displayNameInput"),
  handleInput: document.querySelector("#handleInput"),
  formError: document.querySelector("#formError"),
  toast: document.querySelector("#toast"),
};

let toastTimer = 0;
let modalReturnFocus = null;

async function api(path, options = {}) {
  const headers = new Headers(options.headers ?? {});
  headers.set("accept", "application/json");
  if (options.body) headers.set("content-type", "application/json");
  if (options.method && options.method !== "GET") {
    const token = state.config?.csrfToken;
    if (!token) throw new Error("Protection de session indisponible. Recharge le dashboard.");
    headers.set("x-switch-social-request", token);
  }
  const response = await fetch(`/api/social${path}`, {
    cache: "no-store",
    credentials: "same-origin",
    ...options,
    headers,
  });
  let payload;
  try {
    payload = await response.json();
  } catch {
    payload = { ok: false, error: "Réponse illisible du service Social Analytics." };
  }
  if (!response.ok || payload?.ok === false) {
    const error = new Error(payload?.error || `Erreur ${response.status}`);
    error.status = response.status;
    error.code = payload?.code;
    throw error;
  }
  return payload;
}

function showToast(message, error = false) {
  window.clearTimeout(toastTimer);
  elements.toast.textContent = message;
  elements.toast.classList.toggle("error", error);
  elements.toast.hidden = false;
  toastTimer = window.setTimeout(() => { elements.toast.hidden = true; }, 3200);
}

function setServiceStatus(label, status = "ready") {
  elements.syncStatus.className = `sync-status ${status}`;
  elements.syncStatus.innerHTML = `<i></i>${escapeHtml(label)}`;
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

function accountPlatformLabel(platform) {
  return platform === "instagram" ? "Instagram" : "TikTok";
}

function accountPlatformShort(platform) {
  return platform === "instagram" ? "IG" : "TT";
}

function seriesColourClass(colour) {
  const index = colourPalette.indexOf(String(colour).toLowerCase());
  return `series-colour-${index >= 0 ? index : 0}`;
}

function platformAccounts() {
  return state.accounts.filter((account) =>
    state.platform === "all" || account.platform === state.platform,
  );
}

function selectedAccounts() {
  return platformAccounts().filter((account) => state.selectedAccountIds.has(account.id));
}

function selectedDataMode() {
  const accounts = selectedAccounts();
  const hasDemo = accounts.some((account) => account.connectionMode === "demo");
  const hasLive = accounts.some((account) => account.connectionMode !== "demo");
  if (hasDemo && hasLive) return "mixed";
  if (hasDemo) return "demo";
  if (hasLive) return "live";
  return "none";
}

function selectedDataStatus() {
  const accounts = selectedAccounts();
  if (!accounts.length) {
    return {
      label: state.accounts.length ? "Aucun compte sélectionné" : "Service prêt — aucun compte",
      status: "ready",
    };
  }
  const mode = selectedDataMode();
  if (mode === "demo") return { label: "Mode test — données simulées", status: "" };
  if (mode === "mixed") return { label: "Données mixtes — officiel + test", status: "" };
  return { label: "Données officielles à jour", status: "ready" };
}

function formatSignedViews(value) {
  if (!Number.isFinite(value)) return "—";
  return `${value > 0 ? "+" : ""}${numberFormatter.format(value)}`;
}

function formatDateKey(key, long = false) {
  return (long ? dateLongFormatter : dateFormatter).format(new Date(`${key}T12:00:00`));
}

function renderMode() {
  elements.modeBanner.hidden = !state.config?.demoMode;
  elements.demoAccountForm.hidden = !state.config?.demoMode;
  elements.demoDivider.hidden = !state.config?.demoMode;
}

function renderProviderConfiguration() {
  const platform = state.provider;
  const provider = state.config?.providers?.[platform];
  const label = accountPlatformLabel(platform);
  elements.liveConnectTitle.textContent = `Connexion officielle ${label}`;
  if (provider?.configured) {
    elements.liveConnectCopy.textContent = platform === "instagram"
      ? "Connexion sécurisée au compte professionnel et lecture des vues exactes de ses Reels."
      : "Connexion sécurisée TikTok Login Kit avec accès au profil et aux vidéos publiques.";
    elements.liveConnectButton.disabled = state.oauthPending;
    elements.liveConnectButton.textContent = state.oauthPending ? "Ouverture…" : `Continuer avec ${label}`;
  } else {
    elements.liveConnectCopy.textContent = "La connexion officielle doit d’abord être activée dans les secrets de Switch développement.";
    elements.liveConnectButton.disabled = true;
    elements.liveConnectButton.textContent = "Configuration requise";
  }
}

function safeProviderUrl(value, platform) {
  try {
    const url = new URL(value);
    if (url.protocol !== "https:") return null;
    const hostname = url.hostname.toLowerCase();
    const allowed = platform === "instagram"
      ? hostname === "instagram.com" || hostname.endsWith(".instagram.com")
      : hostname === "tiktok.com" || hostname.endsWith(".tiktok.com");
    return allowed ? url.href : null;
  } catch {
    return null;
  }
}

function renderMediaDayPoints(points = []) {
  const visible = points.slice(-(state.range === 7 ? 7 : 14));
  return `<div class="media-days" aria-label="Vues gagnées lors des derniers relevés">${visible.map((point) => `
    <span class="media-day ${point.available ? "available" : "missing"} ${Number(point.views) < 0 ? "negative" : ""}" title="${escapeHtml(formatDateKey(point.date, true))}">
      <small>${escapeHtml(formatDateKey(point.date))}</small>
      <b>${formatSignedViews(point.views)}</b>
    </span>
  `).join("")}</div>`;
}

function renderMedia() {
  const items = state.media?.items ?? [];
  if (state.loading && !items.length) {
    elements.mediaSummary.textContent = "Chargement des contenus…";
    elements.mediaGrid.innerHTML = `<div class="accounts-empty">Lecture des compteurs officiels en cours…</div>`;
    return;
  }
  if (state.mediaError) {
    elements.mediaSummary.textContent = "Données temporairement indisponibles";
    elements.mediaGrid.innerHTML = `<div class="accounts-empty">${escapeHtml(state.mediaError)}</div>`;
    return;
  }
  elements.mediaSummary.textContent = items.length
    ? `${items.length} contenu${items.length > 1 ? "s" : ""} · ${state.loading ? "actualisation…" : "compteurs officiels"}`
    : "Aucun contenu disponible";
  if (!items.length) {
    const emptyMessage = state.accounts.length && !selectedAccounts().length
      ? "Sélectionnez au moins un compte pour afficher ses Reels ou vidéos TikTok."
      : "Connectez un compte officiel puis lancez une actualisation pour récupérer ses Reels ou vidéos TikTok.";
    elements.mediaGrid.innerHTML = `<div class="accounts-empty">${emptyMessage}</div>`;
    return;
  }
  elements.mediaGrid.innerHTML = items.map((item) => {
    const account = state.accounts.find((candidate) => candidate.id === item.accountId);
    const title = String(item.title || (item.platform === "instagram" ? "Reel sans légende" : "Vidéo sans titre"));
    const providerUrl = safeProviderUrl(item.permalink, item.platform);
    const periodLabel = item.periodComplete ? `${state.range} jours` : "Période partielle";
    const published = item.publishedAt && Number.isFinite(Date.parse(item.publishedAt))
      ? new Intl.DateTimeFormat("fr-FR", { day: "numeric", month: "short", year: "numeric" }).format(new Date(item.publishedAt))
      : "Date inconnue";
    return `<article class="media-card">
      <header>
        <span class="provider-logo ${escapeHtml(item.platform)}">${accountPlatformShort(item.platform)}</span>
        <span><small>${accountPlatformLabel(item.platform)}${account ? ` · @${escapeHtml(account.handle)}` : ""}</small><strong>${escapeHtml(title)}</strong></span>
      </header>
      <div class="media-metrics">
        <span><small>Total exact</small><strong>${Number.isFinite(item.latestViews) ? numberFormatter.format(item.latestViews) : "—"}</strong></span>
        <span class="${Number(item.periodViews) < 0 ? "negative" : ""}"><small>${escapeHtml(periodLabel)}</small><strong>${formatSignedViews(item.periodViews)}</strong></span>
      </div>
      ${renderMediaDayPoints(item.points)}
      <footer><span>Publié le ${escapeHtml(published)}</span>${providerUrl ? `<a href="${escapeHtml(providerUrl)}" target="_blank" rel="noopener noreferrer" aria-label="${escapeHtml(`Voir ${title} sur ${accountPlatformLabel(item.platform)}`)}">Voir le contenu</a>` : ""}</footer>
    </article>`;
  }).join("");
}

function renderAccountFilters() {
  const accounts = platformAccounts();
  elements.accountFilter.innerHTML = accounts.map((account) => {
    const active = state.selectedAccountIds.has(account.id);
    return `<button type="button" class="account-chip ${active ? "active" : ""} ${seriesColourClass(account.colour)}" data-account-filter="${escapeHtml(account.id)}" aria-pressed="${active}">
      <span class="chip-dot"></span>
      <span class="chip-platform">${accountPlatformShort(account.platform)}</span>
      <span>@${escapeHtml(account.handle)}</span>
      ${account.connectionMode === "demo" ? '<span class="chip-mode">TEST</span>' : ""}
    </button>`;
  }).join("");
  if (!accounts.length) {
    elements.accountFilter.innerHTML = `<span class="accounts-empty">Aucun compte ${state.platform === "all" ? "connecté" : accountPlatformLabel(state.platform)}.</span>`;
  }
}

function renderLegend() {
  const series = state.metrics?.series ?? [];
  elements.chartLegend.innerHTML = series.map(({ account }) => `
    <span class="legend-chip ${seriesColourClass(account.colour)}">
      <i class="legend-dot"></i>
      <span class="legend-platform">${accountPlatformShort(account.platform)}</span>
      <span>@${escapeHtml(account.handle)}</span>
      ${account.connectionMode === "demo" ? '<span class="legend-mode">TEST</span>' : ""}
    </span>
  `).join("");
}

function renderAccounts() {
  if (!state.accounts.length) {
    elements.accountsGrid.innerHTML = `<div class="accounts-empty">Connectez un premier compte Instagram ou TikTok pour commencer.</div>`;
    return;
  }
  elements.accountsGrid.innerHTML = state.accounts.map((account) => {
    const syncLabel = account.syncedAt
      ? `Synchronisé ${new Intl.DateTimeFormat("fr-FR", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" }).format(new Date(account.syncedAt))}`
      : "Pas encore synchronisé";
    let modeLabel = "Compte test · données simulées";
    let statusClass = "demo";
    if (account.connectionMode !== "demo") {
      if (account.status === "syncing") {
        modeLabel = "Synchronisation officielle en cours";
        statusClass = "syncing";
      } else if (account.status === "error") {
        modeLabel = "Échec de synchronisation · dernières données conservées";
        statusClass = "error";
      } else if (account.status === "no_data") {
        modeLabel = "Compte officiel · aucune métrique publiée";
        statusClass = "syncing";
      } else {
        modeLabel = "Compte officiel";
        statusClass = "connected";
      }
    }
    return `<article class="account-card">
      <span class="provider-logo ${escapeHtml(account.platform)}">${accountPlatformShort(account.platform)}</span>
      <div class="account-card-copy">
        <strong>${escapeHtml(account.displayName)}</strong>
        <small>${accountPlatformLabel(account.platform)} · @${escapeHtml(account.handle)}</small>
        <span class="${statusClass}">${escapeHtml(modeLabel)} · ${escapeHtml(syncLabel)}</span>
      </div>
      <button type="button" data-delete-account="${escapeHtml(account.id)}" aria-label="Déconnecter @${escapeHtml(account.handle)}" title="Déconnecter">×</button>
    </article>`;
  }).join("");
}

function metricDayTotals(metrics) {
  if (!metrics?.dates?.length) return [];
  return metrics.dates.map((date, index) => {
    const points = metrics.series.map((series) => series.points[index]);
    const availablePoints = points.filter((point) => Number.isFinite(point?.views));
    return {
      date,
      views: availablePoints.reduce((total, point) => total + point.views, 0),
      available: availablePoints.length > 0,
      complete: availablePoints.length === points.length,
    };
  });
}

function renderKpis() {
  const metrics = state.metrics;
  const total = metrics?.currentTotal;
  const previous = metrics?.previousTotal;
  const dailyTotals = metricDayTotals(metrics);
  const availableDays = dailyTotals.filter((day) => day.available);
  const hasMeasuredData = availableDays.length > 0;
  const best = availableDays.reduce((current, day) => !current || day.views > current.views ? day : current, null);
  const average = hasMeasuredData ? Math.round(total / availableDays.length) : null;
  const completeCurrentPeriod = hasMeasuredData && !metrics?.missingPoints;
  const change = completeCurrentPeriod && metrics?.comparisonAvailable && previous > 0
    ? ((total - previous) / previous) * 100
    : null;
  const accounts = selectedAccounts();
  const dataMode = selectedDataMode();
  const dataModeSuffix = dataMode === "demo"
    ? " · TEST simulé"
    : dataMode === "mixed" ? " · officiel + TEST" : "";
  const platformCount = new Set(accounts.map((account) => account.platform)).size;

  elements.totalViews.textContent = hasMeasuredData ? numberFormatter.format(total) : "—";
  elements.totalViewsCaption.textContent = state.metricsError
    ? "Données temporairement indisponibles"
    : `${accounts.length} compte${accounts.length > 1 ? "s" : ""} · ${state.range} jours${metrics?.missingPoints ? " · données partielles" : ""}${dataModeSuffix}`;
  elements.dailyAverage.textContent = hasMeasuredData ? numberFormatter.format(average) : "—";
  elements.bestDayViews.textContent = best ? numberFormatter.format(best.views) : "—";
  elements.bestDayLabel.textContent = best ? formatDateKey(best.date, true) : "Aucune donnée";
  elements.selectedCount.textContent = numberFormatter.format(accounts.length);
  elements.platformCount.textContent = `${platformCount} PLATEFORME${platformCount > 1 ? "S" : ""}`;
  elements.accountCountCaption.textContent = `${state.accounts.length} connecté${state.accounts.length > 1 ? "s" : ""} au total`;
  elements.trendBadge.className = "";
  if (!accounts.length || !hasMeasuredData || state.metricsError) {
    elements.trendBadge.textContent = "—";
  } else if (!completeCurrentPeriod) {
    elements.trendBadge.textContent = "PARTIEL";
  } else if (change == null) {
    elements.trendBadge.textContent = metrics?.comparisonAvailable ? "NOUVEAU" : "HIST. PARTIEL";
  } else {
    const positive = change >= 0;
    elements.trendBadge.textContent = `${positive ? "+" : ""}${change.toLocaleString("fr-FR", { maximumFractionDigits: 1 })} %`;
    elements.trendBadge.classList.add(positive ? "positive" : "negative");
  }
}

function renderTable() {
  const metrics = state.metrics;
  if (!metrics?.dates?.length || !metrics.series.length) {
    elements.dataTable.innerHTML = `<tbody><tr><td>Aucune donnée à afficher.</td></tr></tbody>`;
    return;
  }
  const header = metrics.series.map(({ account }) => `<th scope="col">${accountPlatformShort(account.platform)} · @${escapeHtml(account.handle)}</th>`).join("");
  const rows = [...metrics.dates].reverse().map((date) => {
    const sourceIndex = metrics.dates.indexOf(date);
    const cells = metrics.series.map((series) => {
      const point = series.points[sourceIndex];
      const value = Number.isFinite(point?.views) ? numberFormatter.format(point.views) : "—";
      return `<td><strong>${value}</strong>${point?.provisional ? "<small>PROV.</small>" : ""}</td>`;
    }).join("");
    const dayPoints = metrics.series.map((series) => series.points[sourceIndex]);
    const knownPoints = dayPoints.filter((point) => Number.isFinite(point?.views));
    const total = knownPoints.reduce((sum, point) => sum + point.views, 0);
    const totalLabel = knownPoints.length ? numberFormatter.format(total) : "—";
    return `<tr><th scope="row">${escapeHtml(formatDateKey(date, true))}</th>${cells}<td><strong>${totalLabel}</strong></td></tr>`;
  }).join("");
  elements.dataTable.innerHTML = `<thead><tr><th scope="col">Jour</th>${header}<th scope="col">Total</th></tr></thead><tbody>${rows}</tbody>`;
}

function niceMaximum(value) {
  if (value <= 0) return 100;
  const magnitude = 10 ** Math.floor(Math.log10(value));
  const normalized = value / magnitude;
  const nice = normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10;
  return nice * magnitude;
}

function drawChart() {
  const canvas = elements.canvas;
  const wrap = elements.chartWrap;
  const metrics = state.metrics;
  const series = metrics?.series ?? [];
  const dates = metrics?.dates ?? [];
  const hasData = series.some((item) => item.points.some((point) => Number.isFinite(point.views)));
  const empty = !series.length || !dates.length || !hasData;
  elements.chartEmpty.hidden = !empty;
  canvas.hidden = empty;
  elements.chartTooltip.hidden = true;
  if (empty) {
    state.chartGeometry = null;
    const emptyTitle = elements.chartEmpty.querySelector("strong");
    const emptyCopy = elements.chartEmpty.querySelector("span");
    if (state.metricsError) {
      emptyTitle.textContent = "Données temporairement indisponibles";
      emptyCopy.textContent = state.metricsError;
      elements.resetFiltersButton.hidden = true;
      elements.chartSummary.textContent = "La dernière requête de métriques a échoué.";
    } else if (selectedAccounts().length) {
      emptyTitle.textContent = "Aucune donnée disponible";
      emptyCopy.textContent = "La première synchronisation est en cours ou la plateforme n’a pas encore publié de métriques.";
      elements.resetFiltersButton.hidden = true;
      elements.chartSummary.textContent = "Aucun point quotidien disponible pour cette sélection.";
    } else {
      emptyTitle.textContent = "Aucun compte sélectionné";
      emptyCopy.textContent = "Choisissez au moins un compte pour afficher ses vues.";
      elements.resetFiltersButton.hidden = false;
      elements.chartSummary.textContent = "Aucune série sélectionnée.";
    }
    return;
  }

  const rect = wrap.getBoundingClientRect();
  if (rect.width < 120 || rect.height < 120) return;
  const width = Math.max(1, Math.floor(rect.width));
  const height = Math.max(240, Math.floor(rect.height));
  const ratio = Math.min(2, window.devicePixelRatio || 1);
  canvas.width = Math.round(width * ratio);
  canvas.height = Math.round(height * ratio);
  const context = canvas.getContext("2d");
  context.setTransform(ratio, 0, 0, ratio, 0, 0);
  context.clearRect(0, 0, width, height);

  const padding = { top: 28, right: 21, bottom: 39, left: width < 520 ? 44 : 58 };
  const plotWidth = width - padding.left - padding.right;
  const plotHeight = height - padding.top - padding.bottom;
  const allValues = series
    .flatMap((item) => item.points.map((point) => point.views))
    .filter(Number.isFinite);
  const yMax = niceMaximum(Math.max(...allValues, 1) * 1.08);
  const steps = 5;

  context.font = "9px Inter, system-ui, sans-serif";
  context.lineWidth = 1;
  context.textBaseline = "middle";
  for (let step = 0; step <= steps; step += 1) {
    const y = padding.top + (plotHeight * step) / steps;
    const value = yMax - (yMax * step) / steps;
    context.strokeStyle = step === steps ? "#303837" : "#1a201f";
    context.beginPath();
    context.moveTo(padding.left, Math.round(y) + 0.5);
    context.lineTo(width - padding.right, Math.round(y) + 0.5);
    context.stroke();
    context.fillStyle = "#707a77";
    context.textAlign = "right";
    context.fillText(compactFormatter.format(value), padding.left - 10, y);
  }

  const xFor = (index) => padding.left + (dates.length === 1 ? plotWidth / 2 : (plotWidth * index) / (dates.length - 1));
  const yFor = (value) => padding.top + plotHeight - (Math.max(0, value) / yMax) * plotHeight;
  const labelEvery = Math.max(1, Math.ceil(dates.length / (width < 620 ? 5 : 8)));
  context.textAlign = "center";
  context.textBaseline = "top";
  dates.forEach((date, index) => {
    if (index % labelEvery !== 0 && index !== dates.length - 1) return;
    context.fillStyle = "#707a77";
    context.fillText(formatDateKey(date), xFor(index), height - padding.bottom + 13);
  });

  series.forEach((item) => {
    context.save();
    context.strokeStyle = item.account.colour;
    context.fillStyle = item.account.colour;
    context.lineWidth = 2.2;
    context.lineJoin = "round";
    context.lineCap = "round";
    if (item.account.platform === "tiktok") context.setLineDash([7, 5]);
    context.beginPath();
    let segmentStarted = false;
    item.points.forEach((point, index) => {
      if (!Number.isFinite(point.views)) {
        segmentStarted = false;
        return;
      }
      const x = xFor(index);
      const y = yFor(point.views);
      if (!segmentStarted) context.moveTo(x, y);
      else context.lineTo(x, y);
      segmentStarted = true;
    });
    context.stroke();
    context.setLineDash([]);
    if (dates.length <= 30) {
      item.points.forEach((point, index) => {
        if (!Number.isFinite(point.views)) return;
        const x = xFor(index);
        const y = yFor(point.views);
        context.beginPath();
        context.arc(x, y, 3.2, 0, Math.PI * 2);
        context.fill();
        context.strokeStyle = "#090c0b";
        context.lineWidth = 1.5;
        context.stroke();
        context.strokeStyle = item.account.colour;
      });
    }
    context.restore();
  });

  state.chartGeometry = { width, height, padding, plotWidth, plotHeight, xFor, dates, series };
  const totals = metricDayTotals(metrics);
  const completeDays = totals.filter((day) => day.complete);
  const start = completeDays[0];
  const end = completeDays.at(-1);
  const direction = start && end && start.date !== end.date
    ? (end.views > start.views ? "en hausse" : end.views < start.views ? "en baisse" : "stable")
    : null;
  const partialNote = metrics.missingPoints
    ? ` ${metrics.missingPoints} point${metrics.missingPoints > 1 ? "s" : ""} indisponible${metrics.missingPoints > 1 ? "s" : ""} ne ${metrics.missingPoints > 1 ? "sont" : "sera"} pas tracé${metrics.missingPoints > 1 ? "s" : ""}.`
    : "";
  const periodLabel = `Du ${formatDateKey(dates[0], true)} au ${formatDateKey(dates.at(-1), true)}`;
  const trendLabel = direction
    ? `la tendance totale est ${direction}`
    : "la tendance est indisponible sur les données partielles";
  const dataQualifier = selectedDataMode() === "demo"
    ? "simulées (TEST)"
    : selectedDataMode() === "mixed" ? "mixtes (officielles + TEST)" : "mesurées officiellement";
  elements.chartSummary.textContent = `${periodLabel}, ${trendLabel}. ${numberFormatter.format(metrics.currentTotal)} vues ${dataQualifier} sont affichées sur ${series.length} compte${series.length > 1 ? "s" : ""}.${partialNote}`;
}

function showChartTooltipAtIndex(requestedIndex) {
  const geometry = state.chartGeometry;
  if (!geometry) return;
  const index = Math.max(0, Math.min(geometry.dates.length - 1, requestedIndex));
  state.chartFocusIndex = index;
  const tooltipX = Math.max(94, Math.min(geometry.width - 94, geometry.xFor(index)));
  const topValue = Math.max(0, ...geometry.series.map((item) =>
    Number.isFinite(item.points[index]?.views) ? item.points[index].views : 0
  ));
  const tooltipY = geometry.padding.top + geometry.plotHeight - (topValue / niceMaximum(
    Math.max(...geometry.series.flatMap((item) => item.points.map((point) => point.views ?? 0)), 1) * 1.08,
  )) * geometry.plotHeight;
  elements.chartTooltip.innerHTML = `<strong>${escapeHtml(formatDateKey(geometry.dates[index], true))}</strong>${geometry.series.map((item) => `
    <span class="${seriesColourClass(item.account.colour)}"><span><i></i>@${escapeHtml(item.account.handle)}</span><b>${Number.isFinite(item.points[index]?.views) ? numberFormatter.format(item.points[index].views) : "—"}</b></span>
  `).join("")}`;
  elements.chartTooltip.style.left = `${tooltipX}px`;
  elements.chartTooltip.style.top = `${Math.max(120, tooltipY)}px`;
  elements.chartTooltip.hidden = false;
  elements.chartAnnouncement.textContent = `${formatDateKey(geometry.dates[index], true)}. ${geometry.series.map((item) =>
    `@${item.account.handle} : ${Number.isFinite(item.points[index]?.views) ? numberFormatter.format(item.points[index].views) : "donnée indisponible"}`
  ).join(". ")}.`;
}

function showChartTooltip(event) {
  const geometry = state.chartGeometry;
  if (!geometry) return;
  const rect = elements.canvas.getBoundingClientRect();
  const x = (event.clientX - rect.left) * (geometry.width / rect.width);
  const rawIndex = geometry.dates.length === 1
    ? 0
    : Math.round(((x - geometry.padding.left) / geometry.plotWidth) * (geometry.dates.length - 1));
  showChartTooltipAtIndex(rawIndex);
}

function renderLastUpdated() {
  const timestamps = selectedAccounts()
    .map((account) => account.syncedAt)
    .filter(Boolean)
    .map((value) => new Date(value))
    .filter((value) => Number.isFinite(value.getTime()));
  const value = timestamps.sort((left, right) => right - left)[0];
  elements.lastUpdated.textContent = value
    ? `Dernière collecte fournisseur : ${new Intl.DateTimeFormat("fr-FR", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" }).format(value)}`
    : "Dernière collecte fournisseur : —";
}

function renderAll() {
  elements.socialApp.setAttribute("aria-busy", String(state.loading));
  elements.chartWrap.classList.toggle("loading", state.loading);
  elements.syncButton.disabled = state.loading || state.syncPending;
  renderMode();
  renderProviderConfiguration();
  renderAccountFilters();
  renderAccounts();
  renderLegend();
  renderKpis();
  renderTable();
  renderMedia();
  renderLastUpdated();
  drawChart();
}

async function loadDashboard() {
  const requestId = ++state.metricsRequestId;
  const ids = selectedAccounts().map((account) => account.id);
  if (!ids.length) {
    state.metricsError = null;
    state.mediaError = null;
    state.metricsStatus = "empty-selection";
    state.metrics = {
      range: state.range,
      dates: [],
      series: [],
      currentTotal: 0,
      previousTotal: 0,
      generatedAt: new Date().toISOString(),
    };
    state.media = {
      range: state.range,
      dates: [],
      items: [],
      generatedAt: new Date().toISOString(),
    };
    state.loading = false;
    elements.syncButton.disabled = false;
    if (!state.syncPending) {
      const status = selectedDataStatus();
      setServiceStatus(status.label, status.status);
    }
    renderAll();
    return;
  }
  state.loading = true;
  state.metricsStatus = "loading";
  elements.syncButton.disabled = true;
  renderAll();
  try {
    const accountQuery = encodeURIComponent(ids.join(","));
    const [metricsResult, mediaResult] = await Promise.allSettled([
      api(`/metrics?range=${state.range}&accounts=${accountQuery}`),
      api(`/media?range=${state.range}&accounts=${accountQuery}`),
    ]);
    if (requestId !== state.metricsRequestId) return;
    if (metricsResult.status === "fulfilled") {
      state.metricsError = null;
      state.metrics = metricsResult.value;
      state.metricsStatus = "ready";
    } else {
      state.metricsError = metricsResult.reason?.message || "Métriques quotidiennes indisponibles.";
      state.metrics = null;
      state.metricsStatus = "error";
    }
    if (mediaResult.status === "fulfilled") {
      state.mediaError = null;
      state.media = mediaResult.value;
    } else {
      state.mediaError = mediaResult.reason?.message || "Compteurs par contenu indisponibles.";
      state.media = null;
    }
    if (state.metricsError && state.mediaError) {
      throw metricsResult.reason || mediaResult.reason;
    }
    if (state.metricsError || state.mediaError) {
      if (!state.syncPending) setServiceStatus("Données partielles", "error");
      showToast(state.metricsError || state.mediaError, true);
    } else {
      if (!state.syncPending) {
        const status = selectedDataStatus();
        setServiceStatus(status.label, status.status);
      }
    }
  } catch (error) {
    if (requestId !== state.metricsRequestId) return;
    if (!state.syncPending) setServiceStatus("Service indisponible", "error");
    showToast(error.message, true);
    if (!state.metricsError) {
      state.metricsError = error.message;
      state.metrics = null;
      state.metricsStatus = "error";
    }
    if (!state.mediaError) {
      state.mediaError = error.message;
      state.media = null;
    }
  } finally {
    if (requestId !== state.metricsRequestId) return;
    state.loading = false;
    elements.syncButton.disabled = false;
    renderAll();
  }
}

async function bootstrap() {
  setServiceStatus("Connexion au service…", "");
  try {
    const [config, accountsPayload] = await Promise.all([
      api("/config"),
      api("/accounts"),
    ]);
    state.config = config;
    state.accounts = accountsPayload.accounts;
    state.accounts.forEach((account) => state.selectedAccountIds.add(account.id));
    await loadDashboard();
  } catch (error) {
    state.metricsError = error.message;
    state.mediaError = error.message;
    state.metrics = null;
    state.media = null;
    state.metricsStatus = "error";
    setServiceStatus(error.status === 401 ? "Session Switch requise" : "Service indisponible", "error");
    showToast(error.message, true);
    renderAll();
  }
}

function openModal(returnFocus = document.activeElement) {
  modalReturnFocus = returnFocus;
  elements.formError.hidden = true;
  elements.oauthError.hidden = true;
  elements.connectModal.hidden = false;
  elements.socialApp.inert = true;
  elements.socialApp.setAttribute("aria-hidden", "true");
  document.body.style.overflow = "hidden";
  window.setTimeout(() => {
    const initialFocus = state.config?.demoMode ? elements.displayNameInput : elements.liveConnectButton;
    (initialFocus.disabled ? elements.modalCloseButton : initialFocus).focus();
  }, 0);
}

function closeModal() {
  if (state.oauthPending) {
    state.oauthRequestId += 1;
    state.oauthPending = false;
  }
  elements.connectModal.hidden = true;
  elements.socialApp.inert = false;
  elements.socialApp.removeAttribute("aria-hidden");
  document.body.style.overflow = "";
  elements.demoAccountForm.reset();
  elements.formError.hidden = true;
  elements.oauthError.hidden = true;
  elements.providerPicker.querySelectorAll("button").forEach((button) => { button.disabled = false; });
  elements.modalCloseButton.disabled = false;
  renderProviderConfiguration();
  modalReturnFocus?.focus?.();
}

async function refreshAccounts(selectAll = false) {
  const payload = await api("/accounts");
  state.accounts = payload.accounts;
  const validIds = new Set(state.accounts.map((account) => account.id));
  state.selectedAccountIds = new Set(
    [...state.selectedAccountIds].filter((id) => validIds.has(id)),
  );
  if (selectAll) {
    state.accounts.forEach((account) => state.selectedAccountIds.add(account.id));
  }
}

elements.rangeFilter.addEventListener("click", (event) => {
  const button = event.target.closest("[data-range]");
  if (!button) return;
  state.range = Number.parseInt(button.dataset.range, 10);
  elements.rangeFilter.querySelectorAll("button").forEach((item) => {
    const active = item === button;
    item.classList.toggle("active", active);
    item.setAttribute("aria-pressed", String(active));
  });
  void loadDashboard();
});

elements.platformFilter.addEventListener("click", (event) => {
  const button = event.target.closest("[data-platform]");
  if (!button) return;
  state.platform = button.dataset.platform;
  elements.platformFilter.querySelectorAll("button").forEach((item) => {
    const active = item === button;
    item.classList.toggle("active", active);
    item.setAttribute("aria-pressed", String(active));
  });
  void loadDashboard();
});

elements.accountFilter.addEventListener("click", (event) => {
  const button = event.target.closest("[data-account-filter]");
  if (!button) return;
  const id = button.dataset.accountFilter;
  if (state.selectedAccountIds.has(id)) state.selectedAccountIds.delete(id);
  else state.selectedAccountIds.add(id);
  void loadDashboard().then(() => {
    const restoredButton = [...elements.accountFilter.querySelectorAll("[data-account-filter]")]
      .find((candidate) => candidate.dataset.accountFilter === id);
    restoredButton?.focus();
  });
});

elements.resetFiltersButton.addEventListener("click", () => {
  state.platform = "all";
  elements.platformFilter.querySelectorAll("button").forEach((item) => {
    const active = item.dataset.platform === "all";
    item.classList.toggle("active", active);
    item.setAttribute("aria-pressed", String(active));
  });
  state.accounts.forEach((account) => state.selectedAccountIds.add(account.id));
  void loadDashboard();
});

elements.toggleTableButton.addEventListener("click", () => {
  const show = elements.dataTableCard.hidden;
  elements.dataTableCard.hidden = !show;
  elements.toggleTableButton.setAttribute("aria-expanded", String(show));
  elements.toggleTableButton.textContent = show ? "Masquer le tableau" : "Afficher le tableau";
});

elements.connectButton.addEventListener("click", (event) => openModal(event.currentTarget));
elements.manageAccountsButton.addEventListener("click", (event) => openModal(event.currentTarget));
elements.modalCloseButton.addEventListener("click", closeModal);
elements.connectModal.addEventListener("mousedown", (event) => {
  if (event.target === elements.connectModal) closeModal();
});

elements.providerPicker.addEventListener("click", (event) => {
  const button = event.target.closest("[data-provider]");
  if (!button || state.oauthPending) return;
  state.provider = button.dataset.provider;
  elements.providerPicker.querySelectorAll("button").forEach((item) => {
    const active = item === button;
    item.classList.toggle("active", active);
    item.setAttribute("aria-pressed", String(active));
  });
  renderProviderConfiguration();
});

elements.liveConnectButton.addEventListener("click", async () => {
  const provider = state.provider;
  const requestId = ++state.oauthRequestId;
  state.oauthPending = true;
  elements.oauthError.hidden = true;
  elements.providerPicker.querySelectorAll("button").forEach((button) => { button.disabled = true; });
  renderProviderConfiguration();
  try {
    const payload = await api(`/oauth/${provider}/start`, {
      method: "POST",
      body: "{}",
    });
    if (requestId !== state.oauthRequestId || elements.connectModal.hidden) return;
    const authorizeUrl = new URL(payload.authorizeUrl);
    const expectedHost = state.config?.providers?.[provider]?.authorizeHost;
    if (authorizeUrl.protocol !== "https:" || !expectedHost || authorizeUrl.hostname !== expectedHost) {
      throw new Error("URL d’autorisation fournisseur refusée.");
    }
    window.top.location.assign(authorizeUrl.href);
  } catch (error) {
    if (requestId !== state.oauthRequestId || elements.connectModal.hidden) return;
    elements.oauthError.textContent = error.message;
    elements.oauthError.hidden = false;
  } finally {
    if (requestId === state.oauthRequestId) {
      state.oauthPending = false;
      elements.providerPicker.querySelectorAll("button").forEach((button) => { button.disabled = false; });
      renderProviderConfiguration();
    }
  }
});

elements.demoAccountForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  elements.demoSubmitButton.disabled = true;
  elements.formError.hidden = true;
  try {
    await api("/accounts/demo", {
      method: "POST",
      body: JSON.stringify({
        platform: state.provider,
        displayName: elements.displayNameInput.value,
        handle: elements.handleInput.value,
      }),
    });
    await refreshAccounts(true);
    closeModal();
    await loadDashboard();
    showToast(`Compte test ${accountPlatformLabel(state.provider)} connecté.`);
  } catch (error) {
    elements.formError.textContent = error.message;
    elements.formError.hidden = false;
  } finally {
    elements.demoSubmitButton.disabled = false;
  }
});

elements.accountsGrid.addEventListener("click", async (event) => {
  const button = event.target.closest("[data-delete-account]");
  if (!button) return;
  const account = state.accounts.find((item) => item.id === button.dataset.deleteAccount);
  if (!account || !window.confirm(`Déconnecter @${account.handle} de ce dashboard ?`)) return;
  button.disabled = true;
  try {
    await api(`/accounts/${account.id}`, { method: "DELETE" });
    state.selectedAccountIds.delete(account.id);
    await refreshAccounts(false);
    await loadDashboard();
    showToast(`@${account.handle} a été déconnecté.`);
  } catch (error) {
    showToast(error.message, true);
    button.disabled = false;
  }
});

elements.syncButton.addEventListener("click", async () => {
  if (state.syncPending) return;
  state.syncPending = true;
  renderAll();
  setServiceStatus("Synchronisation…", "");
  try {
    const result = await api("/sync", { method: "POST", body: "{}" });
    await refreshAccounts(false);
    await loadDashboard();
    if (result.partial) {
      setServiceStatus("Synchronisation partielle", "error");
      showToast(`${result.failedAccounts} compte officiel n’a pas pu être actualisé. Les dernières données valides sont conservées.`, true);
    } else {
      const status = selectedDataStatus();
      setServiceStatus(status.label, status.status);
      showToast("Données quotidiennes actualisées.");
    }
  } catch (error) {
    setServiceStatus("Échec de synchronisation", "error");
    showToast(error.message, true);
  } finally {
    state.syncPending = false;
    renderAll();
  }
});

elements.modeDetailsButton.addEventListener("click", () => {
  showToast("Les comptes TEST restent séparés des connexions officielles. Instagram exige un compte professionnel ; TikTok utilise Login Kit et le scope video.list pour lire les compteurs par vidéo.");
});

elements.canvas.addEventListener("pointermove", (event) => {
  if (!state.chartTooltipPinned) showChartTooltip(event);
});
elements.canvas.addEventListener("pointerdown", (event) => {
  state.chartTooltipPinned = true;
  showChartTooltip(event);
});
elements.canvas.addEventListener("pointerleave", () => {
  if (!state.chartTooltipPinned) elements.chartTooltip.hidden = true;
});
elements.canvas.addEventListener("focus", () => {
  if (state.chartGeometry) showChartTooltipAtIndex(state.chartGeometry.dates.length - 1);
});
elements.canvas.addEventListener("blur", () => {
  state.chartTooltipPinned = false;
  elements.chartTooltip.hidden = true;
});
elements.canvas.addEventListener("keydown", (event) => {
  if (!state.chartGeometry || !["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
  event.preventDefault();
  if (event.key === "Home") state.chartFocusIndex = 0;
  else if (event.key === "End") state.chartFocusIndex = state.chartGeometry.dates.length - 1;
  else state.chartFocusIndex += event.key === "ArrowLeft" ? -1 : 1;
  showChartTooltipAtIndex(state.chartFocusIndex);
});
new ResizeObserver(() => window.requestAnimationFrame(drawChart)).observe(elements.chartWrap);

document.addEventListener("pointerdown", (event) => {
  if (event.target === elements.canvas) return;
  state.chartTooltipPinned = false;
  elements.chartTooltip.hidden = true;
});

document.addEventListener("keydown", (event) => {
  if (event.key === "Tab" && !elements.connectModal.hidden) {
    const focusable = [...elements.connectModal.querySelectorAll(
      'button:not([disabled]):not([hidden]), input:not([disabled]):not([hidden])',
    )].filter((element) => element.offsetParent !== null);
    if (focusable.length) {
      const first = focusable[0];
      const last = focusable.at(-1);
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }
    return;
  }
  if (event.key !== "Escape") return;
  if (!elements.connectModal.hidden) {
    closeModal();
  } else if (window.parent !== window) {
    window.parent.postMessage({ type: "switch-social-close" }, window.location.origin);
  }
});

const query = new URLSearchParams(window.location.search);
if (query.get("connected")) showToast("Compte connecté. Première synchronisation lancée.");
if (query.get("connect_error")) showToast("La connexion au fournisseur n’a pas abouti.", true);

void bootstrap();
