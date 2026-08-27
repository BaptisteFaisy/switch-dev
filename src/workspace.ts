// Les noms de types et de champs `Workspace*` restent stables pour relire les
// settings et les appels API existants. Dans l'interface, ce concept est un
// « dossier » : un chemin projet qui regroupe conversations et terminaux.
export type WorkspaceProfile = {
  id: string;
  label: string;
  path: string;
  /** Contexte durable partage par tous les chats ouverts dans cet environnement. */
  memory: string;
  /** VPS utilise par defaut pour les nouveaux chats et terminaux. */
  executionTargetId?: string | null;
};

/**
 * Identite stable d'un dossier projet. Les chemins Windows et UNC sont compares
 * sans tenir compte de la casse, contrairement aux chemins Unix distants.
 */
export const normalizeWorkspacePath = (path: string): string => {
  const normalized = path.trim().replace(/[\\/]+$/, "").replaceAll("\\", "/");
  return /^[a-zA-Z]:\//.test(normalized) || normalized.startsWith("//")
    ? normalized.toLowerCase()
    : normalized;
};

export const workspaceIdForPath = (path: string): string => normalizeWorkspacePath(path);

export const workspaceBaseName = (path: string): string =>
  (path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path).replace(/\.git$/i, "");

/**
 * Un terminal ne peut appartenir qu'a un environnement explicitement nomme.
 * Cette fonction centralise la regle (les chaines vides ne sont jamais un
 * dossier implicite) pour que l'UI, la restauration et le transport appliquent
 * exactement la meme contrainte.
 */
export const terminalEnvironmentPath = (
  path: string | null | undefined,
): string | null => path?.trim() || null;

export type WorkspaceBreadcrumb = {
  label: string;
  path: string;
};

/** Construit un fil d'Ariane navigable, borne a la racine exposee. */
export const workspacePathBreadcrumbs = (
  root: string | null | undefined,
  current: string | null | undefined,
): WorkspaceBreadcrumb[] => {
  const currentPath = terminalEnvironmentPath(current);
  if (!currentPath) return [];
  const rootPath = terminalEnvironmentPath(root);
  if (!rootPath) {
    return [{ label: workspaceBaseName(currentPath), path: currentPath }];
  }

  const slashPath = (value: string) =>
    value.replaceAll("\\", "/").replace(/\/+$/, "") || "/";
  const rootSlash = slashPath(rootPath);
  const currentSlash = slashPath(currentPath);
  const windowsPath = /^[a-zA-Z]:\//.test(rootSlash) || rootSlash.startsWith("//");
  const compare = (value: string) => (windowsPath ? value.toLowerCase() : value);
  const rootKey = compare(rootSlash);
  const currentKey = compare(currentSlash);
  if (currentKey !== rootKey && !currentKey.startsWith(`${rootKey}/`)) {
    return [{ label: workspaceBaseName(currentPath), path: currentPath }];
  }

  const separator = currentPath.includes("\\") ? "\\" : "/";
  const rootWithoutTrailingSeparator = rootPath.replace(/[\\/]+$/, "");
  const breadcrumbs: WorkspaceBreadcrumb[] = [
    { label: workspaceBaseName(rootPath), path: rootPath },
  ];
  let accumulated = rootWithoutTrailingSeparator;
  const relative = currentSlash.slice(rootSlash.length).replace(/^\/+/, "");
  relative.split("/").filter(Boolean).forEach((segment) => {
    accumulated = `${accumulated}${separator}${segment}`;
    breadcrumbs.push({ label: segment, path: accumulated });
  });
  return breadcrumbs;
};

/** Chemin d'environnement explicitement choisi. */
export const userEnvironmentPath = (
  path: string | null | undefined,
): string | null => terminalEnvironmentPath(path);

const comparableEnvironmentPath = (path: string): string => {
  const normalized = normalizeWorkspacePath(path);
  const caseInsensitive =
    /^[a-zA-Z]:\//.test(normalized) ||
    normalized.startsWith("//") ||
    /^%[^%]+%\//.test(normalized);
  return caseInsensitive ? normalized.toLowerCase() : normalized;
};

/**
 * Un home d'authentification (CODEX_HOME / CLAUDE_CONFIG_DIR) est un dossier
 * technique, jamais un environnement projet. Les variantes de separateurs et
 * de casse des chemins Windows ou a variable `%...%` sont equivalentes.
 */
export const userEnvironmentPathExcluding = (
  path: string | null | undefined,
  excludedPaths: Iterable<string | null | undefined>,
): string | null => {
  const environment = userEnvironmentPath(path);
  if (!environment) return null;
  const key = comparableEnvironmentPath(environment);
  for (const excluded of excludedPaths) {
    const candidate = userEnvironmentPath(excluded);
    if (candidate && comparableEnvironmentPath(candidate) === key) return null;
  }
  return environment;
};

