import { invoke } from "./platform";
import "./freebuff-cloud.css";

type FreebuffCloudStatus = {
  connected: boolean;
  email: string | null;
  name: string | null;
  githubLogin: string | null;
  message: string | null;
};

type FreebuffCloudConnectResult = {
  connected: boolean;
  email: string | null;
  name: string | null;
  githubLogin: string | null;
  message: string;
};

type ProjectRecord = {
  id: string | null;
  semanticIdentifier: string | null;
  name: string | null;
  repoFullName: string | null;
  status: string | null;
  previewUrl: string | null;
  createdAt: number | null;
  updatedAt: number | null;
  messageId: string | null;
  runId: string | null;
  raw: Record<string, unknown>;
};

type FreebuffCloudPanelBindings = {
  rerender: () => void;
  renderIcons: (root?: ParentNode) => void;
};

let status: FreebuffCloudStatus | null = null;
let projects: ProjectRecord[] | null = null;
let repos: string[] = [];
let loading = false;
let loadError = "";
let toast = "";
let toastTimer: number | null = null;
let connectOpen = false;
let connectCookie = "";
let connecting = false;
let connectError = "";
let createOpen = false;
let createMode: "blank" | "repo" = "blank";
let createName = "";
let createRepo = "";
let creating = false;
let createError = "";
let streamingProjectId: string | null = null;

const FREEBUFF_CLOUD_PROJECT_BASE = "https://freebuff.com/cloud/project";

const escapeHtml = (value: unknown) =>
  String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");

const safeHttpsUrl = (value: unknown, fallbackHost?: string) => {
  const raw = String(value ?? "").trim();
  if (!raw) return "";
  try {
    const url = new URL(raw);
    if (url.protocol !== "https:" && url.protocol !== "http:") return "";
    return url.toString();
  } catch {
    if (fallbackHost && /^[a-z0-9.-]+$/i.test(raw)) return `https://${raw}`;
    return "";
  }
};

const showToast = (message: string, rerender: () => void) => {
  toast = message;
  if (toastTimer) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toast = "";
    rerender();
  }, 3200);
  rerender();
};

const pick = <T,>(object: Record<string, unknown> | undefined | null, keys: string[]): T | null => {
  if (!object) return null;
  for (const key of keys) {
    const value = object[key];
    if (value !== undefined && value !== null) {
      if (typeof value === "string") {
        const trimmed = value.trim();
        if (trimmed) return trimmed as T;
      } else {
        return value as T;
      }
    }
  }
  return null;
};

const numberField = (object: Record<string, unknown> | undefined | null, keys: string[]): number | null => {
  const value: unknown = pick(object, keys);
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim() && !Number.isNaN(Number(value))) {
    return Number(value);
  }
  return null;
};

const normalizeProject = (value: unknown): ProjectRecord | null => {
  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  const preview = pick<string>(
    record,
    ["previewUrl", "preview_url", "previewURL", "preview"],
  );
  return {
    id: pick<string>(record, ["_id", "id", "projectId", "project_id"]),
    semanticIdentifier: pick<string>(record, ["semanticIdentifier", "semantic_identifier", "slug"]),
    name: pick<string>(record, ["name", "title"]),
    repoFullName: pick<string>(record, ["repoFullName", "repo_full_name", "repoName", "repo_url", "repoUrl"]),
    status: pick<string>(record, ["status", "buildStatus", "state"]),
    previewUrl: safeHttpsUrl(preview),
    createdAt: numberField(record, ["createdAt", "created_at", "created"]),
    updatedAt: numberField(record, ["updatedAt", "updated_at", "updated"]),
    messageId: pick<string>(record, ["messageId", "message_id", "agentRunId", "runMessageId"]),
    runId: pick<string>(record, ["runId", "run_id", "agentRun"],
    ),
    raw: record,
  };
};

