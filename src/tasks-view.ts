import {
  TASK_TITLE_MAX_LENGTH,
  addTaskItem,
  filterTaskItems,
  loadTaskItems,
  localTaskDateKey,
  normalizeTaskTitle,
  persistTaskItems,
  removeTaskItem,
  renameTaskItem,
  searchTaskItems,
  setTaskCompleted,
  taskBelongsToEnvironment,
  taskDueState,
  taskItemsForEnvironment,
  taskScheduleGroup,
  taskStats,
  updateTaskDetails,
  type TaskDueState,
  type TaskEnvironment,
  type TaskFilter,
  type TaskItem,
  type TaskPriority,
  type TaskScheduleGroup,
  type TaskStorage,
} from "./tasks.ts";

export type TasksPanelOptions = {
  storage?: TaskStorage | null;
  accountId?: string | null;
  renderIcons?: (root: ParentNode) => void;
  environment?: TaskEnvironment | null;
  onExecuteTask?: (task: TaskItem) => void;
  onItemsChanged?: (items: readonly TaskItem[]) => void;
};

const escapeHtml = (value: string): string =>
  value.replace(/[&<>"']/g, (character) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[character] ?? character);

const formatTaskDate = (timestamp: number): string => {
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return "Ajoutée récemment";
  const today = new Date();
  const sameDay = date.getFullYear() === today.getFullYear()
    && date.getMonth() === today.getMonth()
    && date.getDate() === today.getDate();
  if (sameDay) return "Ajoutée aujourd’hui";
  return `Ajoutée le ${new Intl.DateTimeFormat("fr-FR", {
    day: "numeric",
    month: "short",
  }).format(date)}`;
};

const taskCountLabel = (count: number, singular: string, plural = `${singular}s`): string =>
  `${count} ${count === 1 ? singular : plural}`;

const taskPriorityLabel = (priority: TaskPriority): string => {
  if (priority === "high") return "Priorité haute";
  if (priority === "low") return "Priorité basse";
  return "Priorité normale";
};

const renderPriorityOptions = (priority: TaskPriority): string => [
  ["normal", "Priorité normale"],
  ["high", "Priorité haute"],
  ["low", "Priorité basse"],
].map(([value, label]) =>
  `<option value="${value}" ${priority === value ? "selected" : ""}>${label}</option>`,
).join("");

const renderTaskEnvironmentOptions = (
  environmentPath: string | null,
  environment: TaskEnvironment | null,
): string => {
  const personalSelected = environmentPath ? "" : "selected";
  const environmentSelected = environmentPath ? "selected" : "";
  return `
    <option value="personal" ${personalSelected}>Personnelle</option>
    <option value="environment" ${environmentSelected} ${environment ? "" : "disabled"}>${environment
      ? `Cet environnement · ${escapeHtml(environment.label)}`
      : "Aucun environnement sélectionné"}</option>`;
};

const taskDuePresentation = (task: TaskItem): { label: string; state: TaskDueState } | null => {
  if (!task.dueDate) return null;
  const state = taskDueState(task);
  const tomorrow = new Date();
  tomorrow.setDate(tomorrow.getDate() + 1);
  const formatted = new Intl.DateTimeFormat("fr-FR", {
    day: "numeric",
    month: "short",
  }).format(new Date(`${task.dueDate}T12:00:00`));
  const dateLabel = task.dueDate === localTaskDateKey()
    ? "Aujourd’hui"
    : task.dueDate === localTaskDateKey(tomorrow.getTime())
      ? "Demain"
      : formatted;
  if (state === "overdue") return { label: `En retard · ${formatted}`, state };
  return { label: task.completed ? `Échéance · ${dateLabel}` : dateLabel, state };
};

let activeTaskFilter: TaskFilter = "all";
let editingTaskId: string | null = null;
let taskFeedback = "";
let taskSearch = "";

const renderTaskItem = (task: TaskItem, environment: TaskEnvironment | null): string => {
  const title = escapeHtml(task.title);
  const id = escapeHtml(task.id);
  const editing = editingTaskId === task.id;
  const due = taskDuePresentation(task);
  const priorityLabel = taskPriorityLabel(task.priority);
  const environmentLabel = escapeHtml(environment?.label ?? "Cet environnement");
  return `
    <li class="task-item priority-${task.priority} due-${due?.state ?? "none"} ${task.completed ? "is-completed" : ""} ${task.environmentPath ? "is-environment-task" : ""} ${editing ? "is-editing" : ""}">
      <label class="task-toggle" title="${task.completed ? "Rouvrir la tâche" : "Marquer comme terminée"}">
        <input type="checkbox" data-task-toggle="${id}" ${task.completed ? "checked" : ""} aria-label="${task.completed ? "Rouvrir" : "Terminer"} : ${title}" />
        <span aria-hidden="true"><i data-lucide="check"></i></span>
      </label>
      ${editing
        ? `<form class="task-edit-form" data-task-edit-form="${id}">
            <div class="task-edit-fields">
              <label class="task-visually-hidden" for="taskEdit-${id}">Modifier la tâche</label>
              <input id="taskEdit-${id}" data-task-edit-input="${id}" value="${title}" maxlength="${TASK_TITLE_MAX_LENGTH}" required />
              <div class="task-edit-details">
                <label><span>Échéance</span><input type="date" data-task-edit-due="${id}" value="${task.dueDate ?? ""}" /></label>
                <label><span>Priorité</span><select data-task-edit-priority="${id}">${renderPriorityOptions(task.priority)}</select></label>
                <label><span>Catégorie</span><select data-task-edit-environment="${id}">${renderTaskEnvironmentOptions(task.environmentPath, environment)}</select></label>
              </div>
            </div>
            <button type="submit" title="Enregistrer"><i data-lucide="check"></i><span>Enregistrer</span></button>
            <button type="button" data-task-edit-cancel="${id}" title="Annuler"><i data-lucide="x"></i><span class="task-visually-hidden">Annuler</span></button>
          </form>`
        : `<div class="task-copy">
            <strong>${title}</strong>
            <div class="task-metadata">
              <small>${task.completed ? "Terminée" : formatTaskDate(task.createdAt)}</small>
              ${due ? `<span class="task-due task-due-${due.state}"><i data-lucide="calendar-clock"></i>${escapeHtml(due.label)}</span>` : ""}
              ${task.priority !== "normal" ? `<span class="task-priority task-priority-${task.priority}">${escapeHtml(priorityLabel)}</span>` : ""}
              ${task.environmentPath ? `<span class="task-environment"><i data-lucide="folder"></i>${environmentLabel}</span>` : ""}
            </div>
          </div>
          <div class="task-actions">
            ${task.environmentPath ? `<button class="task-execute" type="button" data-task-execute="${id}" title="Exécuter cette tâche dans ${environmentLabel}" aria-label="Exécuter : ${title}"><i data-lucide="play"></i><span>Exécuter</span></button>` : ""}
            <button type="button" data-task-edit="${id}" title="Modifier la tâche" aria-label="Modifier : ${title}"><i data-lucide="pencil"></i></button>
            <button type="button" data-task-delete="${id}" title="Supprimer la tâche" aria-label="Supprimer : ${title}"><i data-lucide="trash-2"></i></button>
          </div>`}
    </li>`;
};

const taskScheduleGroups: Array<{
  id: TaskScheduleGroup;
  label: string;
  detail: string;
  icon: string;
}> = [
  { id: "today", label: "Aujourd’hui", detail: "À traiter maintenant", icon: "target" },
  { id: "tomorrow", label: "Demain", detail: "La prochaine étape", icon: "clock-3" },
  {
    id: "week",
    label: "Fin de la semaine",
    detail: "À garder en vue",
    icon: "calendar-clock",
  },
];

const renderTaskGroup = (
  definition: (typeof taskScheduleGroups)[number],
  tasks: readonly TaskItem[],
  environment: TaskEnvironment | null,
): string => {
  const groupTasks = tasks.filter((task) => taskScheduleGroup(task) === definition.id);
  const emptyLabel = taskSearch
    ? "Aucun résultat"
    : activeTaskFilter === "completed"
      ? "Rien de terminé"
      : "Aucune tâche";
  return `
    <section class="task-group task-group-${definition.id}" data-task-group="${definition.id}" aria-labelledby="taskGroup-${definition.id}">
      <header>
        <span class="task-group-icon" aria-hidden="true"><i data-lucide="${definition.icon}"></i></span>
        <div>
          <h3 id="taskGroup-${definition.id}">${definition.label}</h3>
          <small>${definition.detail}</small>
        </div>
        <b aria-label="${taskCountLabel(groupTasks.length, "tâche")}">${groupTasks.length}</b>
      </header>
      ${groupTasks.length
        ? `<ul class="task-list">${groupTasks.map((task) => renderTaskItem(task, environment)).join("")}</ul>`
        : `<div class="task-group-empty"><i data-lucide="check"></i><span>${emptyLabel}</span></div>`}
    </section>`;
};

export const renderTasksPanel = (
  storage?: TaskStorage | null,
  environment: TaskEnvironment | null = null,
  accountId?: string | null,
): string => {
  const tasks = loadTaskItems(storage, accountId);
  const scopedTasks = taskItemsForEnvironment(tasks, environment?.path);
  const stats = taskStats(scopedTasks);
  const visibleTasks = searchTaskItems(filterTaskItems(scopedTasks, activeTaskFilter), taskSearch);
  const environmentTaskCount = environment
    ? scopedTasks.filter((task) => taskBelongsToEnvironment(task, environment.path)).length
    : 0;
  const filterButton = (filter: TaskFilter, label: string, count: number) => `
    <button type="button" data-task-filter="${filter}" class="${activeTaskFilter === filter ? "active" : ""}" aria-pressed="${activeTaskFilter === filter}">
      <span>${label}</span><b>${count}</b>
    </button>`;

  return `
    <section id="tasksPanel" class="tasks-panel" aria-labelledby="tasksPanelTitle">
      <div class="tasks-shell">
        <header class="tasks-hero">
          <div class="tasks-heading">
            <span class="tasks-heading-icon" aria-hidden="true"><i data-lucide="list-checks"></i></span>
            <div>
              <p>Organisation personnelle et projet</p>
              <h2 id="tasksPanelTitle">Mes tâches</h2>
              <span>${stats.active
                ? `${taskCountLabel(stats.active, "tâche")} à faire`
                : stats.total
                  ? "Vous êtes à jour"
                  : "Une liste simple pour garder le cap"}</span>
            </div>
          </div>
          <div class="tasks-progress" style="--tasks-progress: ${stats.progress}%" aria-label="${stats.progress} % des tâches terminées">
            <span><strong>${stats.progress}%</strong><small>terminé</small></span>
          </div>
        </header>

        <form id="taskCreateForm" class="task-create">
          <div class="task-create-main">
            <label class="task-visually-hidden" for="taskTitle">Nouvelle tâche</label>
            <span aria-hidden="true"><i data-lucide="plus"></i></span>
            <input id="taskTitle" name="title" type="text" maxlength="${TASK_TITLE_MAX_LENGTH}" placeholder="Ajouter une tâche…" autocomplete="off" required />
            <button type="submit"><i data-lucide="plus"></i><span>Ajouter</span></button>
          </div>
          <div class="task-create-details">
            <label><span>Échéance</span><input id="taskDueDate" name="dueDate" type="date" value="${localTaskDateKey()}" /></label>
            <label><span>Priorité</span><select id="taskPriority" name="priority">${renderPriorityOptions("normal")}</select></label>
            <label><span>Catégorie</span><select id="taskEnvironment" name="environment">${renderTaskEnvironmentOptions(null, environment)}</select></label>
          </div>
        </form>

        <div class="tasks-toolbar">
          <div class="task-filters" role="group" aria-label="Filtrer les tâches">
            ${filterButton("all", "Toutes", stats.total)}
            ${filterButton("active", "À faire", stats.active)}
            ${filterButton("completed", "Terminées", stats.completed)}
          </div>
          <label class="task-search">
            <i data-lucide="search"></i>
            <span class="task-visually-hidden">Rechercher une tâche</span>
            <input id="taskSearch" type="search" value="${escapeHtml(taskSearch)}" placeholder="Rechercher…" autocomplete="off" />
          </label>
        </div>

        ${environment ? `<aside class="task-environment-summary" data-task-environment-summary>
          <span aria-hidden="true"><i data-lucide="folder"></i></span>
          <div><strong>Tâches relatives à cet environnement</strong><small>${escapeHtml(environment.label)} · ${escapeHtml(environment.path)}</small></div>
          <b>${taskCountLabel(environmentTaskCount, "tâche")}</b>
        </aside>` : `<aside class="task-environment-summary is-unavailable" data-task-environment-summary>
          <span aria-hidden="true"><i data-lucide="folder-x"></i></span>
          <div><strong>Tâches relatives à un environnement</strong><small>Sélectionnez d’abord un environnement pour utiliser cette catégorie.</small></div>
        </aside>`}

        <div class="tasks-list-card">
          <div class="task-groups">${taskScheduleGroups
            .map((definition) => renderTaskGroup(definition, visibleTasks, environment))
            .join("")}</div>
        </div>

        <footer class="tasks-footer">
          <p id="tasksFeedback" aria-live="polite">${escapeHtml(taskFeedback)}</p>
          ${stats.completed
            ? `<button type="button" id="tasksClearCompleted"><i data-lucide="trash-2"></i><span>Effacer les terminées</span></button>`
            : `<span>Les tâches sont enregistrées sur cet appareil.</span>`}
        </footer>
      </div>
    </section>`;
};

type TaskFocusTarget =
  | { kind: "create" }
  | { kind: "search" }
  | { kind: "filter"; filter: TaskFilter }
  | { kind: "toggle" | "edit" | "edit-input"; id: string }
  | null;

const focusTaskTarget = (root: HTMLElement, target: TaskFocusTarget) => {
  if (!target) return;
  if (target.kind === "create") {
    root.querySelector<HTMLInputElement>("#taskTitle")?.focus();
    return;
  }
  if (target.kind === "search") {
    const input = root.querySelector<HTMLInputElement>("#taskSearch");
    input?.focus();
    input?.setSelectionRange(input.value.length, input.value.length);
    return;
  }
  if (target.kind === "filter") {
    Array.from(root.querySelectorAll<HTMLButtonElement>("[data-task-filter]"))
      .find((button) => button.dataset.taskFilter === target.filter)
      ?.focus();
    return;
  }
  const attribute = target.kind === "toggle"
    ? "taskToggle"
    : target.kind === "edit"
      ? "taskEdit"
      : "taskEditInput";
  Array.from(root.querySelectorAll<HTMLElement>(`[data-${target.kind === "edit-input" ? "task-edit-input" : target.kind === "edit" ? "task-edit" : "task-toggle"}]`))
    .find((element) => element.dataset[attribute] === target.id)
    ?.focus();
};

export const syncTaskNavigationBadges = (
  items: readonly TaskItem[],
  environmentPath?: string | null,
): void => {
  if (typeof document === "undefined") return;
  const active = taskStats(taskItemsForEnvironment(items, environmentPath)).active;
  document.querySelectorAll<HTMLElement>("[data-task-nav-count]").forEach((badge) => {
    badge.hidden = active === 0;
    badge.textContent = active > 99 ? "99+" : String(active);
    badge.setAttribute("aria-label", taskCountLabel(active, "tâche") + " à faire");
  });
};

export const mountTasksPanel = (options: TasksPanelOptions = {}): void => {
  const root = document.querySelector<HTMLElement>("#tasksPanel");
  if (!root) return;

  const refresh = (focus: TaskFocusTarget = null) => {
    const currentRoot = document.querySelector<HTMLElement>("#tasksPanel");
    if (!currentRoot) return;
    currentRoot.outerHTML = renderTasksPanel(
      options.storage,
      options.environment ?? null,
      options.accountId,
    );
    const nextRoot = document.querySelector<HTMLElement>("#tasksPanel");
    if (!nextRoot) return;
    options.renderIcons?.(nextRoot);
    mountTasksPanel(options);
    queueMicrotask(() => focusTaskTarget(nextRoot, focus));
  };

  const save = (items: TaskItem[], message: string, focus: TaskFocusTarget = null) => {
    const persisted = persistTaskItems(items, options.storage, options.accountId);
    taskFeedback = persisted ? message : "Impossible d’enregistrer les tâches sur cet appareil.";
    if (persisted) {
      syncTaskNavigationBadges(items, options.environment?.path);
      // Le cache local est immédiat ; le serveur reçoit la liste en arrière-plan.
      void import("./tasks-sync").then(({ markTasksDirty, pushTasksToServer }) => {
        markTasksDirty();
        void pushTasksToServer(items, options.accountId);
      });
    }
    refresh(focus);
    if (persisted) options.onItemsChanged?.(items);
  };

  root.querySelector<HTMLFormElement>("#taskCreateForm")?.addEventListener("submit", (event) => {
    event.preventDefault();
    const input = root.querySelector<HTMLInputElement>("#taskTitle");
    const title = normalizeTaskTitle(input?.value);
    if (!input || !title) {
      input?.setCustomValidity("Saisissez une tâche.");
      input?.reportValidity();
      return;
    }
    input.setCustomValidity("");
    const dueDate = root.querySelector<HTMLInputElement>("#taskDueDate")?.value ?? null;
    const priority = root.querySelector<HTMLSelectElement>("#taskPriority")?.value ?? "normal";
    const category = root.querySelector<HTMLSelectElement>("#taskEnvironment")?.value;
    const next = addTaskItem(loadTaskItems(options.storage, options.accountId), title, Date.now(), undefined, {
      dueDate,
      priority,
      environmentPath: category === "environment" ? options.environment?.path : null,
    });
    editingTaskId = null;
    taskSearch = "";
    save(next, "Tâche ajoutée.", { kind: "create" });
  });

  root.querySelectorAll<HTMLInputElement>("[data-task-toggle]").forEach((input) => {
    input.addEventListener("change", () => {
      const id = input.dataset.taskToggle;
      if (!id) return;
      const next = setTaskCompleted(loadTaskItems(options.storage, options.accountId), id, input.checked);
      const message = input.checked ? "Tâche terminée." : "Tâche rouverte.";
      save(next, message, { kind: "toggle", id });
    });
  });

  root.querySelectorAll<HTMLButtonElement>("[data-task-filter]").forEach((button) => {
    button.addEventListener("click", () => {
      const filter = button.dataset.taskFilter as TaskFilter | undefined;
      if (!filter || !["all", "active", "completed"].includes(filter)) return;
      activeTaskFilter = filter;
      editingTaskId = null;
      taskFeedback = "";
      refresh({ kind: "filter", filter });
    });
  });

  root.querySelector<HTMLInputElement>("#taskSearch")?.addEventListener("input", (event) => {
    taskSearch = (event.currentTarget as HTMLInputElement).value.slice(0, TASK_TITLE_MAX_LENGTH);
    editingTaskId = null;
    taskFeedback = "";
    refresh({ kind: "search" });
  });

  root.querySelectorAll<HTMLButtonElement>("[data-task-edit]").forEach((button) => {
    button.addEventListener("click", () => {
      const id = button.dataset.taskEdit;
      if (!id) return;
      editingTaskId = id;
      taskFeedback = "";
      refresh({ kind: "edit-input", id });
    });
  });

  root.querySelectorAll<HTMLButtonElement>("[data-task-execute]").forEach((button) => {
    button.addEventListener("click", () => {
      const id = button.dataset.taskExecute;
      if (!id) return;
      const task = loadTaskItems(options.storage, options.accountId).find((item) => item.id === id);
      if (!task?.environmentPath || !options.onExecuteTask) return;
      taskFeedback = `Ouverture d’un chat pour « ${task.title} »…`;
      options.onExecuteTask(task);
    });
  });

  root.querySelectorAll<HTMLFormElement>("[data-task-edit-form]").forEach((form) => {
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const id = form.dataset.taskEditForm;
      const input = form.querySelector<HTMLInputElement>("[data-task-edit-input]");
      const dueDate = form.querySelector<HTMLInputElement>("[data-task-edit-due]")?.value ?? null;
      const priority = form.querySelector<HTMLSelectElement>("[data-task-edit-priority]")?.value
        ?? "normal";
      const category = form.querySelector<HTMLSelectElement>("[data-task-edit-environment]")?.value;
      const title = normalizeTaskTitle(input?.value);
      if (!id || !input || !title) {
        input?.setCustomValidity("Le titre ne peut pas être vide.");
        input?.reportValidity();
        return;
      }
      input.setCustomValidity("");
      editingTaskId = null;
      const renamed = renameTaskItem(loadTaskItems(options.storage, options.accountId), id, title);
      save(updateTaskDetails(renamed, id, {
        dueDate,
        priority,
        environmentPath: category === "environment" ? options.environment?.path : null,
      }), "Tâche modifiée.", {
        kind: "edit",
        id,
      });
    });
    form.querySelector<HTMLInputElement>("[data-task-edit-input]")?.addEventListener(
      "keydown",
      (event) => {
        if (event.key !== "Escape") return;
        event.preventDefault();
        const id = form.dataset.taskEditForm;
        editingTaskId = null;
        taskFeedback = "";
        refresh(id ? { kind: "edit", id } : null);
      },
    );
  });

  root.querySelectorAll<HTMLButtonElement>("[data-task-edit-cancel]").forEach((button) => {
    button.addEventListener("click", () => {
      const id = button.dataset.taskEditCancel;
      editingTaskId = null;
      taskFeedback = "";
      refresh(id ? { kind: "edit", id } : null);
    });
  });

  root.querySelectorAll<HTMLButtonElement>("[data-task-delete]").forEach((button) => {
    button.addEventListener("click", () => {
      const id = button.dataset.taskDelete;
      if (!id) return;
      editingTaskId = editingTaskId === id ? null : editingTaskId;
      save(removeTaskItem(loadTaskItems(options.storage, options.accountId), id), "Tâche supprimée.", { kind: "create" });
    });
  });

  root.querySelector<HTMLButtonElement>("#tasksClearCompleted")?.addEventListener("click", () => {
    const items = loadTaskItems(options.storage, options.accountId);
    const completedIds = new Set(
      taskItemsForEnvironment(items, options.environment?.path)
        .filter((task) => task.completed)
        .map((task) => task.id),
    );
    const count = completedIds.size;
    editingTaskId = null;
    save(
      items.filter((task) => !completedIds.has(task.id)),
      `${taskCountLabel(count, "tâche")} supprimée${count > 1 ? "s" : ""}.`,
      { kind: "create" },
    );
  });
};
