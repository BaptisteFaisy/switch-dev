import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  CHAT_AUTO_COMPACT_THRESHOLD_TOKENS,
  chatContextPressure,
  chatTokenUsagePresentation,
  isCompactSlashCommand,
  normalizeChatContextUsage,
  normalizeChatTokenCount,
  shouldAutoCompactChatContext,
} from "../src/chat/token-usage.ts";

test("le compteur de tokens normalise et formate l'usage cumule du chat", () => {
  assert.equal(normalizeChatTokenCount(12_345.4), 12_345);
  assert.equal(normalizeChatTokenCount(-1), null);
  assert.equal(normalizeChatTokenCount(Number.NaN), null);

  const usage = chatTokenUsagePresentation(12_345);
  assert.equal(usage.count, 12_345);
  assert.equal(usage.value.replace(/\D/g, ""), "12345");
  assert.equal(usage.unit, "tokens");
  assert.match(usage.title, /tokens utilis.s dans ce chat/);
});

test("le compteur distingue zero, le singulier et une mesure indisponible", () => {
  assert.equal(chatTokenUsagePresentation(0).unit, "tokens");
  assert.equal(chatTokenUsagePresentation(1).unit, "token");
  assert.deepEqual(chatTokenUsagePresentation(null), {
    count: null,
    value: "—",
    unit: "tokens",
    title: "Nombre de tokens indisponible pour ce chat",
    pressure: "unknown",
    usedPercent: null,
    signature: "unavailable",
  });
});

test("la pression de contexte passe du vert a l'orange puis au rouge", () => {
  const context = (usedPercent) => ({
    usedTokens: 32_000,
    contextWindow: 100_000,
    remainingTokens: 68_000,
    usedPercent,
  });
  assert.equal(chatContextPressure(context(59)), "safe");
  assert.equal(chatContextPressure(context(60)), "warning");
  assert.equal(chatContextPressure(context(79)), "warning");
  assert.equal(chatContextPressure(context(80)), "danger");

  const usage = chatTokenUsagePresentation(999_999, context(80));
  assert.equal(usage.pressure, "danger");
  assert.equal(usage.usedPercent, 80);
  assert.match(usage.value, /32.*100/);
  assert.equal(normalizeChatContextUsage({ ...context(80), usedPercent: 120 })?.usedPercent, 100);
});

test("/compact est reconnu comme commande exacte", () => {
  assert.equal(isCompactSlashCommand("/compact"), true);
  assert.equal(isCompactSlashCommand("  /COMPACT  "), true);
  assert.equal(isCompactSlashCommand("/compact maintenant"), false);
  assert.equal(isCompactSlashCommand("bonjour /compact"), false);
});

test("le compactage automatique demarre seulement au-dela de 400 000 tokens de contexte", () => {
  const context = (usedTokens) => ({
    usedTokens,
    contextWindow: 1_000_000,
    remainingTokens: 1_000_000 - usedTokens,
    usedPercent: Math.round(usedTokens / 10_000),
  });

  assert.equal(CHAT_AUTO_COMPACT_THRESHOLD_TOKENS, 400_000);
  assert.equal(shouldAutoCompactChatContext(context(400_000)), false);
  assert.equal(shouldAutoCompactChatContext(context(400_001)), true);
  assert.equal(shouldAutoCompactChatContext(null), false);
});

test("l'en-tete du chat expose le compteur accessible et actualisable", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const view = readFileSync(new URL("../src/chat/view.ts", import.meta.url), "utf8");
  assert.match(view, /data-chat-control="tokens"/);
  assert.match(view, /data-chat-action="compact"/);
  assert.match(view, /data-chat-token-count=/);
  assert.match(view, /data-chat-context-percent=/);
  assert.match(view, /data-chat-token-value/);
  assert.match(view, /aria-label=/);
  assert.match(view, /renderChatTokenUsage\(model, busy\)/);
  assert.equal(
    [...main.matchAll(/totalTokens: discussion \? discussion\.totalTokens : 0/g)].length,
    2,
  );
  assert.match(main, /contextUsage: chatContextUsage/);
  assert.match(main, /contextUsage: pane\.contextUsage/);
  assert.match(main, /syncPanelTokenUsage/);
  assert.match(main, /chatTokenUsagePresentation\(/);
});
