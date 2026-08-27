export type ExpertGridLayout = "auto" | "2" | "3" | "4";

export type ExpertChatPageSize = number;
export type ExpertChatPageSizeMode = "auto" | ExpertChatPageSize;
export type ExpertChatDisplayMode = "all" | "available";

export const DEFAULT_EXPERT_CHAT_PAGE_SIZE: ExpertChatPageSize = 6;
export const DEFAULT_EXPERT_CHAT_PAGE_SIZE_MODE: ExpertChatPageSizeMode = "auto";
export const DEFAULT_EXPERT_CHAT_DISPLAY_MODE: ExpertChatDisplayMode = "all";

export const EXPERT_CHAT_AUTO_MAX_PAGE_SIZE = 16;
export const EXPERT_CHAT_MIN_PANE_WIDTH = 340;
export const EXPERT_CHAT_MIN_PANE_HEIGHT = 270;
export const EXPERT_CHAT_GRID_GAP = 8;

const positivePageSize = (value: unknown): ExpertChatPageSize | null => {
  const numericValue = Number(value);
  if (!Number.isFinite(numericValue)) return null;
  return Math.min(
    Number.MAX_SAFE_INTEGER,
    Math.max(1, Math.trunc(numericValue)),
  );
};

export const normalizeExpertChatPageSize = (value: unknown): ExpertChatPageSize =>
  positivePageSize(value) ?? DEFAULT_EXPERT_CHAT_PAGE_SIZE;

export const normalizeExpertChatPageSizeMode = (value: unknown): ExpertChatPageSizeMode => {
  if (
    value == null
    || value === "auto"
    || (typeof value === "string" && !value.trim())
  ) return "auto";
  return positivePageSize(value) ?? DEFAULT_EXPERT_CHAT_PAGE_SIZE_MODE;
};

export const normalizeExpertChatDisplayMode = (value: unknown): ExpertChatDisplayMode =>
  value === "available" ? "available" : DEFAULT_EXPERT_CHAT_DISPLAY_MODE;

/**
 * Un tour lance depuis le chat actif ne doit pas faire disparaitre ce chat du
 * mode « Disponibles ». L'epingle reste ephemere et sera retiree a la fin du
 * tour par le runtime.
 */
export const shouldPinActiveExpertChatDuringTurn = (
  mode: ExpertChatDisplayMode,
  wasVisible: boolean,
  isActive: boolean,
): boolean => mode === "available" && wasVisible && isActive;

/**
 * Apres un rechargement, les epingles de visibilite sont volontairement
 * ephemeres alors que le panneau et le tour serveur sont restaures separement.
 * Le premier rattachement doit donc rendre le chat occupe a nouveau visible.
 * Un chat deja rattache n'est pas re-epingle afin de respecter une reduction
 * explicite faite pendant la session courante.
 */
export const shouldPinRestoredBusyExpertChat = (
  mode: ExpertChatDisplayMode,
  currentTurnBusy: boolean,
  candidateTurnBusy: boolean,
): boolean => mode === "available" && !currentTurnBusy && candidateTurnBusy;

/**
 * Un second clic sur le chat orange deja actif agit comme une reduction dans
 * le mode « Disponibles ». Un premier clic, un chat vert ou le mode complet
 * conservent leur comportement d'ouverture habituel.
 */
export const shouldMinimizeActiveBusyExpertChat = (
  mode: ExpertChatDisplayMode,
  isBusy: boolean,
  isActive: boolean,
  isVisible: boolean,
): boolean => mode === "available" && isBusy && isActive && isVisible;

export type ExpertChatHistoryFullscreenPreferences = {
  openOnFullscreen: boolean;
  closeOnCompact: boolean;
};

/**
 * Calcule l'etat du panneau d'historique apres un agrandissement ou une
 * reduction. Une preference desactivee conserve toujours l'etat courant.
 */
export const expertChatHistoryOpenAfterFullscreenChange = (
  historyOpen: boolean,
  nextFullscreen: boolean,
  preferences: ExpertChatHistoryFullscreenPreferences,
): boolean => {
  if (nextFullscreen && preferences.openOnFullscreen) return true;
  if (!nextFullscreen && preferences.closeOnCompact) return false;
  return historyOpen;
};

/**
 * Le filtre reste independant du modele de chat : l'appelant fournit la notion
 * de disponibilite correspondant a son runtime (tour running/finalizing).
 */
export const expertChatsForDisplay = <T>(
  chats: readonly T[],
  mode: ExpertChatDisplayMode,
  isAvailable: (chat: T) => boolean,
): T[] => mode === "available" ? chats.filter(isAvailable) : [...chats];

/**
 * Le mode automatique conserve tous les chats visibles jusqu'a la capacite
 * maximale de la grille. Au-dela, la pagination existante prend le relais.
 */
export const resolveExpertChatPageSize = (
  mode: ExpertChatPageSizeMode,
): ExpertChatPageSize => mode === "auto" ? 16 : mode;

/** Repartit les chats dans une grille aussi carree que possible. */
export const expertChatColumnCount = (pageSize: ExpertChatPageSize): number => {
  const count = normalizeExpertChatPageSize(pageSize);
  return count <= 3 ? count : Math.ceil(Math.sqrt(count));
};

const normalizeCount = (value: number): number =>
  Number.isFinite(value) ? Math.max(0, Math.trunc(value)) : 0;

export const expertChatPageCount = (
  openChatCount: number,
  pageSizeMode: ExpertChatPageSizeMode,
): number => Math.max(
  1,
  Math.ceil(normalizeCount(openChatCount) / resolveExpertChatPageSize(pageSizeMode)),
);