const normalizeRepos = (value: unknown): string[] => {
  if (Array.isArray(value)) {
    const items = value
      .map((item) => {
        if (typeof item === "string") return item.trim();
        if (item && typeof item === "object") {
          return pick<string>(
            item as Record<string, unknown>,
            ["full_name", "fullName", "repo", "name", "repo_full_name"],
          );
        }
        return null;
      })
      .filter((item): item is string => Boolean(item));
    return [...new Set(items)];
  }
  if (value && typeof value === "object") {
    const candidates = [
      value,
      (value as Record<string, unknown>).repos,
      (value as Record<string, unknown>).repositories,
      (value as Record<string, unknown>).data,
    ];
    for (const candidate of candidates) {
      if (candidate && typeof candidate === "object") {
        const nested = normalizeRepos(
          (candidate as Record<string, unknown>).items ?? candidate,
        );
        if (nested.length) return nested;
      }
    }
  }
  return [];
};

const statusLabel = (statusValue: string | null) => {
  switch ((statusValue ?? "").toLowerCase()) {
    case "ready":
    case "running":
    case "active":
    case "live":
      return "En cours";
    case "completed":
    case "done":
    case "succeeded":
      return "Terminé";
    case "failed":
    case "error":
      return "Échec";
    case "provisioning":
    case "starting":
    case "building":
      return "Démarrage";
    case "deleted":
    case "archived":
      return "Archivé";
    default:
      return statusValue || "—";
  }
};

const statusTone = (statusValue: string | null) => {
  switch ((statusValue ?? "").toLowerCase()) {
    case "ready":
    case "running":
    case "active":
    case "live":
    case "provisioning":
    case "starting":
    case "building":
      return "active";
    case "completed":
    case "done":
    case "succeeded":
      return "done";
    case "failed":
    case "error":
      return "error";
    case "deleted":
    case "archived":
      return "muted";
    default:
      return "muted";
  }
};

const formatDate = (value: number | null) => {
  if (!value) return "";
  try {
    return new Intl.DateTimeFormat("fr-FR", {
      day: "2-digit",
      month: "short",
      hour: "2-digit",
      minute: "2-digit",
    }).format(new Date(value));
  } catch {
    return "";
  }
};

export async function refreshFreebuffCloudPanel(rerender: () => void, silent = false) {
  if (loading && silent) return;
  loading = true;
  loadError = "";
  if (!silent) rerender();
  try {
    const [statusResult, projectsResult] = await Promise.all([
      invoke<FreebuffCloudStatus>("freebuff_cloud_status"),
      invoke<unknown>("freebuff_cloud_projects").catch(() => null),
    ]);
    status = statusResult;
    projects = Array.isArray(projectsResult)
      ? projectsResult.map(normalizeProject).filter((item): item is ProjectRecord => Boolean(item))
      : null;
    loadError = "";
  } catch (error) {
    loadError = String(error instanceof Error ? error.message : error);
    status = null;
    projects = null;
  } finally {
    loading = false;
    rerender();
  }
}

const resetConnectDraft = () => {
  connectCookie = "";
  connectError = "";
};

const resetCreateDraft = () => {
  createName = "";
  createRepo = "";
  createError = "";
  createMode = "blank";
};

const renderLoading = () => `
  <section class="freebuff-cloud-panel" aria-busy="true">
    <div class="freebuff-cloud-loading"><i data-lucide="loader-circle" class="is-spinning"></i><span>Connexion à Freebuff Cloud…</span></div>
  </section>`;

const renderLoadFailure = () => `
  <section class="freebuff-cloud-panel">
    <div class="freebuff-cloud-error" role="alert">
      <i data-lucide="circle-alert"></i>
      <div><strong>Freebuff Cloud indisponible</strong><small>${escapeHtml(loadError)}</small></div>
      <button type="button" class="freebuff-cloud-button secondary" data-refresh-freebuff-cloud><i data-lucide="refresh-cw"></i><span>Réessayer</span></button>
    </div>
  </section>`;

