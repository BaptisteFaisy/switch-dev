export type KeyboardShortcutId =
  | "new-chat"
  | "search-discussions"
  | "open-discussions"
  | "open-settings"
  | "toggle-sidebar"
  | "toggle-environments"
  | "new-terminal"
  | "toggle-pane-fullscreen"
  | "close-chat"
  | "close-chat-and-discussion"
  | "maximize-terminal"
  | "restore-terminal"
  | "close-terminal";

export type KeyboardShortcutGroup = "general" | "chat" | "terminal";

export type KeyboardShortcutDefinition = {
  id: KeyboardShortcutId;
  group: KeyboardShortcutGroup;
  label: string;
  description: string;
  defaultBinding: string;
};

export const KEYBOARD_SHORTCUT_GROUPS: ReadonlyArray<{
  id: KeyboardShortcutGroup;
  label: string;
}> = [
  { id: "general", label: "Général" },
  { id: "chat", label: "Chats" },
  { id: "terminal", label: "Terminaux" },
];

export const KEYBOARD_SHORTCUT_DEFINITIONS: readonly KeyboardShortcutDefinition[] = [
  {
    id: "new-chat",
    group: "general",
    label: "Nouveau chat",
    description: "Ouvrir la création d’un chat dans l’environnement actif.",
    defaultBinding: "Mod+N",
  },
  {
    id: "search-discussions",
    group: "general",
    label: "Rechercher une discussion",
    description: "Afficher la barre latérale et placer le curseur dans la recherche.",
    defaultBinding: "Mod+K",
  },
  {
    id: "open-discussions",
    group: "general",
    label: "Ouvrir l’historique",
    description: "Ouvrir l’historique qui permet de reprendre une discussion.",
    defaultBinding: "Mod+Shift+K",
  },
  {
    id: "open-settings",
    group: "general",
    label: "Paramètres",
    description: "Ouvrir directement les paramètres de l’application.",
    defaultBinding: "Mod+Comma",
  },
  {
    id: "toggle-sidebar",
    group: "general",
    label: "Colonne de gauche",
    description: "Masquer ou réafficher la barre latérale des conversations.",
    defaultBinding: "Mod+B",
  },
  {
    id: "toggle-environments",
    group: "general",
    label: "Changer d’environnement",
    description: "Ouvrir ou fermer le sélecteur d’environnements.",
    defaultBinding: "Backquote",
  },
  {
    id: "new-terminal",
    group: "general",
    label: "Nouveau terminal",
    description: "Ouvrir la création d’un terminal dans l’environnement actif.",
    defaultBinding: "Mod+Alt+N",
  },
  {
    id: "toggle-pane-fullscreen",
    group: "chat",
    label: "Agrandir ou réduire le panneau survolé",
    description: "Basculer le chat ou le terminal placé sous la souris en plein écran.",
    defaultBinding: "1",
  },
  {
    id: "close-chat",
    group: "chat",
    label: "Fermer le chat survolé",
    description: "Retirer le panneau en conservant sa discussion dans l’historique.",
    defaultBinding: "2",
  },
  {
    id: "close-chat-and-discussion",
    group: "chat",
    label: "Fermer le chat et sa discussion",
    description: "Retirer le panneau et archiver la discussion associée.",
    defaultBinding: "Delete",
  },
  {
    id: "maximize-terminal",
    group: "terminal",
    label: "Agrandir le terminal survolé",
    description: "Passer le terminal placé sous la souris en plein écran.",
    defaultBinding: "ArrowUp",
  },
  {
    id: "restore-terminal",
    group: "terminal",
    label: "Réduire le terminal survolé",
    description: "Quitter le plein écran pour revenir à la grille.",
    defaultBinding: "ArrowDown",
  },
  {
    id: "close-terminal",
    group: "terminal",
    label: "Fermer le terminal survolé",
    description: "Fermer la session terminal placée sous la souris.",
    defaultBinding: "ArrowRight",
  },
];

export type KeyboardShortcutOverrides = Partial<Record<KeyboardShortcutId, string>>;
export type ResolvedKeyboardShortcuts = Record<KeyboardShortcutId, string>;
export type KeyboardShortcutStorage = Pick<Storage, "getItem" | "setItem">;

export type KeyboardShortcutEvent = Pick<
  KeyboardEvent,
  | "key"
  | "code"
  | "repeat"
  | "isComposing"
  | "shiftKey"
  | "ctrlKey"
  | "altKey"
  | "metaKey"
> & Partial<Pick<KeyboardEvent, "getModifierState">>;

type KeyboardShortcutTarget = {
  closest?: (selector: string) => unknown;
};

