import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  DEFAULT_KEYBOARD_SHORTCUTS,
  KEYBOARD_SHORTCUT_DEFINITIONS,
  KEYBOARD_SHORTCUT_STORAGE_KEY,
  formatKeyboardShortcut,
  keyboardShortcutConflict,
  keyboardShortcutFromEvent,
  keyboardShortcutMatches,
  keyboardShortcutTargetIsTerminal,
  loadKeyboardShortcutOverrides,
  normalizeKeyboardShortcutBinding,
  persistKeyboardShortcutOverrides,
  resolveKeyboardShortcuts,
  withKeyboardShortcutOverride,
} from "../src/keyboard-shortcuts.ts";
import { chatHoverShortcutAction } from "../src/chat/shortcuts.ts";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");

const keyEvent = (overrides = {}) => ({
  key: "n",
  code: "KeyN",
  repeat: false,
  isComposing: false,
  shiftKey: false,
  ctrlKey: false,
  altKey: false,
  metaKey: false,
  getModifierState: () => false,
  ...overrides,
});

test("le registre expose les raccourcis existants et les nouvelles actions", () => {
  assert.equal(KEYBOARD_SHORTCUT_DEFINITIONS.length, 13);
  assert.equal(DEFAULT_KEYBOARD_SHORTCUTS["toggle-pane-fullscreen"], "1");
  assert.equal(DEFAULT_KEYBOARD_SHORTCUTS["close-chat"], "2");
  assert.equal(DEFAULT_KEYBOARD_SHORTCUTS["close-chat-and-discussion"], "Delete");
  assert.equal(DEFAULT_KEYBOARD_SHORTCUTS["new-chat"], "Mod+N");
  assert.equal(DEFAULT_KEYBOARD_SHORTCUTS["open-settings"], "Mod+Comma");
  assert.equal(DEFAULT_KEYBOARD_SHORTCUTS["toggle-sidebar"], "Mod+B");
  assert.equal(new Set(Object.values(DEFAULT_KEYBOARD_SHORTCUTS)).size, 13);
});

test("les combinaisons sont normalisees et affichees selon la plateforme", () => {
  assert.equal(normalizeKeyboardShortcutBinding("ctrl + shift + n"), "Mod+Shift+N");
  assert.equal(normalizeKeyboardShortcutBinding("Cmd+,"), "Mod+Comma");
  assert.equal(normalizeKeyboardShortcutBinding("Alt+ArrowUp"), "Alt+ArrowUp");
  assert.equal(normalizeKeyboardShortcutBinding("Ctrl+Alt"), null);
  assert.equal(normalizeKeyboardShortcutBinding("Ctrl+A+B"), null);
  assert.equal(formatKeyboardShortcut("Mod+Shift+N", false), "Ctrl + Maj + N");
  assert.equal(formatKeyboardShortcut("Mod+Shift+N", true), "⌘ + ⇧ + N");
  assert.equal(formatKeyboardShortcut(""), "Désactivé");
});

test("Ctrl et Commande déclenchent le même raccourci portable", () => {
  assert.equal(keyboardShortcutFromEvent(keyEvent({ ctrlKey: true })), "Mod+N");
  assert.equal(keyboardShortcutFromEvent(keyEvent({ metaKey: true })), "Mod+N");
  assert.equal(keyboardShortcutMatches(keyEvent({ ctrlKey: true }), "Mod+N"), true);
  assert.equal(keyboardShortcutMatches(keyEvent({ metaKey: true }), "Mod+N"), true);
  assert.equal(keyboardShortcutMatches(keyEvent({ ctrlKey: true, repeat: true }), "Mod+N"), false);
});

test("un terminal xterm garde la priorite sur les raccourcis globaux", () => {
  const terminalTarget = {
    closest: (selector) => selector.includes(".xterm") ? {} : null,
  };
  const regularTarget = { closest: () => null };
  assert.equal(keyboardShortcutTargetIsTerminal(terminalTarget), true);
  assert.equal(keyboardShortcutTargetIsTerminal(regularTarget), false);
  assert.match(main, /if \(keyboardShortcutTargetIsTerminal\(event\.target\)\) return;/);
});

