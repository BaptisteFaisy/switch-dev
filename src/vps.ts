import {
  hasRemoteAuth,
  invoke,
  remoteBaseUrl,
  remoteNodesText,
  saveRemoteConfig,
} from "./platform";
import "./vps.css";

export type VpsDeployCapabilities = {
  supported: boolean;
  platform: string;
  powershell: string | null;
  scriptPath: string | null;
  missingCommands: string[];
  message: string;
};

export type VpsDeployJob = {
  id: string;
  nodeId: string;
  nodeLabel: string;
  sshTarget: string;
  localPort: number;
  status: "running" | "succeeded" | "failed";
  message: string;
  log: string;
  createdAt: number;
  finishedAt: number | null;
  exitCode: number | null;
};

type VpsDraft = {
  sshTarget: string;
  identityFile: string;
  knownHostsFile: string;
  sshPort: string;
  remotePort: string;
  localPort: string;
  nodeId: string;
  nodeLabel: string;
  capacity: string;
  seedAccounts: boolean;
  acceptNewHostKey: boolean;
};

const draft: VpsDraft = {
  sshTarget: "",
  identityFile: "",
  knownHostsFile: "",
  sshPort: "22",
  remotePort: "8080",
  localPort: "8081",
  nodeId: "azure-vps",
  nodeLabel: "Azure VPS",
  capacity: "4",
  seedAccounts: true,
  acceptNewHostKey: false,
};

let capabilities: VpsDeployCapabilities | null = null;
let jobs: VpsDeployJob[] = [];
let loading = false;
let submitting = false;
let errorMessage = "";
let active = false;
let showAdvancedOptions = false;
let historyExpanded = false;
let refreshPromise: Promise<void> | null = null;
let pollTimer: number | null = null;

const escapeHtml = (value: unknown): string => String(value ?? "")
  .replaceAll("&", "&amp;")
  .replaceAll("<", "&lt;")
  .replaceAll(">", "&gt;")
  .replaceAll('"', "&quot;")
  .replaceAll("'", "&#39;");

const formatDate = (unix: number): string => new Intl.DateTimeFormat("fr-FR", {
  dateStyle: "short",
  timeStyle: "medium",
}).format(new Date(unix * 1_000));

const jobLabel = (status: VpsDeployJob["status"]): string =>
  status === "running" ? "En cours" : status === "succeeded" ? "Déployé" : "Échec";

const jobIcon = (status: VpsDeployJob["status"]): string =>
  status === "running" ? "loader-circle" : status === "succeeded" ? "check" : "circle-alert";

const input = (
  name: keyof VpsDraft,
  label: string,
  options: { type?: string; placeholder?: string; min?: number; max?: number; wide?: boolean } = {},
): string => `
  <label class="vps-field ${options.wide ? "is-wide" : ""}">
    <span>${escapeHtml(label)}</span>
    <input name="${name}" type="${options.type ?? "text"}" value="${escapeHtml(draft[name])}"
      ${options.placeholder ? `placeholder="${escapeHtml(options.placeholder)}"` : ""}
      ${options.min !== undefined ? `min="${options.min}"` : ""}
      ${options.max !== undefined ? `max="${options.max}"` : ""}
      ${submitting ? "disabled" : ""} />
  </label>`;

const renderJob = (job: VpsDeployJob): string => `
  <article class="vps-job is-${job.status}">
    <header>
      <span class="vps-job-icon"><i data-lucide="${jobIcon(job.status)}"></i></span>
      <span class="vps-job-title"><strong>${escapeHtml(job.nodeLabel)}</strong>
        <small>${escapeHtml(job.sshTarget)} · ${formatDate(job.createdAt)}</small></span>
      <span class="vps-job-status">${jobLabel(job.status)}</span>
    </header>
    <p>${escapeHtml(job.message)}</p>
    ${job.status === "succeeded" ? `<div class="vps-next-step"><span>Connexion SSH</span><code>npm run connect:vps -- -Profile ${escapeHtml(job.nodeId)}</code></div>` : ""}
    <details><summary>Journal technique${job.exitCode === null ? "" : ` · code ${job.exitCode}`}</summary>
      <pre>${escapeHtml(job.log || "En attente de la première sortie…")}</pre></details>
  </article>`;