export const clampExpertChatPage = (
  page: number,
  openChatCount: number,
  pageSizeMode: ExpertChatPageSizeMode,
): number => {
  const lastPage = expertChatPageCount(openChatCount, pageSizeMode) - 1;
  const normalizedPage = Number.isFinite(page) ? Math.max(0, Math.trunc(page)) : 0;
  return Math.min(normalizedPage, lastPage);
};

export const expertChatPageForIndex = (
  index: number,
  pageSizeMode: ExpertChatPageSizeMode,
): number => Math.floor(normalizeCount(index) / resolveExpertChatPageSize(pageSizeMode));

export const expertChatsOnPage = <T>(
  chats: readonly T[],
  page: number,
  pageSizeMode: ExpertChatPageSizeMode,
): T[] => {
  const pageSize = resolveExpertChatPageSize(pageSizeMode);
  const normalizedPage = clampExpertChatPage(page, chats.length, pageSizeMode);
  const start = normalizedPage * pageSize;
  return chats.slice(start, start + pageSize);
};

export const expertChatRowCount = (pageSize: ExpertChatPageSize): number =>
  Math.ceil(
    normalizeExpertChatPageSize(pageSize) / expertChatColumnCount(pageSize),
  );

export type ExpertChatGridDimensions = {
  columns: number;
  rows: number;
};

/**
 * La grille depend toujours des chats reellement visibles. Une derniere page
 * incomplete ne conserve donc pas les cases vides de la taille demandee.
 */
export const expertChatGridDimensions = (
  visibleChatCount: number,
  mode: ExpertChatPageSizeMode,
): ExpertChatGridDimensions => {
  const count = Math.min(resolveExpertChatPageSize(mode), normalizeCount(visibleChatCount));
  if (count <= 1) return { columns: 1, rows: 1 };

  const columns = expertChatColumnCount(count);
  return { columns, rows: Math.ceil(count / columns) };
};

const normalizedAvailableSize = (value: number): number =>
  Number.isFinite(value) ? Math.max(1, value) : 1;

const fittedTrackCount = (
  availableSize: number,
  minimumTrackSize: number,
  gap: number,
): number => Math.max(
  1,
  Math.floor((normalizedAvailableSize(availableSize) + gap) / (minimumTrackSize + gap)),
);

/**
 * Nombre de chats que le mode automatique peut afficher sans comprimer un
 * panneau sous sa taille utilisable. Les dimensions reçues correspondent à la
 * zone intérieure du mur, après retrait du bandeau, de la sidebar et du
 * padding de la grille.
 */
export const expertChatResponsiveCapacity = (
  availableWidth: number,
  availableHeight: number,
): ExpertChatPageSize => {
  const columns = fittedTrackCount(
    availableWidth,
    EXPERT_CHAT_MIN_PANE_WIDTH,
    EXPERT_CHAT_GRID_GAP,
  );
  const rows = fittedTrackCount(
    availableHeight,
    EXPERT_CHAT_MIN_PANE_HEIGHT,
    EXPERT_CHAT_GRID_GAP,
  );
  return Math.min(EXPERT_CHAT_AUTO_MAX_PAGE_SIZE, columns * rows);
};

/**
 * Choisit la forme de grille la plus lisible dans la zone disponible.
 *
 * Si une taille manuelle force davantage de chats que la capacité de l'écran,
 * on conserve tous les chats demandés mais on ajoute des lignes scrollables :
 * leur contenu n'est donc jamais réduit ni rogné.
 */
export const expertChatResponsiveGridDimensions = (
  visibleChatCount: number,
  availableWidth: number,
  availableHeight: number,
): ExpertChatGridDimensions => {
  const count = normalizeCount(visibleChatCount);
  if (count <= 1) return { columns: 1, rows: 1 };

  const width = normalizedAvailableSize(availableWidth);
  const height = normalizedAvailableSize(availableHeight);
  const maxColumns = Math.min(
    count,
    fittedTrackCount(width, EXPERT_CHAT_MIN_PANE_WIDTH, EXPERT_CHAT_GRID_GAP),
  );
  const maxRows = fittedTrackCount(
    height,
    EXPERT_CHAT_MIN_PANE_HEIGHT,
    EXPERT_CHAT_GRID_GAP,
  );

  if (count > maxColumns * maxRows) {
    return {
      columns: maxColumns,
      rows: Math.ceil(count / maxColumns),
    };
  }

  const targetAspectRatio = 1.35;
  let best: (ExpertChatGridDimensions & { score: number }) | null = null;
  for (let columns = 1; columns <= maxColumns; columns += 1) {
    const rows = Math.ceil(count / columns);
    if (rows > maxRows) continue;

    const cellWidth = (width - EXPERT_CHAT_GRID_GAP * (columns - 1)) / columns;
    const cellHeight = (height - EXPERT_CHAT_GRID_GAP * (rows - 1)) / rows;
    const aspectRatio = Math.max(Number.EPSILON, cellWidth / cellHeight);
    const emptyCellRatio = (columns * rows - count) / (columns * rows);
    const score = Math.abs(Math.log(aspectRatio / targetAspectRatio))
      + emptyCellRatio * 0.85;

    if (!best || score < best.score) best = { columns, rows, score };
  }

  return best
    ? { columns: best.columns, rows: best.rows }
    : { columns: maxColumns, rows: Math.ceil(count / maxColumns) };
};
