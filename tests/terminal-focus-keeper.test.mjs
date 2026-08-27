import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");
const theme = readFileSync(new URL("../src/theme.css", import.meta.url), "utf8");
const transport = readFileSync(new URL("../src/terminal-transport.ts", import.meta.url), "utf8");

const keeperStart = main.indexOf(
  "const flushTerminalInput = (session: TerminalSession): void => {",
);
const mountStart = main.indexOf("const mountExpertTerminals = () => {", keeperStart);
const keeper = main.slice(keeperStart, mountStart);

test("le verrou clavier est une couche ajoutee, pas un remplacement", () => {
  assert.ok(keeperStart > 0, "bloc verrou clavier manquant");
  // Protections conservees du correctif initial (docs/terminal-focus-vps-fix.md).
  assert.match(keeper, /const flushTerminalInput = \(session: TerminalSession\): void => \{/);
  assert.match(keeper, /terminalPendingInput\.get\(session\.key\)/);
  assert.match(keeper, /write_terminal", \{ id: session\.ptyId, data: chunks\.join\(""\) \}/);
});

test("le chien de garde reprend le focus sans voler un champ editable", () => {
  assert.match(keeper, /TERMINAL_KEYBOARD_WATCHDOG_MS = 500/);
  const watchdogStart = keeper.indexOf("const ensureTerminalKeyboardWatchdog");
  const watchdog = keeper.slice(watchdogStart);
  assert.match(watchdog, /window\.setInterval/);
  assert.match(watchdog, /activeView === "terminal"/);
  assert.match(watchdog, /document\.hasFocus\(\)/);
  assert.match(watchdog, /terminalKeyboardOverlayOpen\(\)/);
  assert.match(watchdog, /document\.activeElement !== session\.terminal\.textarea/);
  assert.match(watchdog, /!activeElementBlocksTerminalFocus\(\)/);
  assert.match(watchdog, /focusTerminalKeyboard\(session\)/);
});

test("l'intercepteur re-delivre la premiere frappe a xterm", () => {
  const predicateStart = keeper.indexOf("const terminalKeyboardBlockedTarget");
  const interceptorEnd = keeper.indexOf("const lastTerminalKeyboardBadgeState", predicateStart);
  const interceptor = keeper.slice(predicateStart, interceptorEnd);
  assert.match(interceptor, /if \(!event\.isTrusted \|\| event\.isComposing \|\| event\.metaKey\) return false;/);
  assert.match(interceptor, /TERMINAL_KEYBOARD_INTERCEPT_SKIP_KEYS\.has\(event\.key\)/);
  assert.match(interceptor, /target\.closest\("\.xterm, \[data-terminal-host\]"\)/);
  assert.match(interceptor, /input, textarea, select, button, a, label, summary, option/);
  assert.match(interceptor, /event\.preventDefault\(\);\s*event\.stopImmediatePropagation\(\);/);
  assert.match(interceptor, /focusTerminalKeyboard\(session\);/);
  assert.match(interceptor, /new KeyboardEvent\("keydown", \{/);
  assert.match(interceptor, /textarea\.dispatchEvent\(/);
  assert.doesNotMatch(interceptor, /activeView !== "terminal"[\s\S]*return true/);
});

test("les raccourcis applicatifs restent prioritaires sur la re-delivrance", () => {
  const eventsStart = main.indexOf("const setupEvents = async");
  const events = main.slice(eventsStart);
  const interceptorIndex = events.indexOf(
    'window.addEventListener("keydown", interceptTerminalKeyboard, true);',
  );
  assert.ok(interceptorIndex > 0, "intercepteur non enregistre dans setupEvents");
  const f11Index = events.indexOf('if (event.key !== "F11") return;');
  assert.ok(
    f11Index >= 0 && f11Index < interceptorIndex,
    "l'echappement global des modales doit etre enregistre avant l'intercepteur",
  );
  assert.match(
    events,
    /addEventListener\("focus", \(\) => \{\s*if \(activeView === "terminal"\) void recoverActiveTerminalInput\(\);\s*\}\)/,
  );
  assert.match(events, /ensureTerminalKeyboardWatchdog\(\);/);
  assert.match(events, /ensureTerminalInputQueueGuard\(\);/);
});

test("la file de saisie ne reste jamais bloquee", () => {
  assert.match(keeper, /TERMINAL_INPUT_QUEUE_GUARD_MS = 2_000/);
  const guardStart = keeper.indexOf("const ensureTerminalInputQueueGuard");
  const guard = keeper.slice(guardStart);
  assert.match(guard, /if \(terminalPendingInput\.size === 0\) return;/);
  assert.match(
    guard,
    /if \(session\.running && session\.ptyId !== null\) flushTerminalInput\(session\);/,
  );

  const attachStart = main.indexOf("const attachRestoredTerminal = async (");
  const attachEnd = main.indexOf("const restoreTerminals = async () =>", attachStart);
  const attach = main.slice(attachStart, attachEnd);
  assert.match(attach, /session\.status = "Rattache";\s*flushTerminalInput\(session\);/);

  const writeStart = platform.indexOf("function writeRemoteTerminal");
  const writeEnd = platform.indexOf("function resizeRemoteTerminal", writeStart);
  const write = platform.slice(writeStart, writeEnd);
  assert.match(write, /queuePendingTerminalInput\(id, data\);\s*if \(\/session terminal introuvable/);
});

test("le badge montre l'etat reel du terminal", () => {
  assert.match(keeper, /terminalKeyboardBadgeState = \(/);
  for (const label of ["Inactif", "Reconnexion", "PTY perdu", "Clavier"]) {
    assert.match(keeper, new RegExp(`label: "${label}"`));
  }
  assert.match(keeper, /label: `File \$\{pending\}`/);
  assert.match(keeper, /terminalTransportState\(session\.ptyId\)/);
  assert.match(keeper, /terminalRemotePendingInputChars\(session\.ptyId\)/);
  assert.match(keeper, /document\.activeElement === textarea && document\.hasFocus\(\)/);
  assert.match(keeper, /updateTerminalKeyboardBadges = \(\): void => \{/);

  const paneStart = main.indexOf("const renderExpertTerminalPane = (");
  const paneEnd = main.indexOf("const renderExpertTerminalGrid = () =>", paneStart);
  const pane = main.slice(paneStart, paneEnd);
  assert.match(pane, /data-terminal-keyboard="\$\{escapeAttr\(session\.key\)\}"/);
  assert.match(pane, /data-terminal-keyboard-label/);
  assert.match(pane, /data-terminal-keyboard-tone="off"/);

  const wireStart = main.indexOf('document.querySelectorAll<HTMLButtonElement>("[data-terminal-keyboard]")');
  const wire = main.slice(wireStart, main.indexOf("document.querySelectorAll<HTMLFormElement>", wireStart));
  assert.match(wire, /focusExpertSession\(session, true\)/);
  assert.match(wire, /void recoverActiveTerminalInput\(\);/);

  assert.match(style, /\.expert-terminal-keyboard-badge/);
  assert.match(style, /\[data-terminal-keyboard-tone="ok"\]/);
  assert.match(style, /\[data-terminal-keyboard-tone="warn"\]/);
  assert.match(theme, /light"\] \.expert-terminal-keyboard-badge/);
});

test("l'etat de transport distant est expose pour le badge", () => {
  assert.match(platform, /export type TerminalTransportState =/);
  assert.match(platform, /export const terminalTransportState = \(id: number\)/);
  const stateStart = platform.indexOf("export const terminalTransportState");
  const state = platform.slice(stateStart, platform.indexOf("/** Caracteres bufferises", stateStart));
  assert.match(state, /remoteStoppingTerminals\.has\(id\)/);
  assert.match(state, /remoteEndedTerminals\.has\(id\)/);
  assert.match(state, /remoteStartingTerminals\.has\(id\)/);
  assert.match(state, /socket\?\.readyState === WebSocket\.OPEN/);
  assert.match(platform, /export const terminalRemotePendingInputChars = \(id: number\)/);
  assert.match(transport, /size\(id: number\)/);
});

test("la fermeture d'un terminal nettoie le badge correspondant", () => {
  const closeStart = main.indexOf("const closeTerminalSession = async (key: string) =>");
  const closeEnd = main.indexOf("const sendLine = async", closeStart);
  const close = main.slice(closeStart, closeEnd);
  assert.match(close, /terminalPendingInput\.delete\(key\);\s*lastTerminalKeyboardBadgeState\.delete\(key\);/);
});
