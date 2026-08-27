import { invoke, isRemoteMode } from "./platform";
import {
  loadTaskItems,
  persistTaskItems,
  type TaskItem,
  type TaskStorage,
} from "./tasks";

/**
 * Synchronisation serveur des tâches de l'onglet Tâches.
 *
 * Le serveur est la source de vérité (`GET/PUT /api/tasks`, liste scindée par
 * compte authentifié) ; le localStorage reste un cache instantané qui permet
 * de travailler hors-ligne. Chaque sauvegarde locale marque la liste comme
 * « sale » et la pousse au serveur ; si le serveur est injoignable, la
 * prochaine ouverture de l'onglet repousse le cache local (au lieu d'écraser
 * les modifications hors-ligne avec l'état serveur périmé).
 */

let localDirty = false;

/** Marque la liste locale comme modifiée (à re-pousser vers le serveur). */
export const markTasksDirty = (dirty = true): void => {
  localDirty = dirty;
};

const fetchRemoteTasks = async (
  accountId: string | null | undefined,
): Promise<TaskItem[] | null> => {
  try {
    const items = await invoke<TaskItem[]>("tasks_list", {
      account: accountId?.trim() || undefined,
    });
    return Array.isArray(items) ? items : null;
  } catch {
    return null;
  }
};

const pushRemoteTasks = async (
  items: readonly TaskItem[],
  accountId: string | null | undefined,
): Promise<boolean> => {
  try {
    await invoke("tasks_replace", {
      account: accountId?.trim() || undefined,
      items,
    });
    return true;
  } catch {
    return false;
  }
};

/**
 * Ouvre la synchronisation avec le serveur : adopte la liste serveur dans le
 * cache local (et rappelle `onAdopted`), ou, si des modifications locales sont
 * en attente ou que le serveur est vide, pousse le cache local vers le serveur.
 */
export const syncTasksFromServer = async (
  storage: TaskStorage | null | undefined,
  accountId: string | null | undefined,
  onAdopted?: (items: TaskItem[]) => void,
): Promise<void> => {
  if (!isRemoteMode() || typeof window === "undefined") return;
  const remote = await fetchRemoteTasks(accountId);
  if (remote === null) return;

  const local = loadTaskItems(storage, accountId);
  if (localDirty) {
    if (await pushRemoteTasks(local, accountId)) localDirty = false;
    return;
  }
  if (remote.length === 0 && local.length > 0) {
    // Première migration : le serveur est vide, on y dépose le cache local.
    if (await pushRemoteTasks(local, accountId)) localDirty = false;
    return;
  }
  if (JSON.stringify(remote) !== JSON.stringify(local)) {
    persistTaskItems(remote, storage, accountId);
    onAdopted?.(remote);
  }
};

/**
 * Pousse la liste locale vers le serveur (appelée après chaque sauvegarde
 * locale). Retourne `true` si le serveur a bien pris la liste.
 */
export const pushTasksToServer = async (
  items: readonly TaskItem[],
  accountId: string | null | undefined,
): Promise<boolean> => {
  if (!isRemoteMode() || typeof window === "undefined") return true;
  const pushed = await pushRemoteTasks(items, accountId);
  localDirty = !pushed;
  return pushed;
};

/**
 * Ajoute une tâche directement côté serveur (usage agent/API, hors interface).
 */
export const addRemoteTask = async (
  task: Pick<TaskItem, "id" | "title"> & Partial<TaskItem>,
  accountId?: string | null,
): Promise<boolean> => {
  if (!isRemoteMode() || typeof window === "undefined") return false;
  try {
    await invoke("tasks_add", {
      account: accountId?.trim() || undefined,
      task,
    });
    return true;
  } catch {
    return false;
  }
};

/**
 * Supprime une tâche directement côté serveur (usage agent/API, hors interface).
 */
export const removeRemoteTask = async (
  id: string,
  accountId?: string | null,
): Promise<boolean> => {
  if (!isRemoteMode() || typeof window === "undefined") return false;
  try {
    await invoke("tasks_remove", {
      account: accountId?.trim() || undefined,
      id,
    });
    return true;
  } catch {
    return false;
  }
};