const renderConnectForm = () => `
  <section class="freebuff-cloud-connect">
    <div class="freebuff-cloud-connect-head">
      <span class="freebuff-cloud-connect-icon"><i data-lucide="github"></i></span>
      <div>
        <strong>Connecter ton compte Freebuff Cloud</strong>
        <small>L'agent cloud de <b>freebuff.com</b> : sandbox cloud + preview live pour n'importe quel repo GitHub.</small>
      </div>
    </div>
    <ol class="freebuff-cloud-steps">
      <li><span>1</span><div><strong>Connecte-toi sur freebuff.com</strong><small>Ouvre <a href="https://freebuff.com/cloud" target="_blank" rel="noreferrer">freebuff.com/cloud</a> et signe-toi avec GitHub (bouton « Sign in with GitHub »).</small></div></li>
      <li><span>2</span><div><strong>Copie le cookie de session</strong><small>Dans ton navigateur (F12 → Application → Cookies → <code>freebuff.com</code>), copie la valeur de <code>__Secure-next-auth.session-token</code>.</small></div></li>
      <li><span>3</span><div><strong>Colle-le ici</strong><small>Switch le garde sur le serveur et ne l'affiche plus jamais ensuite.</small></div></li>
    </ol>
    <form class="freebuff-cloud-connect-form" data-freebuff-cloud-connect-form>
      <label for="freebuffCloudSessionCookie">Cookie de session freebuff.com</label>
      <textarea id="freebuffCloudSessionCookie" data-freebuff-cloud-cookie-input placeholder="eyJhbGciOiJIUzI1NiIs…" rows="3" spellcheck="false" autocomplete="off" ${connecting ? "disabled" : ""}>${escapeHtml(connectCookie)}</textarea>
      ${connectError ? `<div class="freebuff-cloud-form-error" role="alert"><i data-lucide="circle-alert"></i><span>${escapeHtml(connectError)}</span></div>` : ""}
      <div class="freebuff-cloud-connect-actions">
        <button type="submit" class="freebuff-cloud-button primary" ${connecting || !connectCookie.trim() ? "disabled" : ""}>
          <i data-lucide="${connecting ? "loader-circle" : "plug-zap"}" class="${connecting ? "is-spinning" : ""}"></i><span>${connecting ? "Connexion…" : "Connecter le compte"}</span>
        </button>
        <button type="button" class="freebuff-cloud-button secondary" data-close-freebuff-cloud-connect ${connecting ? "disabled" : ""}>Annuler</button>
      </div>
    </form>
  </section>`;

const renderCreateDialog = () => `
  <div class="freebuff-cloud-modal-backdrop">
    <section class="freebuff-cloud-modal" role="dialog" aria-modal="true" aria-labelledby="freebuffCloudCreateTitle">
      <h2 id="freebuffCloudCreateTitle">Nouveau projet cloud</h2>
      <div class="freebuff-cloud-mode-tabs" role="tablist">
        <button type="button" class="${createMode === "blank" ? "is-active" : ""}" data-freebuff-cloud-mode="blank" ${creating ? "disabled" : ""}><i data-lucide="file-plus-2"></i><span>Projet vierge</span></button>
        <button type="button" class="${createMode === "repo" ? "is-active" : ""}" data-freebuff-cloud-mode="repo" ${creating ? "disabled" : ""}><i data-lucide="github"></i><span>Depuis un repo GitHub</span></button>
      </div>
      <form class="freebuff-cloud-create-form" data-freebuff-cloud-create-form>
        ${createMode === "blank"
          ? `<label for="freebuffCloudProjectName">Nom du projet</label>
             <input id="freebuffCloudProjectName" data-freebuff-cloud-create-name type="text" placeholder="Mon projet cloud" maxlength="80" autocomplete="off" ${creating ? "disabled" : ""} value="${escapeHtml(createName)}" required />`
          : `<label for="freebuffCloudRepo">Repo GitHub</label>
             <select id="freebuffCloudRepo" data-freebuff-cloud-create-repo ${creating ? "disabled" : ""} required>
               <option value="" disabled ${!createRepo ? "selected" : ""}>Choisis un repo connectable…</option>
               ${repos.map((repo) => `<option value="${escapeHtml(repo)}" ${repo === createRepo ? "selected" : ""}>${escapeHtml(repo)}</option>`).join("")}
             </select>
             ${repos.length === 0 ? `<small class="freebuff-cloud-hint">Aucun repo connectable trouvé — vérifie sur freebuff.com/cloud que ton compte GitHub est connecté.</small>` : ""}`}
        ${createError ? `<div class="freebuff-cloud-form-error" role="alert"><i data-lucide="circle-alert"></i><span>${escapeHtml(createError)}</span></div>` : ""}
        <div class="freebuff-cloud-modal-actions">
          <button type="button" class="freebuff-cloud-button secondary" data-close-freebuff-cloud-create ${creating ? "disabled" : ""}>Annuler</button>
          <button type="submit" class="freebuff-cloud-button primary" ${creating ? "disabled" : ""}>
            <i data-lucide="${creating ? "loader-circle" : "sparkles"}" class="${creating ? "is-spinning" : ""}"></i><span>${creating ? "Création…" : "Créer le projet"}</span>
          </button>
        </div>
      </form>
    </section>
  </div>`;

