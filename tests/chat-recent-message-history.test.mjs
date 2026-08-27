import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(
  new URL("../src/chat/recent-history.ts", import.meta.url),
  "utf8",
);
const compiled = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const history = await import(
  `data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`
);

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");

test("l'historique recent conserve uniquement les bulles envoyees", () => {
  const messages = [
    { role: "user", text: "Construis la page", timestamp: 10, deliveryState: "pending" },
    {
      role: "assistant",
      text: "Voici la page terminee.",
      timestamp: 20,
      parts: [
        { kind: "reasoning", text: "raisonnement prive" },
        { kind: "tool", output: "journal interne" },
        { kind: "text", text: "Voici la page terminee." },
      ],
    },
    {
      role: "assistant",
      text: "",
      timestamp: 30,
      parts: [
        { kind: "reasoning", text: "thinking a exclure" },
        { kind: "text", text: "Deuxieme reponse finale." },
      ],
    },
    {
      role: "assistant",
      text: "",
      timestamp: 40,
      parts: [{ kind: "reasoning", text: "thinking seul" }],
    },
  ];

  assert.deepEqual(history.recentSentChatMessages(messages), [
    { role: "user", text: "Construis la page", timestamp: 10 },
    { role: "assistant", text: "Voici la page terminee.", timestamp: 20 },
    { role: "assistant", text: "Deuxieme reponse finale.", timestamp: 30 },
  ]);
  const serialized = JSON.stringify(history.recentSentChatMessages(messages));
  assert.doesNotMatch(serialized, /raisonnement|thinking|journal interne|deliveryState|parts/);
});

test("l'historique recent est borne aux 24 derniers messages", () => {
  const messages = Array.from({ length: 30 }, (_, index) => ({
    role: index % 2 === 0 ? "user" : "assistant",
    text: `message-${index}`,
    timestamp: index,
  }));
  const recent = history.recentSentChatMessages(messages);
  assert.equal(recent.length, history.RECENT_SENT_CHAT_MESSAGE_LIMIT);
  assert.equal(recent[0].text, "message-6");
  assert.equal(recent.at(-1).text, "message-29");
  assert.ok(
    recent.reduce((total, message) => total + message.text.length, 0)
      <= history.RECENT_SENT_CHAT_HISTORY_MAX_CHARS,
  );
});

test("les panneaux restaurent et rafraichissent leur cache de messages", () => {
  assert.match(main, /recentMessages\?: PersistedSentChatMessage\[\]/);
  assert.match(main, /const recentMessages = restoreRecentSentChatMessages\(persisted\.recentMessages\)/);
  assert.match(main, /messages: recentMessages/);
  assert.match(main, /recentMessages: persistedRecentChatMessagesForPane\(pane\)/);
  assert.match(main, /if \(changed\) persistExpertChats\(\)/);
  assert.match(main, /snapshot\.status === "completed"\) persistExpertChats\(\)/);
  assert.match(main, /parts: pane\.turn\.parts/);
});
