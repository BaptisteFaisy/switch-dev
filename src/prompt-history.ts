export type PromptEntry = {
  sessionId: string;
  accountId: string;
  accountLabel: string;
  codexHome: string;
  filePath: string;
  cwd: string | null;
  timestamp: number;
  sessionTitle: string | null;
  text: string;
};

export type PromptHistoryView = {
  generatedAt: number;
  totalPrompts: number;
  returned: number;
  truncated: boolean;
  prompts: PromptEntry[];
};

export type PromptSessionHistory = {
  key: string;
  sessionId: string;
  accountId: string;
  accountLabel: string;
  cwd: string | null;
  sessionTitle: string | null;
  firstTimestamp: number;
  lastTimestamp: number;
  prompts: PromptEntry[];
};

export type PromptHistoryPanelModel = {
  history: PromptHistoryView | null;
  loaded: boolean;
  freebuffAccounts: Array<{ id: string; label: string }>;
  formatTimestamp: (timestamp?: number | null) => string;
  displayProjectDir: (projectDir?: string | null) => string;
};

export type PromptHistoryPanelMountOptions = {
  getModel: () => PromptHistoryPanelModel;
  onRefresh: () => void;
  onOpenDiscussion: (accountId: string, sessionId: string) => void;
  onImportToFreebuff: (accountId: string, sessionId: string, targetAccountId: string) => void;
  renderIcons: (root?: ParentNode) => void;
  rerender: () => void;
  root?: ParentNode;
};

const PROMPT_RENDER_LIMIT = 400;

let promptSearch = "";

const escapeHtml = (value: string): string =>
  value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");

const escapeRegExp = (value: string): string => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

export const highlightPromptMatch = (text: string, query: string): string => {
  const needle = query.trim();
  if (!needle) return escapeHtml(text);
  const re = new RegExp(escapeRegExp(needle), "gi");
  let result = "";
  let last = 0;
  for (const match of text.matchAll(re)) {
    const start = match.index ?? 0;
    if (match[0].length === 0) break;
    result += escapeHtml(text.slice(last, start));
    result += `<mark>${escapeHtml(match[0])}</mark>`;
    last = start + match[0].length;
  }
  return result + escapeHtml(text.slice(last));
};

export const promptSessionsFromHistory = (
  history: PromptHistoryView | null,
): PromptSessionHistory[] => {
  const groups = new Map<string, PromptSessionHistory>();
  for (const entry of history?.prompts ?? []) {
    const key = `${entry.accountId}\u0000${entry.sessionId}`;
    const existing = groups.get(key);
    if (existing) {
      const duplicate = existing.prompts.some(
        (candidate) => candidate.timestamp === entry.timestamp && candidate.text === entry.text,
      );
      if (!duplicate) existing.prompts.push(entry);
      existing.firstTimestamp = Math.min(existing.firstTimestamp, entry.timestamp);
      existing.lastTimestamp = Math.max(existing.lastTimestamp, entry.timestamp);
      existing.cwd ||= entry.cwd;
      existing.sessionTitle ||= entry.sessionTitle;
      continue;
    }
    groups.set(key, {
      key,
      sessionId: entry.sessionId,
      accountId: entry.accountId,
      accountLabel: entry.accountLabel,
      cwd: entry.cwd,
      sessionTitle: entry.sessionTitle,
      firstTimestamp: entry.timestamp,
      lastTimestamp: entry.timestamp,
      prompts: [entry],
    });
  }
  return [...groups.values()]
    .map((session) => ({
      ...session,
      prompts: session.prompts.sort((left, right) => left.timestamp - right.timestamp),
    }))
    .sort((left, right) => right.lastTimestamp - left.lastTimestamp);
};

const promptSessionMatches = (session: PromptSessionHistory, search: string): boolean => {
  const query = search.trim().toLowerCase();
  if (!query) return true;
  return [
    ...session.prompts.map((entry) => entry.text),
    session.cwd ?? "",
    session.accountLabel,
    session.sessionTitle ?? "",
    session.sessionId,
  ].some((field) => field.toLowerCase().includes(query));
};