/** Un environnement distant est toujours un chemin Unix absolu du serveur. */
export const remoteEnvironmentPath = (
  path: string | null | undefined,
): string | null => {
  const environment = userEnvironmentPath(path);
  return environment?.startsWith("/") ? environment : null;
};

/** Identite canonique d'une cible d'execution distante (actuellement son URL). */
export const normalizeWorkspaceExecutionTargetId = (
  value: string | null | undefined,
): string | null => value?.trim().replace(/\/+$/, "").toLowerCase() || null;

export type FolderLinkedTerminal = {
  folderPath?: string | null;
};

/**
 * L'appartenance se calcule depuis le dossier choisi par l'utilisateur.
 */
export const terminalBelongsToFolder = (
  terminal: FolderLinkedTerminal,
  folderPath: string | null | undefined,
): boolean => {
  const left = terminal.folderPath?.trim();
  const right = folderPath?.trim();
  if (!left || !right) return !left && !right;
  return workspaceIdForPath(left) === workspaceIdForPath(right);
};

export const terminalsForFolder = <T extends FolderLinkedTerminal>(
  terminals: readonly T[],
  folderPath: string | null | undefined,
): T[] => terminals.filter((terminal) => terminalBelongsToFolder(terminal, folderPath));

export type DraftableChatPane = {
  discussion?: { accountId: string; sessionId: string } | null;
};

export type ListedChatDiscussionIdentity = {
  accountId: string;
  sessionId: string;
};

const listedChatDiscussionKey = (
  discussion: ListedChatDiscussionIdentity,
): string => JSON.stringify([
  discussion.accountId.trim(),
  discussion.sessionId.trim(),
]);

/**
 * Selectionne, parmi les chats ouverts d'un environnement, ceux qui ne sont pas
 * deja representes par une discussion listee dans la barre laterale : soit un
 * nouveau chat sans premier message (pas encore de discussion), soit un chat
 * dont la discussion n'a pas de dossier resolu cote serveur et qui a donc ete
 * ecarte de la liste persistee. Sans cette union, la
 * grille compte « 1 chat » pendant que la barre laterale affiche « Aucun chat ».
 */
export const draftEnvironmentChatPanes = <T extends DraftableChatPane>(
  environmentPanes: readonly T[],
  listedDiscussions: Iterable<ListedChatDiscussionIdentity>,
): T[] => {
  const listed = new Set(Array.from(listedDiscussions, listedChatDiscussionKey));
  return environmentPanes.filter((pane) => {
    const discussion = pane.discussion;
    return !discussion || !listed.has(listedChatDiscussionKey(discussion));
  });
};

export type MergedWorkspaceProfiles = {
  workspaces: WorkspaceProfile[];
  changed: boolean;
};

export type WorkspaceRegistryUpdate = {
  workspaces: WorkspaceProfile[];
  closedWorkspaceIds: string[];
  changed: boolean;
};

/**
 * Recalcule toujours l'id depuis le chemin puis fusionne les doublons. Garder
 * la premiere occurrence preserve son chemin d'affichage et son libelle, tout
 * en donnant une identite commune aux chats et aux terminaux rattaches.
 */
export const mergeWorkspaceProfiles = (
  profiles: readonly WorkspaceProfile[],
): MergedWorkspaceProfiles => {
  const byId = new Map<string, WorkspaceProfile>();
  let changed = false;

  profiles.forEach((profile) => {
    const path = userEnvironmentPath(profile.path);
    if (!path) {
      changed = true;
      return;
    }

    const id = workspaceIdForPath(path);
    const label = profile.label.trim() || workspaceBaseName(path);
    const memory = (profile.memory ?? "").trim();
    const executionTargetId = normalizeWorkspaceExecutionTargetId(profile.executionTargetId);
    const existing = byId.get(id);
    if (existing) {
      // Une ancienne copie sans memoire ne doit pas effacer la copie renseignee
      // du meme environnement lors de la migration/deduplication.
      if (!existing.memory && memory) existing.memory = memory;
      if (!existing.executionTargetId && executionTargetId) {
        existing.executionTargetId = executionTargetId;
      }
      changed = true;
      return;
    }

    if (
      profile.id !== id ||
      profile.label !== label ||
      profile.path !== path ||
      profile.memory !== memory ||
      (profile.executionTargetId ?? null) !== executionTargetId
    ) {
      changed = true;
    }
    byId.set(id, {
      id,
      label,
      path,
      memory,
      ...(executionTargetId ? { executionTargetId } : {}),
    });
  });

  return { workspaces: [...byId.values()], changed };
};

/** Normalise et deduplique les tombstones des dossiers fermes. */
export const mergeClosedWorkspaceIds = (ids: readonly string[]): string[] => {
  const seen = new Set<string>();
  const merged: string[] = [];
  ids.forEach((rawId) => {
    const id = workspaceIdForPath(rawId);
    if (!id || seen.has(id)) return;
    seen.add(id);
    merged.push(id);
  });
  return merged;
};

