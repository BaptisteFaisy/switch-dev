import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

// Le brouillon du chat principal ne doit jamais disparaitre quand la page se
// recharge toute seule (web-update sur /healthz, chunk Vite perime, F5).
// Ces tests verifient le cycle complet : saisie -> ecriture differee,
// rechargement -> flush sur pagehide, redemarrage -> restauration.

// main.ts est en CRLF : normalise en LF pour que les slices et regex fonctionnent.
const source = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8").replace(/\r\n/g, "\n");

const indexOf = (needle, from = 0) => {
  const index = source.indexOf(needle, from);
  assert.ok(index >= 0, `ancrage introuvable dans src/main.ts : ${JSON.stringify(needle)}`);
  return index;
};

const countOf = (needle) => source.split(needle).length - 1;

test("le brouillon est ecrit en differe apres chaque frappe, pas a chaque frappe", () => {
  const block = source.slice(
    indexOf("const CHAT_DRAFT_STORAGE_KEY"),
    indexOf("const restoreChatDraft"),
  );
  assert.match(block, /window\.setTimeout\(flushChatDraft, 250\)/);
  assert.match(block, /window\.clearTimeout\(chatDraftPersistTimer\)/);
});

test("le brouillon vide est retire du stockage et le brouillon est restaure depuis le stockage scope compte", () => {
  const block = source.slice(
    indexOf("const flushChatDraft"),
    indexOf("const scheduleChatDraftPersist"),
  );
  assert.match(block, /accountScopedStorage\.removeItem\(CHAT_DRAFT_STORAGE_KEY\)/);
  const restore = source.slice(
    indexOf("const restoreChatDraft"),
    indexOf("restoreChatDraft();", indexOf("const restoreChatDraft")),
  );
  assert.match(restore, /accountScopedStorage\.getItem\(CHAT_DRAFT_STORAGE_KEY\)/);
});

test("la restauration se fait dans boot apres la portee de stockage par compte", () => {
  const boot = source.slice(indexOf("const boot = async () => {"));
  const scope = boot.indexOf("setAccountStorageScope(");
  const restore = boot.indexOf("restoreChatDraft();");
  assert.ok(scope >= 0 && restore >= 0, "boot doit definir la portee puis restaurer le brouillon");
  assert.ok(scope < restore, "restoreChatDraft doit suivre setAccountStorageScope");
});

test("la saisie du composeur planifie la persistance", () => {
  const listener = source.slice(
    indexOf('chatPrompt?.addEventListener("input"'),
    indexOf('chatPrompt?.addEventListener("keydown"'),
  );
  assert.match(listener, /chatDraft = chatPrompt\.value;/);
  assert.match(listener, /scheduleChatDraftPersist\(\);/);
});

test("l'insertion d'un prompt et les boutons starter persistent aussi le brouillon", () => {
  const insertion = source.slice(
    indexOf("const insertion = insertPromptAtComposerSelection"),
    indexOf("chatDraft = insertion.value;") + 200,
  );
  assert.match(insertion, /chatDraft = insertion\.value;\s*\n\s*scheduleChatDraftPersist\(\);/);
  const starter = source.slice(
    indexOf('chatDraft = button.dataset.chatStarter ?? "";'),
    indexOf('chatDraft = button.dataset.chatStarter ?? "";') + 300,
  );
  assert.match(starter, /scheduleChatDraftPersist\(\);/);
});

test("l'envoi vide le brouillon ET le stockage immediatement (pas de resurrection apres reload)", () => {
  // Declaration + clear /compact + deux sites de file d'attente.
  assert.equal(countOf('chatDraft = "";'), 4);
  assert.equal(countOf("flushChatDraft();"), 4);
  const queue = source.slice(
    indexOf('if (!queuedSubmission) {\n      chatDraft = "";'),
    indexOf('if (!queuedSubmission) {\n      chatDraft = "";') + 120,
  );
  assert.match(queue, /flushChatDraft\(\);/);
});

test("tout rechargement force l'ecriture du brouillon sur pagehide", () => {
  assert.match(source, /window\.addEventListener\("pagehide", flushChatDraft\);/);
  // Second filet : passage en arriere-plan (navigateurs mobiles qui sautent
  // parfois pagehide).
  assert.match(
    source,
    /document\.addEventListener\("visibilitychange"[^]*?visibilityState === "hidden"[^]*?flushChatDraft\(\);/,
  );
});