const renderProjectCard = (project: ProjectRecord) => `
  <article class="freebuff-cloud-project" data-freebuff-cloud-project>
    <div class="freebuff-cloud-project-main">
      <span class="freebuff-cloud-project-icon"><i data-lucide="${project.repoFullName ? "github" : "layers"}"></i></span>
      <div class="freebuff-cloud-project-copy">
        <strong>${escapeHtml(project.name || project.repoFullName || project.semanticIdentifier || "Projet sans nom")}</strong>
        <small>${escapeHtml(project.repoFullName || "Projet vierge")}${project.updatedAt ? ` · modifié ${escapeHtml(formatDate(project.updatedAt))}` : ""}</small>
      </div>
      <span class="freebuff-cloud-pill is-${statusTone(project.status)}"><i data-lucide="circle-dot"></i><span>${escapeHtml(statusLabel(project.status))}</span></span>
    </div>
    <div class="freebuff-cloud-project-actions">
      ${project.semanticIdentifier
        ? `<a class="freebuff-cloud-button primary" href="${escapeHtml(`${FREEBUFF_CLOUD_PROJECT_BASE}/${encodeURIComponent(project.semanticIdentifier)}`)}" target="_blank" rel="noreferrer"><i data-lucide="external-link"></i><span>Ouvrir le projet</span></a>`
        : ""}
      ${project.previewUrl
        ? `<a class="freebuff-cloud-button secondary" href="${escapeHtml(project.previewUrl)}" target="_blank" rel="noreferrer"><i data-lucide="external-link"></i><span>Preview</span></a>`
        : ""}
      ${project.messageId
        ? `<button type="button" class="freebuff-cloud-button secondary" data-freebuff-cloud-stream="${escapeHtml(project.id || project.semanticIdentifier || "")}" data-message-id="${escapeHtml(project.messageId)}" data-run-id="${escapeHtml(project.runId || "")}"><i data-lucide="radio"></i><span>Session</span></button>`
        : ""}
      <button type="button" class="freebuff-cloud-button danger" data-freebuff-cloud-delete="${escapeHtml(project.id || "")}" title="Supprimer le projet"><i data-lucide="trash-2"></i><span>Supprimer</span></button>
    </div>
  </article>`;

const renderSessionStream = (project: ProjectRecord) => `
  <section class="freebuff-cloud-session" data-freebuff-cloud-session>
    <header class="freebuff-cloud-session-head">
      <div><i data-lucide="radio"></i><strong>Session en direct</strong><small>${escapeHtml(project.name || project.semanticIdentifier || "")}</small></div>
      <button type="button" class="freebuff-cloud-button secondary" data-close-freebuff-cloud-session><i data-lucide="x"></i><span>Fermer</span></button>
    </header>
    <div class="freebuff-cloud-session-output" data-freebuff-cloud-session-output aria-live="polite">
      <div class="freebuff-cloud-session-waiting"><i data-lucide="loader-circle" class="is-spinning"></i><span>Connexion au flux de l'agent…</span></div>
    </div>
  </section>`;

