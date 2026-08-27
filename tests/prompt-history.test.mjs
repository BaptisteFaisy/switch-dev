import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  highlightPromptMatch,
  promptSessionsFromHistory,
  renderPromptHistoryPanel,
  renderPromptRows,
} from "../src/prompt-history.ts";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");
const theme = readFileSync(new URL("../src/theme.css", import.meta.url), "utf8");
const view = readFileSync(new URL("../src/prompt-history-view.ts", import.meta.url), "utf8");
const historyStyle = readFileSync(new URL("../src/prompt-history.css", import.meta.url), "utf8");
const smoke = readFileSync(new URL("../scripts/smoke-site.mjs", import.meta.url), "utf8");
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
const server = readFileSync(new URL("../src-tauri/src/server.rs", import.meta.url), "utf8");

const entry = (overrides = {}) => ({
  sessionId: "session-a",
  accountId: "account-a",
  accountLabel: "Compte A",
  codexHome: "/tmp/codex-a",
  filePath: "/tmp/session-a.jsonl",
  cwd: "/workspace/a",
  timestamp: 100,
  sessionTitle: "Premiere session",
  text: "Premier message",
  ...overrides,
});

const history = (prompts, overrides = {}) => ({
  generatedAt: 500,
  totalPrompts: prompts.length,
  returned: prompts.length,
  truncated: false,
  prompts,
  ...overrides,
});

const model = (value, loaded = true) => ({
  history: value,
  loaded,
  formatTimestamp: (timestamp) => `date-${timestamp}`,
  displayProjectDir: (projectDir) => `repo:${projectDir}`,
});

test("regroupe l'historique par compte et session sans doublon puis trie par activite", () => {
  const sessions = promptSessionsFromHistory(history([
    entry({ timestamp: 100, text: "ancien" }),
    entry({ timestamp: 200, text: "recent" }),
    entry({ timestamp: 200, text: "recent" }),
    entry({ accountId: "account-b", accountLabel: "Compte B", timestamp: 300, text: "autre" }),
  ]));

  assert.equal(sessions.length, 2);
  assert.equal(sessions[0].accountId, "account-b");
  assert.deepEqual(sessions[1].prompts.map((prompt) => prompt.text), ["ancien", "recent"]);
  assert.equal(sessions[1].firstTimestamp, 100);
  assert.equal(sessions[1].lastTimestamp, 200);
});

test("recherche, surligne et echappe les contenus controles par l'utilisateur", () => {
  assert.equal(
    highlightPromptMatch("<img src=x> & Beta [x]", "beta"),
    "&lt;img src=x&gt; &amp; <mark>Beta</mark> [x]",
  );
  assert.equal(highlightPromptMatch("avant [x] apres", "[x]"), "avant <mark>[x]</mark> apres");

  const value = history([
    entry({
      sessionId: 'session-" onclick="unsafe',
      sessionTitle: "Titre <script>",
      text: "Message & contenu",
    }),
  ]);
  const html = renderPromptRows(model(value), "contenu");
  assert.match(html, /Titre &lt;script&gt;/);
  assert.match(html, /Message &amp; <mark>contenu<\/mark>/);
  assert.match(html, /data-prompt-discussion="session-&quot; onclick=&quot;unsafe"/);
  assert.doesNotMatch(html, /<script>/);
});

test("conserve le panneau, ses compteurs, son chargement et sa limite de rendu", () => {
  const prompts = Array.from({ length: 401 }, (_, index) => entry({
    sessionId: `session-${index}`,
    timestamp: index + 1,
    text: `message-${index}`,
  }));
  const value = history(prompts, { returned: 401, truncated: true });
  const panel = renderPromptHistoryPanel(model(value));

  assert.match(panel, /401 chat\(s\) \/ terminal\(aux\) · 401 message\(s\) · 401 plus recentes indexees/);
  assert.match(panel, /id="promptSearch"/);
  assert.match(panel, /id="refreshPromptHistory"/);
  assert.match(panel, /Affichage limite a 400 sur 401 resultats/);
  assert.match(renderPromptRows(model(null, false)), /Lecture des demandes Codex/);
});

test("charge l'interface et ses styles uniquement a l'ouverture sans retarder la collecte", () => {
  assert.match(main, /promptHistoryViewModulePromise = import\("\.\/prompt-history-view"\)/);
  assert.match(
    main,
    /if \(view === "history" && !promptHistoryViewModule\) \{\s*if \(!promptHistoryLoaded\) void refreshPromptHistory\(\);\s*void loadPromptHistoryViewModule\(\)/,
  );
  assert.match(main, /if \(promptHistoryRefreshPromise\) return promptHistoryRefreshPromise/);
  assert.match(main, /promptHistoryViewModule\?\.renderPromptHistoryPanel\(promptHistoryPanelModel\(\)\)/);
  assert.match(main, /promptHistoryViewModule\?\.mountPromptHistoryPanel\(\{/);
  assert.doesNotMatch(main, /const renderPromptHistoryPanel|const promptSessions|const bindPromptRowUi/);

  assert.match(view, /import "\.\/prompt-history\.css"/);
  assert.match(view, /export \* from "\.\/prompt-history"/);
  assert.doesNotMatch(
    `${style}\n${theme}`,
    /#promptList|\.prompt-row|\.prompt-main|\.prompt-session-title|\.prompt-session-messages|\.prompt-message-index|\.prompt-text|\.prompt-meta|\.prompt-actions|\.prompt-more/,
  );
  assert.match(historyStyle, /#promptList/);
  assert.match(historyStyle, /@supports \(content-visibility: auto\)[\s\S]*?\.prompt-row/);
  assert.match(historyStyle, /@media \(max-width: 700px\)[\s\S]*?\.prompt-session-messages li/);
  assert.match(historyStyle, /:root\[data-theme="light"\] \.prompt-row/);
  assert.match(smoke, /CST_SMOKE_NAVIGATION_TARGET/);
});

test("le conteneur charge l'historique reel du serveur au lieu d'une liste vide", () => {
  assert.match(
    platform,
    /case "list_prompt_history":\s*return listRemotePromptHistory<T>\(args\.limit\)/,
  );
  assert.doesNotMatch(
    platform.match(/case "list_prompt_history":[\s\S]*?case "claim_session_for_terminal":/)?.[0] ?? "",
    /prompts:\s*\[\]/,
  );
  assert.match(server, /\.route\("\/prompt-history", get\(api_list_prompt_history\)\)/);
  assert.match(server, /discussions::list_prompt_history_dashboard\(limit\)/);
  assert.match(server, /filter_prompt_history_for_identity/);
  assert.match(platform, /async function listRemotePromptHistory<T>/);
  assert.match(platform, /apiAt<Record<string, any>>\([\s\S]*?\/api\/prompt-history\?limit=/);
  assert.match(platform, /prompts\.sort\([\s\S]*?right\.timestamp/);
});