export const keyboardShortcutTargetIsTerminal = (target: unknown): boolean => {
  if (!target || typeof target !== "object") return false;
  const candidate = target as KeyboardShortcutTarget;
  return typeof candidate.closest === "function"
    && Boolean(candidate.closest(".xterm, [data-terminal-host]"));
};

export const KEYBOARD_SHORTCUT_STORAGE_KEY = "codex-switch-terminal.keyboard-shortcuts.v1";

const definitionById = new Map(
  KEYBOARD_SHORTCUT_DEFINITIONS.map((definition) => [definition.id, definition]),
);
const shortcutIds = new Set<KeyboardShortcutId>(
  KEYBOARD_SHORTCUT_DEFINITIONS.map((definition) => definition.id),
);

const modifierAliases: Record<string, "Mod" | "Alt" | "Shift"> = {
  mod: "Mod",
  command: "Mod",
  cmd: "Mod",
  meta: "Mod",
  control: "Mod",
  ctrl: "Mod",
  alt: "Alt",
  option: "Alt",
  shift: "Shift",
};

const keyAliases: Record<string, string> = {
  " ": "Space",
  space: "Space",
  spacebar: "Space",
  backspace: "Backspace",
  delete: "Delete",
  del: "Delete",
  insert: "Insert",
  ins: "Insert",
  enter: "Enter",
  return: "Enter",
  tab: "Tab",
  escape: "Escape",
  esc: "Escape",
  home: "Home",
  end: "End",
  pageup: "PageUp",
  pagedown: "PageDown",
  arrowup: "ArrowUp",
  up: "ArrowUp",
  arrowdown: "ArrowDown",
  down: "ArrowDown",
  arrowleft: "ArrowLeft",
  left: "ArrowLeft",
  arrowright: "ArrowRight",
  right: "ArrowRight",
  backquote: "Backquote",
  "`": "Backquote",
  "~": "Backquote",
  comma: "Comma",
  ",": "Comma",
  "<": "Comma",
  period: "Period",
  ".": "Period",
  ">": "Period",
  slash: "Slash",
  "/": "Slash",
  "?": "Slash",
  semicolon: "Semicolon",
  ";": "Semicolon",
  ":": "Semicolon",
  quote: "Quote",
  "'": "Quote",
  "\"": "Quote",
  bracketleft: "BracketLeft",
  "[": "BracketLeft",
  "{": "BracketLeft",
  bracketright: "BracketRight",
  "]": "BracketRight",
  "}": "BracketRight",
  backslash: "Backslash",
  "\\": "Backslash",
  "|": "Backslash",
  minus: "Minus",
  "-": "Minus",
  "_": "Minus",
  equal: "Equal",
  "=": "Equal",
  "+": "Equal",
};

const codeAliases: Record<string, string> = {
  Space: "Space",
  Backspace: "Backspace",
  Delete: "Delete",
  Insert: "Insert",
  Enter: "Enter",
  NumpadEnter: "Enter",
  Tab: "Tab",
  Escape: "Escape",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  ArrowUp: "ArrowUp",
  ArrowDown: "ArrowDown",
  ArrowLeft: "ArrowLeft",
  ArrowRight: "ArrowRight",
  Backquote: "Backquote",
  Comma: "Comma",
  Period: "Period",
  Slash: "Slash",
  Semicolon: "Semicolon",
  Quote: "Quote",
  BracketLeft: "BracketLeft",
  BracketRight: "BracketRight",
  Backslash: "Backslash",
  Minus: "Minus",
  Equal: "Equal",
};

const isKeyboardShortcutId = (value: string): value is KeyboardShortcutId =>
  shortcutIds.has(value as KeyboardShortcutId);

const canonicalKeyToken = (value: string): string | null => {
  if (value === " ") return "Space";
  const token = value.trim();
  if (!token) return null;
  if (/^[a-z]$/i.test(token)) return token.toUpperCase();
  if (/^[0-9]$/.test(token)) return token;
  if (/^f(?:[1-9]|1[0-9]|2[0-4])$/i.test(token)) return token.toUpperCase();
  if (/^numpad[0-9]$/i.test(token)) {
    return `Numpad${token.slice(-1)}`;
  }
  return keyAliases[token.toLocaleLowerCase()] ?? null;
};

export const normalizeKeyboardShortcutBinding = (
  value: unknown,
): string | null => {
  if (typeof value !== "string") return null;
  if (value === "") return "";
  const tokens = value.split("+").map((token) => token.trim()).filter(Boolean);
  if (!tokens.length) return null;

  const modifiers = new Set<"Mod" | "Alt" | "Shift">();
  let key: string | null = null;
  for (const token of tokens) {
    const modifier = modifierAliases[token.toLocaleLowerCase()];
    if (modifier) {
      modifiers.add(modifier);
      continue;
    }
    const candidate = canonicalKeyToken(token);
    if (!candidate || key) return null;
    key = candidate;
  }
  if (!key) return null;

  return [
    modifiers.has("Mod") ? "Mod" : "",
    modifiers.has("Alt") ? "Alt" : "",
    modifiers.has("Shift") ? "Shift" : "",
    key,
  ].filter(Boolean).join("+");
};