export function renderFreebuffCloudPanel(): string {
  if (!status && !loadError && loading) return renderLoading();
  if (!status && loadError) return renderLoadFailure();

  const connected = status?.connected === true;
  const projectList = Array.isArray(projects) ? projects : [];
  const streamingProject = streamingProjectId
    ? projectList.find((project) => project.id === streamingProjectId || project.semanticIdentifier === streamingProjectId)
    : null;

  return `<section class="freebuff-cloud-panel" aria-labelledby="freebuffCloudTitle">
    <header class="freebuff-cloud-hero">
      <div class="freebuff-cloud-hero-copy">
        <span class="freebuff-cloud-mark"><i data-lucide="cloud"></i></span>
        <div>
          <h1 id="freebuffCloudTitle">Freebuff Cloud</h1>
          <p>Sandbox cloud gratuit pour n'importe quel repo GitHub — l'agent travaille dans le cloud, la preview est en direct.</p>
        </div>
      </div>
      <span class="freebuff-cloud-pill is-${connected ? "done" : "muted"}"><i data-lucide="${connected ? "circle-check" : "circle-dot"}"></i><span>${connected ? "Connecté" : "Non connecté"}</span></span>
      <button type="button" class="freebuff-cloud-button secondary" data-refresh-freebuff-cloud ${loading ? "disabled" : ""}><i data-lucide="refresh-cw" class="${loading ? "is-spinning" : ""}"></i><span>Actualiser</span></button>
    </header>

    ${connected
      ? `<div class="freebuff-cloud-account">
          <span class="freebuff-cloud-avatar">${escapeHtml((status?.name || status?.email || "C")[0]?.toUpperCase() ?? "C")}</span>
          <span><strong>${escapeHtml(status?.name || "Compte Freebuff")}</strong><small>${escapeHtml(status?.email || "")}${status?.githubLogin ? ` · @${escapeHtml(status.githubLogin)}` : ""}</small></span>
          <button type="button" class="freebuff-cloud-button ghost danger-text" data-freebuff-cloud-disconnect><i data-lucide="log-out"></i><span>Déconnecter</span></button>
        </div>`
      : renderConnectForm()}

    ${connected ? `
      <div class="freebuff-cloud-toolbar">
        <div class="freebuff-cloud-toolbar-title"><i data-lucide="folders"></i><strong>Projets cloud</strong><small>${projectList.length} projet${projectList.length > 1 ? "s" : ""}</small></div>
        <button type="button" class="freebuff-cloud-button primary" data-open-freebuff-cloud-create><i data-lucide="plus"></i><span>Nouveau projet</span></button>
      </div>

      ${projectList.length
        ? `<div class="freebuff-cloud-projects">${projectList.map(renderProjectCard).join("")}</div>`
        : `<div class="freebuff-cloud-empty"><span><i data-lucide="cloud-off"></i></span><strong>Aucun projet cloud</strong><small>Crée un projet pour démarrer un sandbox.</small></div>`}
    ` : ""}

    ${streamingProject ? renderSessionStream(streamingProject) : ""}
    ${createOpen ? renderCreateDialog() : ""}
    ${toast ? `<div class="freebuff-cloud-toast" role="status"><i data-lucide="circle-check"></i><span>${escapeHtml(toast)}</span></div>` : ""}
  </section>`;
}