/**
 * Ouvre (ou rouvre) un dossier : ajoute son profil et retire son tombstone.
 * Le resultat est directement persistable dans AppSettings.
 */
export const openWorkspaceRegistry = (
  profiles: readonly WorkspaceProfile[],
  closedIds: readonly string[],
  path: string,
): WorkspaceRegistryUpdate => {
  const trimmed = userEnvironmentPath(path);
  const id = trimmed ? workspaceIdForPath(trimmed) : "";
  const merged = mergeWorkspaceProfiles(profiles);
  const normalizedClosed = mergeClosedWorkspaceIds(closedIds);
  const workspaces = [...merged.workspaces];
  const hadProfile = workspaces.some((workspace) => workspace.id === id);
  const wasClosed = normalizedClosed.includes(id);

  if (trimmed && !hadProfile) {
    workspaces.push({ id, label: workspaceBaseName(trimmed), path: trimmed, memory: "" });
  }

  return {
    workspaces,
    closedWorkspaceIds: normalizedClosed.filter((closedId) => closedId !== id),
    changed:
      merged.changed ||
      (!!trimmed && (!hadProfile || wasClosed)) ||
      normalizedClosed.length !== closedIds.length ||
      normalizedClosed.some((closedId, index) => closedId !== closedIds[index]),
  };
};

/** Met a jour le contexte durable d'un environnement sans toucher aux autres. */
export const setWorkspaceMemory = (
  profiles: readonly WorkspaceProfile[],
  path: string,
  memory: string,
): MergedWorkspaceProfiles => {
  const merged = mergeWorkspaceProfiles(profiles);
  const environmentPath = userEnvironmentPath(path);
  if (!environmentPath) return merged;

  const id = workspaceIdForPath(environmentPath);
  const normalizedMemory = memory.trim();
  const existing = merged.workspaces.find((workspace) => workspace.id === id);
  if (!existing) {
    return {
      workspaces: [
        ...merged.workspaces,
        {
          id,
          label: workspaceBaseName(environmentPath),
          path: environmentPath,
          memory: normalizedMemory,
        },
      ],
      changed: true,
    };
  }
  if (existing.memory === normalizedMemory) return merged;

  return {
    workspaces: merged.workspaces.map((workspace) =>
      workspace.id === id ? { ...workspace, memory: normalizedMemory } : workspace,
    ),
    changed: true,
  };
};

/** Definit le VPS par defaut d'un environnement, ou revient au routage automatique. */
export const setWorkspaceExecutionTarget = (
  profiles: readonly WorkspaceProfile[],
  path: string,
  executionTargetId: string | null | undefined,
): MergedWorkspaceProfiles => {
  const merged = mergeWorkspaceProfiles(profiles);
  const environmentPath = userEnvironmentPath(path);
  if (!environmentPath) return merged;

  const id = workspaceIdForPath(environmentPath);
  const normalizedTargetId = normalizeWorkspaceExecutionTargetId(executionTargetId);
  const existing = merged.workspaces.find((workspace) => workspace.id === id);
  if (!existing) {
    return {
      workspaces: [
        ...merged.workspaces,
        {
          id,
          label: workspaceBaseName(environmentPath),
          path: environmentPath,
          memory: "",
          ...(normalizedTargetId ? { executionTargetId: normalizedTargetId } : {}),
        },
      ],
      changed: true,
    };
  }
  if ((existing.executionTargetId ?? null) === normalizedTargetId) return merged;

  return {
    workspaces: merged.workspaces.map((workspace) => {
      if (workspace.id !== id) return workspace;
      if (normalizedTargetId) return { ...workspace, executionTargetId: normalizedTargetId };
      const { executionTargetId: _removed, ...withoutTarget } = workspace;
      return withoutTarget;
    }),
    changed: true,
  };
};

/**
 * Ferme un dossier sans toucher a son contenu : retire son profil et conserve
 * un tombstone pour que les anciennes discussions ne le rouvrent pas seules.
 */
export const closeWorkspaceRegistry = (
  profiles: readonly WorkspaceProfile[],
  closedIds: readonly string[],
  path: string,
): WorkspaceRegistryUpdate => {
  const selectablePath = userEnvironmentPath(path);
  const id = selectablePath ? workspaceIdForPath(selectablePath) : "";
  const merged = mergeWorkspaceProfiles(profiles);
  const normalizedClosed = mergeClosedWorkspaceIds(closedIds);
  const hadProfile = merged.workspaces.some((workspace) => workspace.id === id);
  const wasClosed = normalizedClosed.includes(id);

  return {
    workspaces: merged.workspaces.filter((workspace) => workspace.id !== id),
    closedWorkspaceIds: !id || wasClosed ? normalizedClosed : [...normalizedClosed, id],
    changed:
      merged.changed ||
      hadProfile ||
      (!!id && !wasClosed) ||
      normalizedClosed.length !== closedIds.length ||
      normalizedClosed.some((closedId, index) => closedId !== closedIds[index]),
  };
};