const eventUsesAltGraph = (event: KeyboardShortcutEvent): boolean => {
  try {
    return event.getModifierState?.("AltGraph") === true;
  } catch {
    return false;
  }
};

const keyTokenFromEvent = (event: KeyboardShortcutEvent): string | null => {
  const altGraph = eventUsesAltGraph(event);
  if (
    (event.key === "Dead" || event.key === "`") &&
    event.code === "Digit7" &&
    (event.altKey || altGraph)
  ) {
    // Sur un clavier AZERTY, le caractère ` est produit avec AltGr + 7. Il
    // reste traité comme la touche logique ` afin que le réglage soit portable.
    return "Backquote";
  }

  if (
    event.key === "Control" ||
    event.key === "Shift" ||
    event.key === "Alt" ||
    event.key === "Meta" ||
    event.key === "OS" ||
    event.key === "AltGraph"
  ) {
    return null;
  }

  const fromKey = canonicalKeyToken(event.key);
  if (fromKey) return fromKey;
  if (/^Key[A-Z]$/.test(event.code)) return event.code.slice(3);
  if (/^Digit[0-9]$/.test(event.code)) return event.code.slice(5);
  if (/^Numpad[0-9]$/.test(event.code)) return event.code;
  if (/^F(?:[1-9]|1[0-9]|2[0-4])$/.test(event.code)) return event.code;
  return codeAliases[event.code] ?? null;
};

export const keyboardShortcutFromEvent = (
  event: KeyboardShortcutEvent,
): string | null => {
  if (event.repeat || event.isComposing) return null;
  const key = keyTokenFromEvent(event);
  if (!key) return null;

  const altGraph = eventUsesAltGraph(event);
  const azertyBackquote =
    key === "Backquote" &&
    event.code === "Digit7" &&
    (event.altKey || altGraph);
  return [
    !altGraph && !azertyBackquote && (event.ctrlKey || event.metaKey) ? "Mod" : "",
    !altGraph && !azertyBackquote && event.altKey ? "Alt" : "",
    !azertyBackquote && event.shiftKey ? "Shift" : "",
    key,
  ].filter(Boolean).join("+");
};

export const keyboardShortcutMatches = (
  event: KeyboardShortcutEvent,
  binding: string | null | undefined,
): boolean => {
  const normalizedBinding = normalizeKeyboardShortcutBinding(binding);
  if (!normalizedBinding) return false;
  return keyboardShortcutFromEvent(event) === normalizedBinding;
};

const defaultKeyboardShortcuts = (): ResolvedKeyboardShortcuts =>
  Object.fromEntries(
    KEYBOARD_SHORTCUT_DEFINITIONS.map((definition) => [
      definition.id,
      definition.defaultBinding,
    ]),
  ) as ResolvedKeyboardShortcuts;

export const DEFAULT_KEYBOARD_SHORTCUTS: Readonly<ResolvedKeyboardShortcuts> =
  Object.freeze(defaultKeyboardShortcuts());

const normalizeKeyboardShortcutOverridesLoose = (
  value: unknown,
): KeyboardShortcutOverrides => {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const candidate = value as Record<string, unknown>;
  const normalized: KeyboardShortcutOverrides = {};
  for (const [id, rawBinding] of Object.entries(candidate)) {
    if (!isKeyboardShortcutId(id)) continue;
    const binding = normalizeKeyboardShortcutBinding(rawBinding);
    if (binding === null) continue;
    if (binding === DEFAULT_KEYBOARD_SHORTCUTS[id]) continue;
    normalized[id] = binding;
  }
  return normalized;
};

const resolveKeyboardShortcutsLoose = (
  overrides: KeyboardShortcutOverrides,
): ResolvedKeyboardShortcuts => {
  const resolved = defaultKeyboardShortcuts();
  KEYBOARD_SHORTCUT_DEFINITIONS.forEach((definition) => {
    const binding = overrides[definition.id];
    if (binding !== undefined) resolved[definition.id] = binding;
  });

  // Les changements sont appliqués comme un ensemble afin de permettre de
  // réutiliser la combinaison par défaut d'une action qui vient d'être déplacée.
  // Une préférence stockée à la main qui contiendrait encore un doublon perd
  // seulement la seconde action, plutôt que de déclencher les deux commandes.
  const used = new Set<string>();
  KEYBOARD_SHORTCUT_DEFINITIONS.forEach((definition) => {
    const binding = resolved[definition.id];
    if (!binding) return;
    if (used.has(binding)) resolved[definition.id] = "";
    else used.add(binding);
  });
  return resolved;
};