const inputForm = (): string => `
  <form id="vpsDeployForm" class="vps-card vps-form">
    <div class="vps-card-heading"><div class="vps-card-title"><span><i data-lucide="server"></i></span>
      <div><h2>Connexion SSH Azure</h2><p>Déployer le runtime sur le VPS Azure.</p></div></div>
      <button id="vpsDetailsToggle" type="button" class="vps-details-toggle" aria-expanded="${showAdvancedOptions}" aria-controls="vpsAdvancedSettings">
        <i data-lucide="${showAdvancedOptions ? "chevron-up" : "sliders-horizontal"}"></i>
        <span>${showAdvancedOptions ? "Masquer les options" : "Options avancées"}</span>
      </button>
    </div>
    <div class="vps-form-grid vps-form-grid-primary">
      ${input("sshTarget", "Cible SSH", { placeholder: "azure-user@azure-host", wide: true })}
      ${input("identityFile", "Chemin de la clé privée", { placeholder: "C:\\Users\\vous\\.ssh\\id_ed25519", wide: true })}
      ${input("nodeLabel", "Nom affiché", { placeholder: "Azure VPS" })}
    </div>
    <section id="vpsAdvancedSettings" class="vps-advanced-settings" ${showAdvancedOptions ? "" : "hidden"}>
      <div class="vps-form-grid">
        ${input("knownHostsFile", "Fichier known_hosts (optionnel)", { placeholder: "C:\\Users\\vous\\.ssh\\known_hosts", wide: true })}
        ${input("sshPort", "Port SSH", { type: "number", min: 1, max: 65535 })}
        ${input("remotePort", "Port distant", { type: "number", min: 1, max: 65535 })}
        ${input("localPort", "Port local du tunnel", { type: "number", min: 1, max: 65535 })}
        ${input("nodeId", "Identifiant", { placeholder: "azure-vps" })}
        ${input("capacity", "Chats simultanés", { type: "number", min: 1, max: 1024 })}
      </div>
      <div class="vps-options">
        <label><input name="seedAccounts" type="checkbox" ${draft.seedAccounts ? "checked" : ""} ${submitting ? "disabled" : ""} />
          <span><strong>Copier les comptes locaux</strong><small>Amorce les comptes Codex sur Azure.</small></span></label>
        <label><input name="acceptNewHostKey" type="checkbox" ${draft.acceptNewHostKey ? "checked" : ""} ${submitting ? "disabled" : ""} />
          <span><strong>Accepter une nouvelle empreinte SSH</strong><small>À activer après vérification chez l’hébergeur.</small></span></label>
      </div>
      <div class="vps-security-note"><i data-lucide="lock-keyhole"></i><span>La clé reste sur la machine qui lance le déploiement. Ubuntu/Debian, Python 3 et sudo sans mot de passe sont requis.</span></div>
    </section>
    ${showAdvancedOptions ? "" : `<p class="vps-defaults-hint"><i data-lucide="sparkles"></i><span>Les réglages Azure recommandés sont déjà appliqués.</span></p>`}
    <button class="vps-submit" type="submit" ${!capabilities?.supported || submitting || runningJob() ? "disabled" : ""}>
      <i data-lucide="${submitting || runningJob() ? "loader-circle" : "server"}"></i>
      <span>${submitting ? "Démarrage…" : runningJob() ? "Déploiement en cours…" : "Déployer sur Azure"}</span>
    </button>
  </form>`;

const runningJob = (): VpsDeployJob | null => jobs.find((job) => job.status === "running") ?? null;

export const renderVpsPanel = (): string => {
  const historyVisible = historyExpanded;
  const needsAdminToken = !hasRemoteAuth();
  const ready = capabilities?.supported === true;
  return `<section id="vpsPanel" class="vps-panel" aria-labelledby="vpsPanelTitle">
    <header class="vps-heading"><div><h1 id="vpsPanelTitle">Déployer sur Azure</h1><p>Installe le runtime avec Ansible et Docker Compose.</p></div>
      <button id="vpsRefresh" type="button" class="vps-secondary-button" aria-label="Actualiser l’état du serveur" ${loading ? "disabled" : ""}><i data-lucide="refresh-ccw"></i><span>${loading ? "Vérification…" : "Actualiser"}</span></button></header>
    ${ready ? "" : `<div class="vps-capability is-${needsAdminToken ? "blocked" : "checking"}" role="status"><span><i data-lucide="${needsAdminToken ? "lock-keyhole" : "triangle-alert"}"></i></span><div><strong>${needsAdminToken ? "Jeton administrateur requis" : "Action requise"}</strong><p>${escapeHtml(needsAdminToken ? "Déverrouille cette vue avec le jeton administrateur du serveur :8080." : capabilities?.message ?? "Vérification des outils requis.")}</p></div></div>`}
    ${needsAdminToken ? `<form id="vpsAdminUnlock" class="vps-admin-unlock"><div><i data-lucide="lock-keyhole"></i><span><strong>Accès infrastructure</strong><small>Le jeton reste dans la configuration locale.</small></span></div><label><span>Jeton admin</span><input id="vpsAdminToken" type="password" autocomplete="current-password" required /></label><button type="submit"><i data-lucide="lock-open"></i><span>Déverrouiller</span></button></form>` : ""}
    ${errorMessage ? `<div class="vps-alert" role="alert"><i data-lucide="circle-alert"></i><span>${escapeHtml(errorMessage)}</span></div>` : ""}
    <div class="vps-layout">${inputForm()}</div>
    <section class="vps-history ${historyVisible ? "" : "is-collapsed"}" aria-labelledby="vpsHistoryTitle"><div class="vps-history-heading"><div><h2 id="vpsHistoryTitle">Déploiements récents</h2><p>${jobs.length ? `${jobs.length} opération${jobs.length === 1 ? "" : "s"}.` : "Aucun déploiement lancé."}</p></div><div class="vps-history-actions"><span>${jobs.length}</span>${jobs.length ? `<button id="vpsHistoryToggle" type="button" aria-expanded="${historyVisible}"><i data-lucide="${historyVisible ? "chevron-up" : "chevron-down"}"></i><span>${historyVisible ? "Masquer" : "Afficher"}</span></button>` : ""}</div></div>${jobs.length && historyVisible ? `<div id="vpsHistoryContent" class="vps-job-list">${jobs.map(renderJob).join("")}</div>` : ""}</section>
  </section>`;
};