const openSessionStream = (project: ProjectRecord, rerender: () => void) => {
  streamingProjectId = project.id || project.semanticIdentifier;
  rerender();
  const root = document.querySelector<HTMLElement>(".freebuff-cloud-session");
  const output = root?.querySelector<HTMLElement>("[data-freebuff-cloud-session-output]");
  if (!root || !output) return;

  const params = new URLSearchParams({ messageId: project.messageId || "" });
  if (project.runId) params.set("runId", project.runId);
  const controller = new AbortController();
  const close = () => controller.abort();

  const remember = () => {
    const closeButton = root.querySelector<HTMLButtonElement>("[data-close-freebuff-cloud-session]");
    closeButton?.addEventListener("click", () => {
      close();
      streamingProjectId = null;
      rerender();
    });
  };
  remember();

  let buffer = "";
  const appendEvent = (data: string) => {
    try {
      const parsed = JSON.parse(data) as Record<string, unknown>;
      const item = (parsed.item ?? parsed) as Record<string, unknown>;
      const content = typeof item.content === "string"
        ? item.content
        : typeof parsed.content === "string"
          ? parsed.content
          : "";
      if (content) {
        const line = document.createElement("div");
        line.className = "freebuff-cloud-session-line";
        line.textContent = content;
        output.appendChild(line);
        output.scrollTop = output.scrollHeight;
      }
      const statusValue = typeof item.status === "string"
        ? item.status
        : typeof parsed.status === "string"
          ? parsed.status
          : "";
      if (statusValue) {
        const pill = document.createElement("div");
        pill.className = `freebuff-cloud-pill is-${statusTone(statusValue)}`;
        pill.innerHTML = `<i data-lucide="circle-dot"></i><span>${escapeHtml(statusLabel(statusValue))}</span>`;
        output.appendChild(pill);
      }
      const preview = typeof item.description === "string"
        ? item.description
        : typeof parsed.description === "string"
          ? parsed.description
          : "";
      if (preview && /^https?:\/\//.test(preview)) {
        const link = document.createElement("a");
        link.className = "freebuff-cloud-button secondary";
        link.href = preview;
        link.target = "_blank";
        link.rel = "noreferrer";
        link.innerHTML = `<i data-lucide="external-link"></i><span>Ouvrir la preview</span>`;
        output.appendChild(link);
      }
    } catch {
      const line = document.createElement("div");
      line.className = "freebuff-cloud-session-line muted";
      line.textContent = data;
      output.appendChild(line);
    }
  };

  const feed = async () => {
    output.innerHTML = `<div class="freebuff-cloud-session-waiting"><i data-lucide="loader-circle" class="is-spinning"></i><span>Connexion au flux de l'agent…</span></div>`;
    try {
      const response = await fetch(`/api/freebuff-cloud/stream?${params.toString()}`, {
        credentials: "same-origin",
        signal: controller.signal,
      });
      if (!response.ok) {
        const text = await response.text().catch(() => "");
        let message = `Flux indisponible (HTTP ${response.status})`;
        try {
          const parsed = JSON.parse(text) as { error?: { message?: string } };
          if (parsed.error?.message) message = parsed.error.message;
        } catch {
          /* corps non JSON */
        }
        output.innerHTML = `<div class="freebuff-cloud-session-error"><i data-lucide="circle-alert"></i><span>${escapeHtml(message)}</span></div>`;
        return;
      }
      if (!response.body) return;
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      output.innerHTML = "";
      // eslint-disable-next-line no-constant-condition
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split("\n");
        buffer = lines.pop() ?? "";
        for (const line of lines) {
          const trimmed = line.trim();
          if (trimmed.startsWith("data:")) {
            appendEvent(trimmed.slice(5).trim());
          }
        }
      }
    } catch (error) {
      if ((error as Error)?.name === "AbortError") return;
      const line = document.createElement("div");
      line.className = "freebuff-cloud-session-error";
      line.innerHTML = `<i data-lucide="circle-alert"></i><span>${escapeHtml(String(error instanceof Error ? error.message : error))}</span>`;
      output.appendChild(line);
    }
  };

  void feed();
};

