import {
  SKILL_CONTENT_MAX_BYTES,
  SKILL_DESCRIPTION_MAX_LENGTH,
  SKILL_NAME_MAX_LENGTH,
  loadCustomSkills,
  normalizeSkillContent,
  normalizeSkillName,
  persistCustomSkills,
  saveCustomSkill,
  skillContentBytes,
  skillDraftFromMarkdown,
  type CustomSkillEntry,
  type CustomSkillStorage,
} from "./skills";
import "./skills-view.css";

export type SkillEntry = {
  id: string;
  name: string;
  description: string;
  tags: string[];
  buttonLabel: string;
  icon: string;
  content: string;
  custom: boolean;
};

export type SkillsPanelOptions = {
  skills: readonly SkillEntry[];
  loaded: boolean;
  error: string | null;
  chatButtonIds: readonly string[];
  applyTarget?: "chat" | "terminal";
  hasActiveSession?: boolean;
};

export type OpenCustomSkillEditorOptions = {
  skill?: CustomSkillEntry | null;
  reservedIds?: readonly string[];
  storage?: CustomSkillStorage | null;
  renderIcons?: (root: ParentNode) => void;
  onSaved?: (skill: CustomSkillEntry, created: boolean) => void | Promise<void>;
};

const escapeHtml = (value: string): string => value.replace(/[&<>"']/g, (character) => ({
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
})[character] ?? character);

const renderSkillCard = (
  skill: SkillEntry,
  applyTarget: "chat" | "terminal",
  hasActiveSession: boolean,
  chatButtonIds: readonly string[],
): string => {
  const tags = skill.tags
    .slice(0, 6)
    .map((tag) => `<span class="skill-tag">${escapeHtml(tag)}</span>`)
    .join("");
  const applyToChat = applyTarget === "chat";
  const buttonAdded = chatButtonIds.includes(skill.id);
  const hasContent = skill.content.trim().length > 0;
  const canApply = hasContent && (applyToChat || hasActiveSession);
  const applyLabel = applyToChat ? "Ajouter au chat" : "Injecter dans Codex";
  const applyTitle = !hasContent
    ? "Contenu du skill indisponible"
    : applyToChat
      ? "Ajouter le skill au brouillon du chat"
      : hasActiveSession
        ? "Coller le skill dans la session Codex active (relis puis Entrée)"
        : "Aucune session Codex active";
  return `
    <div class="skill-card" data-skill-id="${escapeHtml(skill.id)}" tabindex="-1">
      <div class="skill-card-head">
        <strong>${escapeHtml(skill.name)}</strong>
        ${skill.custom ? `<span class="skill-origin"><i data-lucide="sparkles"></i>Personnel</span>` : ""}
      </div>
      ${skill.description ? `<p class="skill-desc">${escapeHtml(skill.description)}</p>` : ""}
      ${tags ? `<div class="skill-tags">${tags}</div>` : ""}
      <div class="skill-actions">
        <button class="tool-button ${buttonAdded ? "skill-chat-button-active" : "primary"}" data-skill-chat-button="${escapeHtml(skill.id)}" aria-pressed="${buttonAdded}" title="${buttonAdded ? "Retirer ce bouton de toutes les fenêtres de chat" : "Ajouter ce bouton à toutes les fenêtres de chat"}">
          <i data-lucide="${buttonAdded ? "x" : "plus"}"></i><span>${buttonAdded ? "Retirer des chats" : "Ajouter aux chats"}</span>
        </button>
        <button class="tool-button" data-skill-apply="${escapeHtml(skill.id)}" ${canApply ? "" : "disabled"} title="${escapeHtml(applyTitle)}">
          <i data-lucide="send"></i><span>${applyLabel === "Ajouter au chat" ? "Ajouter au message" : applyLabel}</span>
        </button>
        <button class="tool-button" data-skill-copy="${escapeHtml(skill.id)}" title="Copier le contenu du skill dans le presse-papiers">
          <i data-lucide="copy"></i><span>Copier</span>
        </button>
        ${skill.custom ? `
          <button class="tool-button" data-skill-edit="${escapeHtml(skill.id)}" title="Modifier ce skill personnel">
            <i data-lucide="pencil"></i><span>Modifier</span>
          </button>
          <button class="icon-button danger" data-skill-delete="${escapeHtml(skill.id)}" title="Supprimer ce skill personnel" aria-label="Supprimer ${escapeHtml(skill.name)}">
            <i data-lucide="trash-2"></i>
          </button>` : ""}
      </div>
      <details class="skill-details">
        <summary>Voir le contenu</summary>
        <pre class="skill-content">${escapeHtml(skill.content)}</pre>
      </details>
    </div>`;
};

