import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import { rankRemoteAllocations } from "../src/remote-allocation.ts";

const node = (id, priority = 10) => ({ id, priority });
const health = (overrides = {}) => ({
  ok: true,
  ready: true,
  draining: false,
  activeTerminals: 0,
  activeChatTurns: 0,
  capacity: 4,
  ...overrides,
});

test("les nouveaux chats vont au noeud sain le moins charge", () => {
  const ranked = rankRemoteAllocations([
    { node: node("charge"), health: health({ activeChatTurns: 3 }) },
    { node: node("libre"), health: health({ activeChatTurns: 1 }) },
  ], "chat", "account-a");

  assert.deepEqual(ranked.map((entry) => entry.node.id), ["libre", "charge"]);
});

test("les terminaux actifs comptent aussi dans la charge d'un chat", () => {
  const ranked = rankRemoteAllocations([
    { node: node("terminaux"), health: health({ activeTerminals: 3 }) },
    { node: node("libre"), health: health({ activeChatTurns: 1 }) },
  ], "chat");

  assert.deepEqual(ranked.map((entry) => entry.node.id), ["libre", "terminaux"]);
});

test("la capacite est souple mais les noeuds non satures passent d'abord", () => {
  const ranked = rankRemoteAllocations([
    { node: node("sature", 1), health: health({ activeChatTurns: 4 }) },
    { node: node("disponible", 90), health: health({ activeChatTurns: 3 }) },
  ], "chat");

  assert.deepEqual(ranked.map((entry) => entry.node.id), ["disponible", "sature"]);
  assert.equal(ranked[1].saturated, true);
});

test("une capacite nulle annonce un noeud sans plafond numerique", () => {
  const [ranked] = rankRemoteAllocations([
    { node: node("memoire-seule"), health: health({ capacity: 0, activeChatTurns: 40 }) },
  ], "chat");

  assert.equal(ranked.saturated, false);
});

test("le drain, la panne et l'absence explicite du compte excluent un noeud", () => {
  const ranked = rankRemoteAllocations([
    { node: node("drain"), health: health({ draining: true }) },
    { node: node("panne"), health: health({ ok: false }) },
    { node: node("sans-compte"), health: health({ availableAccountIds: ["account-b"] }) },
    { node: node("avec-compte"), health: health({ availableAccountIds: ["account-a"] }) },
  ], "chat", "account-a");

  assert.deepEqual(ranked.map((entry) => entry.node.id), ["avec-compte"]);
});

test("une sante inconnue est un dernier recours", () => {
  const ranked = rankRemoteAllocations([
    { node: node("inconnu", 1), health: null },
    { node: node("sain", 90), health: health({ activeChatTurns: 3 }) },
  ], "chat");

  assert.deepEqual(ranked.map((entry) => entry.node.id), ["sain", "inconnu"]);
  assert.equal(ranked[1].healthKnown, false);
});

test("la plateforme conserve les routes des tours et sessions distants", () => {
  const platform = fs.readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
  const view = fs.readFileSync(new URL("../src/chat/view.ts", import.meta.url), "utf8");

  assert.match(platform, /remoteChatTurnRoutes/);
  assert.match(platform, /startRemoteChatTurn/);
  assert.match(platform, /listRemoteActiveChatTurns/);
  assert.match(platform, /rememberRemoteSessionRoute/);
  assert.match(platform, /listRemoteDiscussions/);
  assert.match(view, /nodeLabel/);
});
