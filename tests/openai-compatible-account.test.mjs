import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");

const main = read("../src/main.ts");
const settings = read("../src-tauri/src/settings.rs");
const provider = read("../src-tauri/src/provider.rs");
const chat = read("../src-tauri/src/chat.rs");
const discussions = read("../src-tauri/src/discussions.rs");

test("la page Comptes expose la categorie Autre et ses champs OpenAI-compatible", () => {
  assert.match(main, /value="openai-compatible"/);
  assert.match(main, /id="newOpenAiCompatibleBaseUrl"/);
  assert.match(main, /id="newOpenAiCompatibleApiKey" type="password"/);
  assert.match(main, /id="newOpenAiCompatibleModel"/);
  assert.match(main, /data-openai-compatible-save/);
  assert.match(main, /data-openai-compatible-scan/);
});

test("la creation enregistre endpoint et cle puis scanne sans terminal de login", () => {
  assert.match(main, /baseUrl: openAiCompatibleBaseUrl/);
  assert.match(main, /apiKey: openAiCompatibleApiKey/);
  assert.match(main, /if \(provider === "openai-compatible"\) \{[\s\S]*?loadChatModelCatalog\(account\.id, true\)[\s\S]*?return;/);
  assert.match(main, /currentAccount\.model = catalog\[0\]\.id/);
});

test("la cle reste privee et le transport OpenCode recoit seulement une reference env", () => {
  assert.match(settings, /pub api_key: Option<String>/);
  assert.match(settings, /skip_serializing/);
  assert.match(settings, /persist_openai_compatible_api_keys/);
  assert.match(settings, /hydrate_openai_compatible_api_keys/);
  assert.match(settings, /restrict_private_file/);
  assert.match(chat, /CST_OPENAI_COMPATIBLE_API_KEY/);
  assert.match(chat, /@ai-sdk\/openai-compatible/);
  assert.match(chat, /format!\("\{\{env:\{OPENAI_COMPATIBLE_API_KEY_ENV\}\}\}"\)/);
});

test("les comptes Autre reutilisent les sessions et historiques OpenCode", () => {
  assert.match(provider, /Provider::OpenCode \| Provider::OpenAiCompatible/);
  assert.match(chat, /Provider::OpenCode \| Provider::OpenAiCompatible/);
  assert.match(discussions, /Provider::OpenCode \| settings::Provider::OpenAiCompatible/);
});
