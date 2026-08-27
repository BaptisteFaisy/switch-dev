import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  CHAT_RESOURCE_RETRY_DELAY_MS,
  MODEL_CAPACITY_CONTINUE_PROMPT,
  MODEL_CAPACITY_RETRY_LIMIT,
  TRANSIENT_STREAM_RETRY_LIMIT,
  isModelCapacityError,
  isNodeCapacityError,
  isTransientStreamError,
  modelCapacityRetryDelayMs,
  modelCapacityRetryPrompt,
} from "../src/chat/capacity.ts";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const backend = readFileSync(new URL("../src-tauri/src/chat.rs", import.meta.url), "utf8");
const terminals = readFileSync(new URL("../src-tauri/src/resource_profile.rs", import.meta.url), "utf8");
const compose = readFileSync(new URL("../compose.yaml", import.meta.url), "utf8");

test("reconnait une saturation de modele sans la confondre avec le quota", () => {
  assert.equal(
    isModelCapacityError("Selected model is at capacity. Please try a different model."),
    true,
  );
  assert.equal(isModelCapacityError("The requested model is currently overloaded"), true);
  assert.equal(isModelCapacityError("You've hit your usage limit. Try again later."), false);
  assert.equal(isModelCapacityError("CST node capacity reached"), false);
  assert.equal(isModelCapacityError("Maximum context length exceeded"), false);
});

test("ne borne plus les chats par un compteur et temporise seulement la pression memoire", () => {
  assert.equal(CHAT_RESOURCE_RETRY_DELAY_MS, 5_000);
  assert.equal(isNodeCapacityError("capacite chats atteinte: 6/6 tours actifs"), true);
  assert.equal(isNodeCapacityError("memoire insuffisante sur l'hote"), true);
  assert.equal(isNodeCapacityError("Aucun noeud de chat disponible."), true);
  assert.equal(isNodeCapacityError("Selected model is at capacity"), false);
  assert.match(backend, /DEFAULT_MAX_ACTIVE_CHAT_TURNS: usize = 0/);
  assert.match(backend, /TOKIO_WORKER_THREADS/);
  assert.match(backend, /MALLOC_ARENA_MAX", "1/);
  assert.match(backend, /self\.max_active > 0 && active >= self\.max_active/);
  assert.match(main, /pauseChatResourceAdmission\(\)/);
  assert.doesNotMatch(main, /DEFAULT_CHAT_CONCURRENCY_LIMIT|EXPERT_MAX_TERMINALS/);
  assert.match(main, /drainNextGlobalChatCapacityQueue/);
  assert.match(main, /pane\.queuedSubmissions\.unshift\(submission\)/);
});

test("le conteneur partage une enveloppe globale de 7 Gio sans plafond de sessions", () => {
  assert.match(terminals, /DEFAULT_MAX_ACTIVE_TERMINALS: usize = 0/);
  assert.match(compose, /CST_NODE_CAPACITY: \$\{CST_NODE_CAPACITY:-0\}/);
  assert.match(compose, /CST_CHAT_MAX_ACTIVE: \$\{CST_CHAT_MAX_ACTIVE:-0\}/);
  assert.match(compose, /CST_TERMINAL_CAPACITY: \$\{CST_TERMINAL_CAPACITY:-0\}/);
  assert.match(compose, /CST_CLAUDE_MAX_SESSIONS: \$\{CST_CLAUDE_MAX_SESSIONS:-0\}/);
  assert.match(compose, /mem_limit: \$\{CST_MEM_LIMIT:-0\}/);
  assert.match(compose, /memswap_limit: \$\{CST_MEMSWAP_LIMIT:-0\}/);
  // L'enveloppe de 7 Gio reste documentee dans le compose : elle est appliquee
  // par l'environnement de deploiement (CST_MEM_LIMIT) plutot qu'en defaut.
  assert.match(compose, /Enveloppe globale de 7 Gio pour tous les chats et terminaux/);
});

test("espace trois reprises automatiques avec un backoff borne", () => {
  assert.equal(MODEL_CAPACITY_RETRY_LIMIT, 3);
  assert.deepEqual([1, 2, 3, 4].map(modelCapacityRetryDelayMs), [3_000, 6_000, 12_000, 12_000]);
});

test("continue la session existante et rejoue seulement une session jamais creee", () => {
  assert.equal(MODEL_CAPACITY_CONTINUE_PROMPT, "continue");
  assert.equal(modelCapacityRetryPrompt("session-id", "demande initiale"), "continue");
  assert.equal(modelCapacityRetryPrompt(null, "demande initiale"), "demande initiale");
});

test("les deux interfaces reessaient avec le modele exact du tour", () => {
  assert.match(main, /scheduleMainModelCapacityRetry/);
  assert.match(main, /scheduleExpertModelCapacityRetry/);
  assert.match(main, /model: activeSubmission\.model/);
  assert.match(main, /reasoningEffort: activeSubmission\.reasoningEffort/);
  assert.match(main, /modelCapacityRetryPrompt\(resumeSessionId, activeSubmission\.prompt\)/);
  assert.match(backend, /is_model_capacity_message/);
});

test("reconnait une coupure de flux transitoire sans la confondre avec autre chose", () => {
  assert.equal(
    isTransientStreamError("API Error: Connection closed mid-response. The response above may be incomplete."),
    true,
  );
  assert.equal(isTransientStreamError("read ECONNRESET"), true);
  assert.equal(isTransientStreamError("socket hang up"), true);
  // Ne doit PAS matcher une saturation, un quota, ni une erreur applicative.
  assert.equal(isTransientStreamError("Selected model is at capacity"), false);
  assert.equal(isTransientStreamError("You've hit your usage limit."), false);
  assert.equal(isTransientStreamError("Maximum context length exceeded"), false);
  assert.equal(isTransientStreamError(""), false);
  assert.equal(isTransientStreamError(null), false);
});

test("la reprise sur drop transitoire est plus prudente (au plus 2 essais)", () => {
  assert.equal(TRANSIENT_STREAM_RETRY_LIMIT, 2);
});

test("un drop transitoire n'est rejoue que dans les cas surs (plan/question ou avant session)", () => {
  // Porte de securite : lecture seule, OU aucune session encore etablie. Cote
  // Claude un tool_use/edit ne cree pas de part, donc `parts` n'est PAS un signal
  // fiable ; le session_id (emis a l'init, avant tout outil) l'est.
  assert.match(
    main,
    /mode === "plan" \|\| mode === "ask" \|\| !turn\.sessionId\?\.trim\(\)/,
  );
  // Le classifieur commun distingue saturation vs drop transitoire.
  assert.match(main, /retryableTurnErrorKind\(turn, activeSubmission\.mode\)/);
  assert.match(main, /kind === "capacity" \? MODEL_CAPACITY_RETRY_LIMIT : TRANSIENT_STREAM_RETRY_LIMIT/);
});

test("les points de decision planifient une reprise sur toute erreur rejouable, pas la seule saturation", () => {
  // Correctif : les callers ne doivent plus gater sur isModelCapacityError seul,
  // sinon un drop transitoire ne declencherait jamais la reprise.
  assert.match(main, /const retryableFailure = failedTurnIsRetryable\(/);
  assert.match(main, /if \(retryableFailure\) \{\s*\n\s*scheduleMainModelCapacityRetry/);
  assert.match(main, /if \(retryableFailure\) \{\s*\n\s*scheduleExpertModelCapacityRetry/);
  // Le helper de rejouabilite exige la soumission active (pour le mode).
  assert.match(
    main,
    /retryableTurnErrorKind\(turn, activeSubmission\.mode\) !== null/,
  );
});