test("le raccourci accent grave reste compatible avec les claviers AZERTY", () => {
  const azertyBackquote = keyEvent({
    key: "Dead",
    code: "Digit7",
    ctrlKey: true,
    altKey: true,
    getModifierState: (modifier) => modifier === "AltGraph",
  });
  assert.equal(keyboardShortcutFromEvent(azertyBackquote), "Backquote");
  assert.equal(keyboardShortcutMatches(azertyBackquote, "Backquote"), true);
});

test("les touches 1 et 2 restent directes sur un clavier AZERTY", () => {
  assert.equal(
    keyboardShortcutFromEvent(keyEvent({ key: "&", code: "Digit1" })),
    "1",
  );
  assert.equal(
    keyboardShortcutFromEvent(keyEvent({ key: "é", code: "Digit2" })),
    "2",
  );
});

test("les conflits sont refusés et une touche libérée peut être réutilisée", () => {
  assert.equal(
    keyboardShortcutConflict("new-chat", "2", {})?.id,
    "close-chat",
  );
  assert.equal(withKeyboardShortcutOverride({}, "new-chat", "2"), null);

  const movedClose = withKeyboardShortcutOverride({}, "close-chat", "Alt+W");
  assert.ok(movedClose);
  const reusedTwo = withKeyboardShortcutOverride(movedClose, "new-chat", "2");
  assert.ok(reusedTwo);
  const resolved = resolveKeyboardShortcuts(reusedTwo);
  assert.equal(resolved["close-chat"], "Alt+W");
  assert.equal(resolved["new-chat"], "2");
  assert.equal(
    withKeyboardShortcutOverride(reusedTwo, "close-chat", "2"),
    null,
    "la réinitialisation individuelle ne doit pas recréer un doublon",
  );
});

test("un raccourci peut être désactivé puis restauré", () => {
  const disabled = withKeyboardShortcutOverride({}, "close-chat", "");
  assert.ok(disabled);
  assert.equal(resolveKeyboardShortcuts(disabled)["close-chat"], "");
  const restored = withKeyboardShortcutOverride(disabled, "close-chat", "2");
  assert.deepEqual(restored, {});
});

test("les préférences locales sont filtrées et persistées", () => {
  const values = new Map([
    [
      KEYBOARD_SHORTCUT_STORAGE_KEY,
      JSON.stringify({
        "close-chat": "Alt+W",
        "open-settings": "not-a-key",
        unknown: "Mod+P",
      }),
    ],
  ]);
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
  };
  assert.deepEqual(loadKeyboardShortcutOverrides(storage), { "close-chat": "Alt+W" });
  assert.equal(persistKeyboardShortcutOverrides({ "close-chat": "" }, storage), true);
  assert.deepEqual(JSON.parse(values.get(KEYBOARD_SHORTCUT_STORAGE_KEY)), {
    "close-chat": "",
  });
});

test("un raccourci de chat personnalisé respecte encore les champs de saisie", () => {
  const editable = {
    tagName: "TEXTAREA",
    value: "brouillon",
    selectionStart: 4,
    selectionEnd: 4,
    closest: () => editable,
  };
  const bindings = {
    "close-chat": "Mod+W",
    "close-chat-and-discussion": "Mod+Delete",
  };
  assert.equal(
    chatHoverShortcutAction(keyEvent({ key: "w", code: "KeyW", ctrlKey: true, target: editable }), bindings),
    "close-chat",
  );
  assert.equal(
    chatHoverShortcutAction(keyEvent({ key: "w", code: "KeyW", target: editable }), bindings),
    null,
  );
});

test("les paramètres rendent l'éditeur et les handlers utilisent le registre", () => {
  assert.match(main, /const renderKeyboardShortcutSettings = \(\): string =>/);
  assert.match(main, /data-record-keyboard-shortcut=/);
  assert.match(main, /data-clear-keyboard-shortcut=/);
  assert.match(main, /id="resetKeyboardShortcuts"/);
  assert.match(main, /keyboardShortcutMatchesAction\("new-chat", event\)/);
  assert.match(main, /keyboardShortcutMatchesAction\("toggle-sidebar", event\)/);
  assert.match(main, /keyboardShortcutMatchesAction\("toggle-environments", event\)/);
  assert.match(main, /keyboardShortcutBinding\("close-chat"\)/);
});