const renderPromptSession = (
  session: PromptSessionHistory,
  model: PromptHistoryPanelModel,
  search: string,
): string => {
  const meta = [
    `<span><i data-lucide="clock-3"></i>${escapeHtml(model.formatTimestamp(session.lastTimestamp))}</span>`,
    `<span><i data-lucide="users"></i>${escapeHtml(session.accountLabel)}</span>`,
    session.cwd
      ? `<span title="${escapeHtml(session.cwd)}"><i data-lucide="folder-open"></i>${escapeHtml(model.displayProjectDir(session.cwd))}</span>`
      : "",
    `<span><i data-lucide="message-square-text"></i>${session.prompts.length} message(s) envoye(s)</span>`,
  ]
    .filter(Boolean)
    .join("");
  const title = session.sessionTitle?.trim() || session.prompts[0]?.text.trim() || "Session sans titre";
  // Le fournisseur n'est pas toujours présent dans le libellé (ex. compte nommé
  // « principal »). L'historique des prompts provient du catalogue Codex ;
  // l'action doit donc rester visible dès qu'un compte Freebuff cible existe.
  const canImport = (model.freebuffAccounts?.length ?? 0) > 0;
  const messages = session.prompts
    .map(
      (entry, index) => `
        <li>
          <span class="prompt-message-index">${index + 1}</span>
          <span>${highlightPromptMatch(entry.text, search)}</span>
          <time>${escapeHtml(model.formatTimestamp(entry.timestamp))}</time>
        </li>`,
    )
    .join("");
  return `
    <div class="prompt-row prompt-session-row">
      <div class="prompt-main">
        <strong class="prompt-session-title">${highlightPromptMatch(title, search)}</strong>
        <span class="prompt-meta">${meta}</span>
        <ol class="prompt-session-messages">${messages}</ol>
      </div>
      <div class="prompt-actions">
        <button class="tool-button" data-prompt-discussion="${escapeHtml(session.sessionId)}" data-prompt-account="${escapeHtml(session.accountId)}" title="Voir la conversation">
          <i data-lucide="messages-square"></i><span>Conversation</span>
        </button>
        ${canImport ? `<button class="tool-button prompt-import-button" data-prompt-import-open="${escapeHtml(session.sessionId)}" data-prompt-import-source="${escapeHtml(session.accountId)}" title="Transférer cet historique vers un compte Freebuff et reprendre la discussion dans son terminal">
          <i data-lucide="arrow-right-left"></i><span>Vers Freebuff</span>
        </button>` : ""}
      </div>
    </div>
  `;
};

export const renderPromptRows = (
  model: PromptHistoryPanelModel,
  search = promptSearch,
): string => {
  if (!model.loaded) return `<div class="pool-empty">Lecture des demandes Codex…</div>`;
  const sessions = promptSessionsFromHistory(model.history);
  if (sessions.length === 0) return `<div class="pool-empty">Aucune demande trouvee</div>`;
  const matches = sessions.filter((session) => promptSessionMatches(session, search));
  if (matches.length === 0) {
    return `<div class="pool-empty">Aucune demande ne correspond a « ${escapeHtml(search)} »</div>`;
  }
  const shown = matches.slice(0, PROMPT_RENDER_LIMIT);
  const capped = matches.length > shown.length
    ? `<div class="prompt-more">Affichage limite a ${PROMPT_RENDER_LIMIT} sur ${matches.length} resultats — affine la recherche.</div>`
    : "";
  return `${shown.map((session) => renderPromptSession(session, model, search)).join("")}${capped}`;
};

