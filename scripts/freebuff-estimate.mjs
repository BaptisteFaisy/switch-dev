#!/usr/bin/env node
// freebuff-estimate.mjs — estimateur Freebuff (transcript cumulatif)
//
// Reproduit la logique de `account_usage.rs::scan_freebuff_chat_file()` en
// Node.js : chaque réponse est comptée avec TOUT le transcript précédent
// comme input (pas seulement le dernier message utilisateur). Les tokens
// sont estimés à 4 caractères (FREEBUFF_CHARS_PER_TOKEN).
//
// Usage :
//   freebuff-estimate.mjs <codex-homes-dir>
//
// Sortie JSON par session : { sessionId, model, account, input, output,
//   reasoning, messageCount }
//
// Lecture streaming, un fichier à la fois → compatible mémoire faible.

import fs from "node:fs";
import path from "node:path";

const CHARS_PER_TOKEN = 4;

const [, , dataRoot] = process.argv;
if (!dataRoot) {
  console.error("usage: freebuff-estimate.mjs <codex-homes-dir>");
  process.exit(1);
}

if (!fs.existsSync(dataRoot)) {
  console.error("dossier introuvable:", dataRoot);
  process.exit(1);
}

const estimate = (chars) => Math.ceil((chars || 0) / CHARS_PER_TOKEN);

const readJsonMaybe = (p) => {
  try { return JSON.parse(fs.readFileSync(p, "utf8")); } catch { return null; }
};

/**
 * freebuff_message_text — décompose un message Freebuff en
 * (texte_utilisateur, raisonnement, output_tools).
 */
const messageChars = (msg) => {
  let text = 0, reasoning = 0, toolOutput = 0;
  if (typeof msg.content === "string") text += msg.content.length;
  if (Array.isArray(msg.blocks)) {
    for (const block of msg.blocks) {
      if (block.type === "tool") {
        const output = block.output;
        if (typeof output === "string") toolOutput += output.length;
        else if (output !== undefined) toolOutput += JSON.stringify(output).length;
        continue;
      }
      if (typeof block.content === "string") {
        const isReasoning = block.textType === "reasoning";
        if (isReasoning) reasoning += block.content.length;
        else text += block.content.length;
      }
    }
  }
  return { text, reasoning, toolOutput };
};

const isAssistant = (variant) =>
  variant === "ai" || variant === "agent" || variant === "assistant";

const modelFromSettings = (maniDir) => {
  const settings = readJsonMaybe(path.join(maniDir, "settings.json"));
  return settings?.freebuffModel || null;
};

const sessions = [];
const COUNTER_VERSION = 2;
const CACHE_CONFIDENCE = "unavailable";

for (const home of fs.readdirSync(dataRoot)) {
  const homePath = path.join(dataRoot, home);
  const mani = path.join(homePath, ".config", "manicode");
  if (!fs.existsSync(mani)) continue;

  const model = modelFromSettings(mani) || "freebuff-unknown";

  for (const root of ["projects", "projects-archive"]) {
    const projRoot = path.join(mani, root);
    if (!fs.existsSync(projRoot)) continue;

    for (const project of fs.readdirSync(projRoot)) {
      const chatsDir = path.join(projRoot, project, "chats");
      if (!fs.existsSync(chatsDir)) continue;

      for (const chatDir of fs.readdirSync(chatsDir)) {
        const msgsFile = path.join(chatsDir, chatDir, "chat-messages.json");
        const msgs = readJsonMaybe(msgsFile);
        if (!Array.isArray(msgs) || msgs.length === 0) continue;

        const sessionId = chatDir;

        let conversationContextChars = 0;
        let pendingTurnChars = 0;
        let input = 0, output = 0, reasoning = 0, total = 0, msgCount = 0;

        for (const msg of msgs) {
          const variant = msg.variant || "";
          const { text, reasoning: rChars, toolOutput } = messageChars(msg);

          if (!isAssistant(variant)) {
            const incoming = text + rChars + toolOutput;
            pendingTurnChars += incoming;
            conversationContextChars += incoming;
            continue;
          }

          if (text === 0 && rChars === 0) continue; // mode-divider etc.

          const inputChars = Math.max(conversationContextChars, pendingTurnChars);
          const turnInput = estimate(inputChars);
          const turnOutput = estimate(text);
          const turnReasoning = estimate(rChars);

          input += turnInput;
          output += turnOutput;
          reasoning += turnReasoning;
          total += turnInput + turnOutput + turnReasoning;
          msgCount++;

          const assistantOutput = text + rChars;
          conversationContextChars += assistantOutput;
          pendingTurnChars = toolOutput;
        }

        if (msgCount > 0) {
          sessions.push({
            sessionId,
            model,
            account: home,
            input,
            // Freebuff n'écrit pas de compteurs cache natifs dans les transcripts.
            // Ne jamais assimiler l'input transcript au cache réel.
            cacheRead: 0,
            cacheWrite: 0,
            cacheConfidence: CACHE_CONFIDENCE,
            inputConfidence: "estimated_transcript",
            output,
            reasoning,
            messageCount: msgCount,
            total,
            counterVersion: COUNTER_VERSION,
          });
        }
      }
    }
  }
}

console.log(JSON.stringify(sessions, null, 2));