export const renderSkillsPanel = ({
  skills,
  loaded,
  error,
  chatButtonIds,
  applyTarget = "chat",
  hasActiveSession = false,
}: SkillsPanelOptions): string => {
  const sub = !loaded
    ? "Chargement…"
    : applyTarget === "chat"
      ? `${skills.length} skill(s) disponible(s) · choisis les boutons affichés dans toutes les fenêtres de chat`
      : `${skills.length} skill(s) disponible(s)${hasActiveSession ? "" : " · démarre un terminal Codex pour pouvoir les injecter"}`;
  const body = error
    ? `<div class="skills-load-warning"><i data-lucide="triangle-alert"></i><span>Les skills intégrés n’ont pas pu être chargés. Les skills personnels restent disponibles.<small>${escapeHtml(error)}</small></span></div>`
    : "";
  const list = !loaded
    ? `<div class="empty">Chargement des skills…</div>`
    : skills.length === 0
      ? `<div class="skills-empty"><i data-lucide="sparkles"></i><strong>Créez votre premier skill</strong><span>Importez un SKILL.md ou écrivez ses instructions directement.</span><button type="button" class="tool-button primary" data-skill-create><i data-lucide="plus"></i><span>Ajouter un skill</span></button></div>`
      : `<div id="skillsList" class="skills-list">${skills.map((skill) => renderSkillCard(skill, applyTarget, hasActiveSession, chatButtonIds)).join("")}</div>`;

  return `
    <div class="panel audit-panel skills-panel">
      <div class="panel-head">
        <div>
          <h2>Skills</h2>
          <p class="panel-sub">${escapeHtml(sub)}</p>
        </div>
        <div class="panel-actions">
          <button id="skillsAdd" class="tool-button primary" title="Importer ou créer un skill"><i data-lucide="plus"></i><span>Ajouter un skill</span></button>
          <button id="skillsRefresh" class="icon-button wide" title="Rafraîchir la bibliothèque"><i data-lucide="refresh-ccw"></i></button>
        </div>
      </div>
      ${body}${list}
    </div>`;
};