export const renderPromptHistoryPanel = (
  model: PromptHistoryPanelModel,
  search = promptSearch,
): string => {
  const sessions = promptSessionsFromHistory(model.history);
  const sessionCount = sessions.length;
  const messageCount = sessions.reduce((sum, session) => sum + session.prompts.length, 0);
  const truncatedNote = model.history?.truncated
    ? ` · ${model.history.returned} plus recentes indexees`
    : "";
  const countLabel = model.loaded
    ? `${sessionCount} chat(s) / terminal(aux) · ${messageCount} message(s)${truncatedNote}`
    : "Lecture…";
  return `
    <section class="discussions-panel">
      <div class="discussions-head">
        <div>
          <strong>Historique par chat ou terminal</strong>
          <span>${escapeHtml(countLabel)}</span>
        </div>
        <div class="discussions-tools">
          <label class="discussion-search">
            <i data-lucide="search"></i>
            <input id="promptSearch" type="search" placeholder="Rechercher dans les chats, terminaux et messages" value="${escapeHtml(search)}" />
          </label>
          <button id="refreshPromptHistory" class="tool-button" title="Actualiser">
            <i data-lucide="refresh-ccw"></i><span>Actualiser</span>
          </button>
        </div>
      </div>
      <div class="discussion-groups" id="promptList" data-freebuff-accounts="${escapeHtml(JSON.stringify(model.freebuffAccounts ?? []))}">${renderPromptRows(model, search)}</div>
    </section>
  `;
};

const optionsForImport = (root: ParentNode, sourceAccountId: string): Array<{ id: string; label: string }> => {
  const raw = root.querySelector<HTMLElement>("[data-freebuff-accounts]")?.dataset.freebuffAccounts;
  if (!raw) return [];
  try {
    return JSON.parse(raw).filter((account: any) => account.id && account.id !== sourceAccountId);
  } catch {
    return [];
  }
};

const bindPromptRows = (
  root: ParentNode,
  onOpenDiscussion: PromptHistoryPanelMountOptions["onOpenDiscussion"],
  onImportToFreebuff: PromptHistoryPanelMountOptions["onImportToFreebuff"],
): void => {
  root.querySelectorAll<HTMLButtonElement>("[data-prompt-discussion]").forEach((button) => {
    button.addEventListener("click", () => {
      const sessionId = button.dataset.promptDiscussion;
      const accountId = button.dataset.promptAccount;
      if (accountId && sessionId) onOpenDiscussion(accountId, sessionId);
    });
  });
  root.querySelectorAll<HTMLButtonElement>("[data-prompt-import-open]").forEach((button) => {
    button.addEventListener("click", () => {
      const sourceAccountId = button.dataset.promptImportSource;
      const sessionId = button.dataset.promptImportOpen;
      if (!sourceAccountId || !sessionId) return;
      const accounts = optionsForImport(root, sourceAccountId);
      if (accounts.length === 0) {
        window.alert("Aucun compte Freebuff disponible. Ajoute d'abord un compte Freebuff dans Réglages.");
        return;
      }
      const targetAccountId = window.prompt(
        `Compte Freebuff cible :\\n${accounts.map((account, index) => `${index + 1}. ${account.label}`).join("\\n")}`,
        "1",
      );
      if (targetAccountId === null) return;
      const index = Number.parseInt(targetAccountId, 10) - 1;
      const target = Number.isInteger(index) ? accounts[index] : undefined;
      if (!target) {
        window.alert("Sélection Freebuff invalide.");
        return;
      }
      onImportToFreebuff(sourceAccountId, sessionId, target.id);
    });
  });
};

export const mountPromptHistoryPanel = (options: PromptHistoryPanelMountOptions): void => {
  const root = options.root ?? document;
  root.querySelector<HTMLButtonElement>("#refreshPromptHistory")?.addEventListener("click", options.onRefresh);
  root.querySelector<HTMLInputElement>("#promptSearch")?.addEventListener("input", (event) => {
    promptSearch = (event.currentTarget as HTMLInputElement).value;
    const host = root.querySelector<HTMLDivElement>("#promptList");
    if (!host) {
      options.rerender();
      return;
    }
    host.innerHTML = renderPromptRows(options.getModel());
    options.renderIcons(host);
    bindPromptRows(host, options.onOpenDiscussion, options.onImportToFreebuff);
  });
  bindPromptRows(root, options.onOpenDiscussion, options.onImportToFreebuff);
};