const readDraft = (form: HTMLFormElement): void => {
  const data = new FormData(form);
  draft.sshTarget = String(data.get("sshTarget") ?? "").trim();
  draft.identityFile = String(data.get("identityFile") ?? "").trim();
  draft.knownHostsFile = String(data.get("knownHostsFile") ?? "").trim();
  draft.sshPort = String(data.get("sshPort") ?? "22");
  draft.remotePort = String(data.get("remotePort") ?? "8080");
  draft.localPort = String(data.get("localPort") ?? "8081");
  draft.nodeId = String(data.get("nodeId") ?? "").trim();
  draft.nodeLabel = String(data.get("nodeLabel") ?? "").trim();
  draft.capacity = String(data.get("capacity") ?? "1");
  draft.seedAccounts = data.has("seedAccounts");
  draft.acceptNewHostKey = data.has("acceptNewHostKey");
};

const requestFromDraft = () => ({ ...draft, sshPort: Number(draft.sshPort), remotePort: Number(draft.remotePort), localPort: Number(draft.localPort), capacity: Number(draft.capacity) });

const clearPoll = (): void => { if (pollTimer !== null) window.clearTimeout(pollTimer); pollTimer = null; };
const schedulePoll = (rerender: () => void): void => { clearPoll(); if (active && runningJob()) pollTimer = window.setTimeout(() => void refreshVpsPanel(rerender, true), 1_500); };

export const refreshVpsPanel = (rerender: () => void, silent = false): Promise<void> => {
  if (!hasRemoteAuth()) { loading = false; errorMessage = ""; if (active) rerender(); return Promise.resolve(); }
  if (refreshPromise) return refreshPromise;
  const pending = (async () => {
    if (!silent) loading = true;
    try {
      const [nextCapabilities, nextJobs] = await Promise.all([
        invoke<VpsDeployCapabilities>("vps_deploy_capabilities"),
        invoke<VpsDeployJob[]>("vps_list_deployments"),
      ]);
      capabilities = nextCapabilities;
      jobs = nextJobs;
      if (nextJobs.some((job) => job.status === "running")) historyExpanded = true;
      errorMessage = "";
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      errorMessage = /token admin|authentification|unauthorized/i.test(message) ? "Cet onglet exige le jeton administrateur du serveur :8080." : message;
    } finally { loading = false; refreshPromise = null; if (active) rerender(); schedulePoll(rerender); }
  })();
  refreshPromise = pending;
  return pending;
};

export const activateVpsPanel = (rerender: () => void): void => { active = true; if (!hasRemoteAuth()) { rerender(); return; } void refreshVpsPanel(rerender, capabilities !== null); };
export const deactivateVpsPanel = (): void => { active = false; clearPoll(); };

export const bindVpsPanel = (rerender: () => void): void => {
  document.querySelector<HTMLFormElement>("#vpsAdminUnlock")?.addEventListener("submit", (event) => {
    event.preventDefault(); const input = document.querySelector<HTMLInputElement>("#vpsAdminToken"); const token = input?.value.trim() ?? "";
    if (!token) { input?.setCustomValidity("Jeton administrateur requis"); input?.reportValidity(); return; }
    saveRemoteConfig(remoteBaseUrl(), token, remoteNodesText()); capabilities = null; errorMessage = ""; void refreshVpsPanel(rerender);
  });
  document.querySelector<HTMLButtonElement>("#vpsRefresh")?.addEventListener("click", () => void refreshVpsPanel(rerender));
  document.querySelector<HTMLButtonElement>("#vpsDetailsToggle")?.addEventListener("click", () => { showAdvancedOptions = !showAdvancedOptions; rerender(); });
  document.querySelector<HTMLButtonElement>("#vpsHistoryToggle")?.addEventListener("click", () => { historyExpanded = !historyExpanded; rerender(); });
  const form = document.querySelector<HTMLFormElement>("#vpsDeployForm");
  if (!form) return;
  form.addEventListener("input", () => readDraft(form)); form.addEventListener("change", () => readDraft(form));
  form.addEventListener("submit", (event) => {
    event.preventDefault(); readDraft(form); submitting = true; historyExpanded = true; errorMessage = ""; rerender();
    void (async () => {
      try { const job = await invoke<VpsDeployJob>("vps_start_deployment", { request: requestFromDraft() }); jobs = [job, ...jobs.filter((candidate) => candidate.id !== job.id)]; }
      catch (error) { errorMessage = error instanceof Error ? error.message : String(error); }
      finally { submitting = false; if (active) rerender(); schedulePoll(rerender); }
    })();
  });
};