export const normalizeKeyboardShortcutOverrides = (
  value: unknown,
): KeyboardShortcutOverrides => {
  const loose = normalizeKeyboardShortcutOverridesLoose(value);
  const resolved = resolveKeyboardShortcutsLoose(loose);
  const normalized: KeyboardShortcutOverrides = {};
  KEYBOARD_SHORTCUT_DEFINITIONS.forEach((definition) => {
    const override = loose[definition.id];
    if (override === "") {
      normalized[definition.id] = "";
    } else if (override && resolved[definition.id] === override) {
      normalized[definition.id] = override;
    }
  });
  return normalized;
};

export const resolveKeyboardShortcuts = (
  overrides: KeyboardShortcutOverrides | null | undefined,
): ResolvedKeyboardShortcuts =>
  resolveKeyboardShortcutsLoose(normalizeKeyboardShortcutOverrides(overrides));

export const keyboardShortcutConflict = (
  id: KeyboardShortcutId,
  binding: string,
  overrides: KeyboardShortcutOverrides,
): KeyboardShortcutDefinition | null => {
  const normalized = normalizeKeyboardShortcutBinding(binding);
  if (!normalized) return null;
  const resolved = resolveKeyboardShortcuts(overrides);
  return KEYBOARD_SHORTCUT_DEFINITIONS.find(
    (definition) => definition.id !== id && resolved[definition.id] === normalized,
  ) ?? null;
};

export const withKeyboardShortcutOverride = (
  overrides: KeyboardShortcutOverrides,
  id: KeyboardShortcutId,
  binding: string,
): KeyboardShortcutOverrides | null => {
  const normalized = normalizeKeyboardShortcutBinding(binding);
  if (normalized === null || keyboardShortcutConflict(id, normalized, overrides)) return null;
  const next = { ...normalizeKeyboardShortcutOverrides(overrides) };
  if (normalized === DEFAULT_KEYBOARD_SHORTCUTS[id]) delete next[id];
  else next[id] = normalized;
  return normalizeKeyboardShortcutOverrides(next);
};

export const loadKeyboardShortcutOverrides = (
  storage: Pick<KeyboardShortcutStorage, "getItem"> | null | undefined =
    typeof localStorage === "undefined" ? null : localStorage,
): KeyboardShortcutOverrides => {
  try {
    const stored = storage?.getItem(KEYBOARD_SHORTCUT_STORAGE_KEY);
    return stored ? normalizeKeyboardShortcutOverrides(JSON.parse(stored)) : {};
  } catch {
    return {};
  }
};

export const persistKeyboardShortcutOverrides = (
  overrides: KeyboardShortcutOverrides,
  storage: Pick<KeyboardShortcutStorage, "setItem"> | null | undefined =
    typeof localStorage === "undefined" ? null : localStorage,
): boolean => {
  try {
    if (!storage) return false;
    storage.setItem(
      KEYBOARD_SHORTCUT_STORAGE_KEY,
      JSON.stringify(normalizeKeyboardShortcutOverrides(overrides)),
    );
    return true;
  } catch {
    return false;
  }
};

const displayKey: Record<string, string> = {
  Space: "Espace",
  Backspace: "Retour arrière",
  Delete: "Suppr",
  Insert: "Inser",
  Escape: "Échap",
  PageUp: "Page préc.",
  PageDown: "Page suiv.",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  Backquote: "`",
  Comma: ",",
  Period: ".",
  Slash: "/",
  Semicolon: ";",
  Quote: "'",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Minus: "-",
  Equal: "=",
};

const currentPlatformIsMac = (): boolean =>
  typeof navigator !== "undefined" && /Mac|iPhone|iPad|iPod/i.test(navigator.platform);

export const keyboardShortcutDisplayParts = (
  binding: string | null | undefined,
  isMac = currentPlatformIsMac(),
): string[] => {
  const normalized = normalizeKeyboardShortcutBinding(binding);
  if (!normalized) return [];
  return normalized.split("+").map((token) => {
    if (token === "Mod") return isMac ? "⌘" : "Ctrl";
    if (token === "Alt") return isMac ? "⌥" : "Alt";
    if (token === "Shift") return isMac ? "⇧" : "Maj";
    return displayKey[token] ?? token;
  });
};

export const formatKeyboardShortcut = (
  binding: string | null | undefined,
  isMac = currentPlatformIsMac(),
): string => {
  const parts = keyboardShortcutDisplayParts(binding, isMac);
  return parts.length ? parts.join(" + ") : "Désactivé";
};

export const keyboardShortcutDefinition = (
  id: KeyboardShortcutId,
): KeyboardShortcutDefinition => definitionById.get(id)!;
