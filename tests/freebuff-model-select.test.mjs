import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const provider = readFileSync(new URL("../src-tauri/src/provider.rs", import.meta.url), "utf8");
const relay = readFileSync(new URL("../src/freebuff-relay.ts", import.meta.url), "utf8");
const relayStyle = readFileSync(new URL("../src/freebuff-relay.css", import.meta.url), "utf8");

test("le catalogue Freebuff propose le défaut illimité puis les modèles premium", () => {
  assert.match(main, /const DEFAULT_FREEBUFF_MODEL = "deepseek[/]deepseek-v4-flash";/);
  const start = main.indexOf("const FREEBUFF_MODEL_SUGGESTIONS = [");
  const end = main.indexOf("];", start);
  const suggestions = main.slice(start, end);
  assert.match(suggestions, /DEFAULT_FREEBUFF_MODEL/);
  assert.match(suggestions, /"deepseek[/]deepseek-v4-pro"/);
  assert.match(suggestions, /"openai[/]gpt-5\.6-luna"/);
  assert.match(suggestions, /"minimax[/]minimax-m3"/);
  assert.match(main, /if \(provider === "freebuff"\) return \[\.\.\.FREEBUFF_MODEL_SUGGESTIONS\];/);
});

test("appliquer un modèle enregistre le compte, réécrit la config et redémarre le TUI", () => {
  assert.match(main, /const applyFreebuffTerminalModel = async \(/);
  const start = main.indexOf("const applyFreebuffTerminalModel = async (");
  const end = main.indexOf("\nconst switchFreebuffTerminalAccount", start);
  const fn = main.slice(start, end);
  assert.match(fn, /account\.model = normalized;/);
  assert.match(fn, /invoke<AppSettings>\("save_settings", \{ settings \}\)/);
  assert.match(fn, /await provisionAccountHome\(account\);/);
  assert.match(fn, /await findFreebuffDiscussionForTerminal\(session\)/);
  assert.match(fn, /await closeTerminalSession\(session\.key\);/);
  assert.match(fn, /await launchFreebuffDiscussionTerminal\(discussion, account, folderPath\)/);
  // Le CLI freebuff ne lit le modele qu'au demarrage : relancer la discussion
  // est la facon dont le TUI redemarre avec le nouveau modele.
});

test("le pont du relais expose la liste des modèles et l'application au terminal cible", () => {
  assert.match(main, /models: \(\) => \[\.\.\.FREEBUFF_MODEL_SUGGESTIONS\],/);
  assert.match(main, /applyModel: async \(sessionKey: string, model: string\)/);
  assert.match(relay, /models\?: \(\) => string\[\];/);
  assert.match(relay, /applyModel\?: \(sessionKey: string, model: string\) => Promise<\{/);
  assert.match(relay, /data-freebuff-relay-model/);
  assert.match(relayStyle, /\.freebuff-relay-model-field/);
});

test("le champ modèle des réglages retombe sur le défaut du fournisseur, pas Codex", () => {
  assert.match(
    main,
    /account\.model = accountModelInput\.value\.trim\(\)\s*\|\| providerDefaultModel\(accountProvider\(account\)\);/,
  );
});

test("le backend écrit freebuffModel avant le lancement du TUI", () => {
  assert.match(
    provider,
    /"freebuffModel"\.to_string\(\),\r?\n\s*Value::String\(model\.to_string\(\)\),\r?\n\s*\);/,
  );
  assert.match(provider, /`freebuffModel` est la cle que le binaire relit au demarrage/);
  assert.match(
    provider,
    /Provider::Freebuff => ensure_freebuff_account_config\(home, model\),/,
  );
});