const skillEditorMarkup = (skill: CustomSkillEntry | null): string => `
  <form method="dialog" class="skill-editor-shell" id="skillEditorForm">
    <header class="skill-editor-head">
      <span class="skill-editor-mark" aria-hidden="true"><i data-lucide="sparkles"></i></span>
      <div>
        <small>${skill ? "Skill personnel" : "Nouveau skill"}</small>
        <h2 id="skillEditorTitle">${skill ? `Modifier ${escapeHtml(skill.name)}` : "Ajouter un skill"}</h2>
        <p>Importez un <code>SKILL.md</code> ou écrivez directement ses instructions.</p>
      </div>
      <button type="button" class="icon-button" data-skill-editor-close title="Fermer" aria-label="Fermer"><i data-lucide="x"></i></button>
    </header>
    <div class="skill-editor-body">
      <button type="button" class="skill-import-zone" id="skillImportButton">
        <i data-lucide="upload"></i>
        <span><strong>Importer un fichier Markdown</strong><small>SKILL.md, .md ou .txt · 64 Ko maximum</small></span>
      </button>
      <input id="skillImportInput" type="file" accept=".md,.markdown,.txt,text/markdown,text/plain" hidden />
      <div class="skill-editor-grid">
        <label class="skill-editor-field">
          <span>Nom <b>obligatoire</b></span>
          <input id="skillEditorName" name="name" maxlength="${SKILL_NAME_MAX_LENGTH}" value="${escapeHtml(skill?.name ?? "")}" placeholder="Ex. Revue de sécurité" autocomplete="off" required />
        </label>
        <label class="skill-editor-field">
          <span>Tags <small>séparés par des virgules</small></span>
          <input id="skillEditorTags" name="tags" value="${escapeHtml(skill?.tags.join(", ") ?? "")}" placeholder="code, sécurité, revue" autocomplete="off" />
        </label>
      </div>
      <label class="skill-editor-field">
        <span>Description <small>quand utiliser ce skill</small></span>
        <textarea id="skillEditorDescription" name="description" maxlength="${SKILL_DESCRIPTION_MAX_LENGTH}" rows="3" placeholder="Décrivez le déclencheur et le résultat attendu…">${escapeHtml(skill?.description ?? "")}</textarea>
      </label>
      <label class="skill-editor-field skill-editor-content-field">
        <span>Instructions <b>obligatoires</b><small id="skillContentSize">0 Ko / 64 Ko</small></span>
        <textarea id="skillEditorContent" name="content" rows="14" placeholder="# Rôle\n\nExpliquez précisément comment l’agent doit travailler…" required>${escapeHtml(skill?.content ?? "")}</textarea>
      </label>
      <p class="skill-editor-feedback" id="skillEditorFeedback" role="status" aria-live="polite"></p>
    </div>
    <footer class="skill-editor-actions">
      <button type="button" class="tool-button" data-skill-editor-close>Annuler</button>
      <button type="submit" class="tool-button primary" id="skillEditorSave"><i data-lucide="save"></i><span>${skill ? "Enregistrer" : "Ajouter aux chats"}</span></button>
    </footer>
  </form>`;

