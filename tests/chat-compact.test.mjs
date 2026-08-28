import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const source = (path) => readFileSync(new URL(path, import.meta.url), "utf8");

test("/compact utilise le protocole app-server Codex et remonte la fenetre de contexte", () => {
  const chat = source("../src-tauri/src/chat.rs");
  const discussions = source("../src-tauri/src/discussions.rs");

  assert.match(chat, /"thread\/compact\/start"/);
  assert.match(chat, /"experimentalApi": true/);
  assert.match(chat, /thread\/tokenUsage\/updated/);
  assert.match(chat, /pub async fn compact_chat_session/);
  assert.match(discussions, /pub struct DiscussionContextUsage/);
  assert.match(discussions, /CODEX_CONTEXT_BASELINE_TOKENS/);
  assert.match(discussions, /context_usage: Option<DiscussionContextUsage>/);
});

test("la commande est disponible sur desktop et serveur distant", () => {
  const lib = source("../src-tauri/src/lib.rs");
  const server = source("../src-tauri/src/server.rs");
  const platform = source("../src/platform.ts");

  assert.match(lib, /chat::compact_chat_session/);
  assert.match(server, /\.route\("\/chat\/compact", post\(api_compact_chat_session\)\)/);
  assert.match(platform, /case "compact_chat_session"/);
  assert.match(platform, /"POST", "\/api\/chat\/compact"/);
});

test("les deux interfaces interceptent /compact et exposent les trois niveaux", () => {
  const main = source("../src/main.ts");
  const view = source("../src/chat/view.ts");
  const style = source("../src/style.css");

  assert.equal([...main.matchAll(/isCompactSlashCommand\(prompt\)/g)].length, 2);
  assert.match(main, /compactCurrentChatContext/);
  assert.match(main, /compactExpertChatContext/);
  const mainSend = main.indexOf("const sendChatMessage");
  const mainCompact = main.indexOf("isCompactSlashCommand(prompt)", mainSend);
  const mainPreferences = main.indexOf("const preferences", mainSend);
  assert.ok(mainSend >= 0 && mainCompact > mainSend && mainCompact < mainPreferences);
  const expertSend = main.indexOf("const sendExpertChatMessage");
  const expertCompact = main.indexOf("isCompactSlashCommand(prompt)", expertSend);
  const expertPreferences = main.indexOf("const preferences", expertSend);
  assert.ok(expertSend >= 0 && expertCompact > expertSend && expertCompact < expertPreferences);
  assert.match(view, /pressure-\$\{usage\.pressure\}/);
  assert.match(style, /\.chat-token-usage\.pressure-safe/);
  assert.match(style, /\.chat-token-usage\.pressure-warning/);
  assert.match(style, /\.chat-token-usage\.pressure-danger/);
});

test("les deux interfaces compactent automatiquement le contexte au-dela de 400k tokens", () => {
  const main = source("../src/main.ts");
  const tokenUsage = source("../src/chat/token-usage.ts");

  assert.match(tokenUsage, /CHAT_AUTO_COMPACT_THRESHOLD_TOKENS = 400_000/);
  assert.match(tokenUsage, /usedTokens > CHAT_AUTO_COMPACT_THRESHOLD_TOKENS/);
  assert.match(main, /maybeAutoCompactCurrentChatContext/);
  assert.match(main, /maybeAutoCompactExpertChatContext/);
  assert.equal([...main.matchAll(/compact(?:Current|Expert)ChatContext\([^\n]*false, true\)/g)].length, 2);
  assert.match(main, /automaticCompactionAttempts/);
});
