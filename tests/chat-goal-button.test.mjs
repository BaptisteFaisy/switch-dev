import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { createGoalPrompt } from "../src/chat/runtime.ts";

const view = readFileSync(new URL("../src/chat/view.ts", import.meta.url), "utf8");
const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");

test("le bouton Goal est rendu par le composant partage desktop et web", () => {
  assert.match(view, /id="\$\{id\("chatGoal"\)\}"/);
  assert.match(view, /data-chat-action="goal"/);
  assert.match(view, /class="chat-goal"/);
  assert.match(view, /data-lucide="target"/);
  assert.match(view, /model\.supportsGoals/);
  assert.match(
    main,
    /const providerSupportsGoals = \(provider: Provider\): boolean =>[\s\S]{0,180}provider === "codex"[\s\S]{0,80}provider === "claude"[\s\S]{0,80}provider === "opencode"[\s\S]{0,80}provider === "openai-compatible"/,
  );
  assert.equal((main.match(/supportsGoals: providerSupportsGoals\(provider\)/g) ?? []).length, 2);
  assert.equal(
    (main.match(/intent === "goal" && !providerSupportsGoals\(accountProvider\(account\)\)/g) ?? []).length,
    2,
  );
});

test("le clic transforme explicitement le brouillon en creation de goal", () => {
  assert.equal(createGoalPrompt("   "), "");
  assert.equal(
    createGoalPrompt("  Livrer la version web  "),
    "Crée un goal avec l'outil create_goal pour l'objectif suivant, puis commence à le poursuivre. Si le fournisseur préfixe les outils MCP, utilise son nom exposé correspondant (par exemple cst_chat_create_goal avec OpenCode) :\n\nLivrer la version web",
  );
  assert.match(main, /sendChatMessage\("goal"\)/);
  assert.match(main, /sendExpertChatMessage\(pane, root, undefined, "goal"\)/);
  assert.match(main, /intent === "goal" \? createGoalPrompt\(value\) : value/);
});

test("le bouton reste utilisable et compact sur les petits ecrans", () => {
  assert.match(style, /\.chat-goal:focus-visible/);
  assert.match(style, /\.chat-goal:disabled/);
  assert.match(style, /\.chat-panel--compact \.chat-goal span \{ display: none; \}/);
  assert.match(style, /\.chat-goal span \{ display: none; \}/);
});