export const openCustomSkillEditor = (
  options: OpenCustomSkillEditorOptions = {},
): void => {
  if (typeof document === "undefined") return;
  document.querySelector<HTMLDialogElement>("#skillEditorDialog")?.close();

  const stored = loadCustomSkills(options.storage);
  const requestedId = options.skill?.id ?? null;
  const skill = requestedId
    ? stored.find((candidate) => candidate.id === requestedId) ?? options.skill ?? null
    : null;
  const dialog = document.createElement("dialog");
  dialog.id = "skillEditorDialog";
  dialog.className = "skill-editor-dialog";
  dialog.setAttribute("aria-labelledby", "skillEditorTitle");
  dialog.innerHTML = skillEditorMarkup(skill);

  const nameInput = dialog.querySelector<HTMLInputElement>("#skillEditorName");
  const tagsInput = dialog.querySelector<HTMLInputElement>("#skillEditorTags");
  const descriptionInput = dialog.querySelector<HTMLTextAreaElement>("#skillEditorDescription");
  const contentInput = dialog.querySelector<HTMLTextAreaElement>("#skillEditorContent");
  const fileInput = dialog.querySelector<HTMLInputElement>("#skillImportInput");
  const feedback = dialog.querySelector<HTMLElement>("#skillEditorFeedback");
  const saveButton = dialog.querySelector<HTMLButtonElement>("#skillEditorSave");
  const returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;

  const setFeedback = (message: string, error = false): void => {
    if (!feedback) return;
    feedback.textContent = message;
    feedback.classList.toggle("error", error);
  };
  const updateContentSize = (): void => {
    const size = skillContentBytes(contentInput?.value ?? "");
    const target = dialog.querySelector<HTMLElement>("#skillContentSize");
    if (target) target.textContent = `${(size / 1024).toFixed(size >= 1024 ? 1 : 2)} Ko / 64 Ko`;
    contentInput?.classList.toggle("invalid", size > SKILL_CONTENT_MAX_BYTES);
  };
  const close = (): void => {
    if (dialog.open) dialog.close();
    else dialog.remove();
  };
  const importFile = async (file: File): Promise<void> => {
    if (file.size > SKILL_CONTENT_MAX_BYTES) {
      setFeedback("Ce fichier dépasse 64 Ko.", true);
      return;
    }
    try {
      const imported = skillDraftFromMarkdown(file.name, await file.text());
      if (!imported.content) throw new Error("empty");
      if (nameInput && imported.name) nameInput.value = imported.name;
      if (descriptionInput && imported.description) descriptionInput.value = imported.description;
      if (tagsInput && imported.tags.length) tagsInput.value = imported.tags.join(", ");
      if (contentInput) contentInput.value = imported.content;
      updateContentSize();
      setFeedback(`${file.name} est prêt à être enregistré.`);
      (nameInput?.value ? contentInput : nameInput)?.focus();
    } catch {
      setFeedback("Import impossible : choisissez un fichier Markdown lisible.", true);
    }
  };

  dialog.querySelectorAll<HTMLElement>("[data-skill-editor-close]").forEach((button) =>
    button.addEventListener("click", close));
  dialog.addEventListener("click", (event) => {
    if (event.target === dialog) close();
  });
  dialog.addEventListener("close", () => {
    dialog.remove();
    if (returnFocus?.isConnected) window.requestAnimationFrame(() => returnFocus.focus());
  }, { once: true });
  contentInput?.addEventListener("input", () => {
    contentInput.setCustomValidity("");
    setFeedback("");
    updateContentSize();
  });
  nameInput?.addEventListener("input", () => {
    nameInput.setCustomValidity("");
    setFeedback("");
  });
  dialog.querySelector<HTMLButtonElement>("#skillImportButton")?.addEventListener("click", () =>
    fileInput?.click());
  fileInput?.addEventListener("change", () => {
    const file = fileInput.files?.[0];
    if (file) void importFile(file);
    fileInput.value = "";
  });
  const importZone = dialog.querySelector<HTMLElement>("#skillImportButton");
  importZone?.addEventListener("dragover", (event) => {
    event.preventDefault();
    importZone.classList.add("dragging");
  });
  importZone?.addEventListener("dragleave", () => importZone.classList.remove("dragging"));
  importZone?.addEventListener("drop", (event) => {
    event.preventDefault();
    importZone.classList.remove("dragging");
    const file = event.dataTransfer?.files[0];
    if (file) void importFile(file);
  });
  dialog.querySelector<HTMLFormElement>("#skillEditorForm")?.addEventListener("submit", (event) => {
    event.preventDefault();
    const current = loadCustomSkills(options.storage);
    const result = saveCustomSkill(current, {
      name: nameInput?.value,
      description: descriptionInput?.value,
      tags: tagsInput?.value,
      content: contentInput?.value,
    }, {
      id: skill?.id,
      reservedIds: options.reservedIds,
    });
    if (!result.ok) {
      setFeedback(result.error, true);
      if (!normalizeSkillName(nameInput?.value)) {
        nameInput?.setCustomValidity(result.error);
        nameInput?.reportValidity();
      } else if (!normalizeSkillContent(contentInput?.value)
        || skillContentBytes(contentInput?.value ?? "") > SKILL_CONTENT_MAX_BYTES) {
        contentInput?.setCustomValidity(result.error);
        contentInput?.reportValidity();
      }
      return;
    }
    if (!persistCustomSkills(result.items, options.storage)) {
      setFeedback("Impossible d’enregistrer ce skill sur cet appareil.", true);
      return;
    }
    if (saveButton) saveButton.disabled = true;
    void Promise.resolve(options.onSaved?.(result.skill, result.created))
      .then(close)
      .catch(() => {
        if (saveButton) saveButton.disabled = false;
        setFeedback("Le skill est enregistré, mais la bibliothèque n’a pas pu être rafraîchie.", true);
      });
  });

  document.body.appendChild(dialog);
  options.renderIcons?.(dialog);
  updateContentSize();
  dialog.showModal();
  window.requestAnimationFrame(() => (skill ? contentInput : nameInput)?.focus());
};