export function bindFreebuffCloudPanel({ rerender, renderIcons }: FreebuffCloudPanelBindings) {
  const root = document.querySelector<HTMLElement>(".freebuff-cloud-panel");
  if (!root) return;

  if (!status && !loadError && !loading) void refreshFreebuffCloudPanel(rerender, true);

  root.querySelectorAll<HTMLButtonElement>("[data-refresh-freebuff-cloud]").forEach((button) => {
    button.addEventListener("click", () => void refreshFreebuffCloudPanel(rerender));
  });

  root.querySelectorAll<HTMLButtonElement>("[data-freebuff-cloud-disconnect]").forEach((button) => {
    button.addEventListener("click", async () => {
      if (!window.confirm("Déconnecter le compte Freebuff Cloud ? Le cookie de session sera supprimé du serveur.")) return;
      button.disabled = true;
      try {
        await invoke("freebuff_cloud_disconnect");
        status = null;
        projects = null;
        showToast("Compte Freebuff Cloud déconnecté", rerender);
      } catch (error) {
        showToast(String(error instanceof Error ? error.message : error), rerender);
      } finally {
        button.disabled = false;
      }
    });
  });

  root.querySelectorAll<HTMLButtonElement>("[data-close-freebuff-cloud-connect]").forEach((button) => {
    button.addEventListener("click", () => {
      connectOpen = false;
      resetConnectDraft();
      rerender();
    });
  });

  const connectForm = root.querySelector<HTMLFormElement>("[data-freebuff-cloud-connect-form]");
  if (connectForm) {
    const input = connectForm.querySelector<HTMLTextAreaElement>("[data-freebuff-cloud-cookie-input]");
    input?.addEventListener("input", () => {
      connectCookie = input.value;
      rerender();
    });
    connectForm.addEventListener("submit", async (event) => {
      event.preventDefault();
      if (connecting || !connectCookie.trim()) return;
      connecting = true;
      connectError = "";
      rerender();
      try {
        const result = await invoke<FreebuffCloudConnectResult>("freebuff_cloud_connect", {
          sessionCookie: connectCookie.trim(),
        });
        status = { connected: true, email: result.email, name: result.name, githubLogin: result.githubLogin, message: result.message };
        connectOpen = false;
        resetConnectDraft();
        await refreshFreebuffCloudPanel(rerender, true);
        showToast(result.message || "Compte connecté", rerender);
      } catch (error) {
        connectError = String(error instanceof Error ? error.message : error);
        connecting = false;
        rerender();
      }
    });
  }

  root.querySelectorAll<HTMLButtonElement>("[data-open-freebuff-cloud-create]").forEach((button) => {
    button.addEventListener("click", async () => {
      createOpen = true;
      resetCreateDraft();
      repos = [];
      rerender();
      try {
        const value = await invoke<unknown>("freebuff_cloud_repos");
        repos = normalizeRepos(value);
      } catch {
        repos = [];
      }
      rerender();
    });
  });

  root.querySelectorAll<HTMLButtonElement>("[data-close-freebuff-cloud-create]").forEach((button) => {
    button.addEventListener("click", () => {
      if (creating) return;
      createOpen = false;
      resetCreateDraft();
      rerender();
    });
  });

  root.querySelectorAll<HTMLButtonElement>("[data-freebuff-cloud-mode]").forEach((button) => {
    button.addEventListener("click", () => {
      if (creating) return;
      createMode = button.dataset.freebuffCloudMode === "repo" ? "repo" : "blank";
      createError = "";
      rerender();
    });
  });

  const createForm = root.querySelector<HTMLFormElement>("[data-freebuff-cloud-create-form]");
  if (createForm) {
    const nameInput = createForm.querySelector<HTMLInputElement>("[data-freebuff-cloud-create-name]");
    nameInput?.addEventListener("input", () => {
      createName = nameInput.value;
    });
    const repoSelect = createForm.querySelector<HTMLSelectElement>("[data-freebuff-cloud-create-repo]");
    repoSelect?.addEventListener("change", () => {
      createRepo = repoSelect.value;
    });
    createForm.addEventListener("submit", async (event) => {
      event.preventDefault();
      if (creating) return;
      if (createMode === "blank" && !createName.trim()) return;
      if (createMode === "repo" && !createRepo) return;
      creating = true;
      createError = "";
      rerender();
      try {
        if (createMode === "blank") {
          await invoke("freebuff_cloud_create_blank", { name: createName.trim() });
        } else {
          await invoke("freebuff_cloud_connect_repo", { repoFullName: createRepo });
        }
        createOpen = false;
        resetCreateDraft();
        await refreshFreebuffCloudPanel(rerender, true);
        showToast("Projet cloud créé", rerender);
      } catch (error) {
        createError = String(error instanceof Error ? error.message : error);
        creating = false;
        rerender();
      }
    });
  }

  root.querySelectorAll<HTMLButtonElement>("[data-freebuff-cloud-delete]").forEach((button) => {
    button.addEventListener("click", async () => {
      const projectId = button.dataset.freebuffCloudDelete;
      const project = projects?.find((item) => item.id === projectId);
      if (!projectId) return;
      if (!window.confirm(`Supprimer définitivement le projet « ${project?.name || projectId} » de Freebuff Cloud ?`)) return;
      button.disabled = true;
      try {
        await invoke("freebuff_cloud_delete_project", { projectId });
        await refreshFreebuffCloudPanel(rerender, true);
        showToast("Projet supprimé", rerender);
      } catch (error) {
        showToast(String(error instanceof Error ? error.message : error), rerender);
      } finally {
        button.disabled = false;
      }
    });
  });

  root.querySelectorAll<HTMLButtonElement>("[data-freebuff-cloud-stream]").forEach((button) => {
    button.addEventListener("click", () => {
      const project = projects?.find((item) => (
        item.id === button.dataset.freebuffCloudStream
        || item.semanticIdentifier === button.dataset.freebuffCloudStream
      ));
      if (project) openSessionStream(project, rerender);
    });
  });

  if (renderIcons) renderIcons(root);
}
