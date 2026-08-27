import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const main = read("../src/main.ts");
const view = read("../src/chat/view.ts");
const settings = read("../src-tauri/src/settings.rs");
const server = read("../src-tauri/src/server.rs");
const chat = read("../src-tauri/src/chat.rs");

const OPENROUTER_EFFORTS = ["max", "xhigh", "high", "medium", "low", "minimal", "none"];

const block = (source, start, end) => {
  const from = source.indexOf(start);
  const to = source.indexOf(end, from + start.length);
  assert.ok(from >= 0, `debut introuvable: ${start}`);
  assert.ok(to > from, `fin introuvable: ${end}`);
  return source.slice(from, to);
};

test("le catalogue OpenRouter utilise la cle du compte, le proxy et toutes les pages sures", () => {
  const dispatch = block(
    settings,
    "pub async fn load_account_model_catalog(",
    "fn freebuff_model_catalog(",
  );

  assert.match(settings, /https:\/\/openrouter\.ai\/api\/v1\/models\/user/);
  assert.match(settings, /OPENROUTER_MODEL_PAGE_LIMIT: usize = 1000/);
  assert.match(dispatch, /inference_provider[\s\S]*?eq_ignore_ascii_case\("openrouter"\)/);
  assert.match(dispatch, /read_opencode_provider_api_key\(&home, "openrouter"\)/);
  assert.match(dispatch, /proxy_url_for_account/);
  assert.match(settings, /\.bearer_auth\(api_key\)/);
  assert.match(settings, /redirect\(reqwest::redirect::Policy::none\(\)\)/);
  assert.match(settings, /reqwest::Proxy::all\(proxy_url\)/);
  assert.match(settings, /visited/);
  assert.match(settings, /total_count/);
  assert.match(settings, /same_openrouter_catalog_endpoint/);
  assert.match(settings, /candidate\.path\(\) == endpoint\.path\(\)/);
  assert.match(
    server,
    /settings::load_account_model_catalog\(&query\.account_id\)[\s\S]*?\.await/,
  );

  const fetch = block(
    settings,
    "fn fetch_openrouter_model_catalog(",
    "fn openrouter_page_url(",
  );
  assert.doesNotMatch(fetch, /response\.text|\.text\(\)/);
});

test("les metadonnees reasoning pilotent exactement les efforts, mandatory et none", () => {
  const parser = block(
    settings,
    "fn parse_openrouter_reasoning(",
    "fn freebuff_model_catalog(",
  );

  for (const effort of OPENROUTER_EFFORTS) {
    assert.ok(settings.includes(`"${effort}"`), `niveau OpenRouter manquant: ${effort}`);
  }
  assert.match(parser, /supported_efforts/);
  assert.match(parser, /Value::Null/);
  assert.match(parser, /Value::Array/);
  assert.match(parser, /mandatory/);
  assert.match(parser, /default_enabled/);
  assert.match(parser, /default_effort/);
  assert.match(parser, /if mandatory[\s\S]*?retain\(\|effort\| effort != "none"\)/);
  assert.match(
    parser,
    /default_reasoning_effort[\s\S]*?supported_reasoning_efforts[\s\S]*?\.any\(/,
  );
  assert.match(settings, /fn openrouter_catalog_preserves_exact_reasoning_metadata_and_aliases/);
  assert.match(settings, /fn openrouter_alias_filter_rejects_shell_metacharacters/);
});

test("l'alias officiel tilde est accepte sans ouvrir la grammaire aux slugs shell", () => {
  const validator = block(
    settings,
    "pub(crate) fn safe_openrouter_model_slug(",
    "fn parse_openrouter_reasoning(",
  );
  const parserTests = block(
    settings,
    "fn openrouter_catalog_preserves_exact_reasoning_metadata_and_aliases()",
    "fn openrouter_pagination_stays_on_the_catalog_endpoint()",
  );

  assert.match(validator, /'~'/);
  assert.match(parserTests, /openrouter\/~[a-z0-9-]+\/[a-z0-9./:_-]+/);
  assert.match(chat, /openrouter\/~z-ai\/glm-latest/);
  assert.match(chat, /\["~z-ai\/glm-latest"\]/);
  for (const hostile of ["$", "`", "\\\\", ";", "|", "&"]) {
    assert.ok(
      parserTests.includes(hostile),
      `cas hostile absent du contrat de slug: ${JSON.stringify(hostile)}`,
    );
  }
});

test("l'overlay OpenRouter fusionne le modele exact sans effacer le MCP existant", () => {
  const overlay = block(
    chat,
    "fn merge_opencode_config_content(",
    "fn configure_codex_app_connectors(",
  );

  assert.match(chat, /OPENCODE_CONFIG_CONTENT_ENV/);
  assert.match(overlay, /Provider::OpenCode/);
  assert.match(overlay, /eq_ignore_ascii_case\("openrouter"\)/);
  assert.match(overlay, /strip_prefix\("openrouter\/"\)/);
  assert.match(overlay, /safe_openrouter_model_slug/);
  assert.match(overlay, /provider/);
  assert.match(overlay, /openrouter/);
  assert.match(overlay, /models/);
  assert.match(overlay, /variants/);
  assert.match(overlay, /reasoning/);
  assert.match(overlay, /effort/);
  assert.match(overlay, /command\.env\(OPENCODE_CONFIG_CONTENT_ENV/);
  assert.doesNotMatch(overlay, /reasoningEffort/);

  assert.ok(
    /get_envs\(\)/.test(overlay) || /merge_opencode_config_content/.test(overlay),
    "l'overlay doit relire ou fusionner OPENCODE_CONFIG_CONTENT",
  );
  assert.match(chat, /fn openrouter_command_overlay_registers_selected_model_and_exact_variant/);
  assert.match(chat, /fn openrouter_command_overlay_preserves_existing_mcp_config/);
  assert.match(chat, /pointer\("\/mcp\/cst_chat\/url"\)/);
  assert.match(
    chat,
    /\["variants"\]\["high"\][\s\S]*?\["reasoning"\]\["effort"\]/,
  );
  assert.match(chat, /assert!\(!encoded\.contains\("reasoningEffort"\)\)/);
  assert.match(
    chat,
    /configure_environment\([\s\S]*?configure_provider_command_with_images_and_scope\([\s\S]*?configure_opencode_model_overlay\(/,
  );
});

test("Automatique et none restent deux choix runtime distincts", () => {
  assert.match(chat, /fn openrouter_command_overlay_keeps_automatic_without_variant/);
  assert.match(chat, /fn openrouter_command_overlay_registers_explicit_none_variant/);
  assert.match(
    chat,
    /\["variants"\][\s\S]*?\["none"\]\["reasoning"\]\["effort"\]/,
  );
});

test("Compte scanne, choisit et persiste le modele et l'intensite OpenRouter", () => {
  const panel = block(
    main,
    "const renderOpenRouterAccountCatalog =",
    "const renderAccountsPanel =",
  );
  const bindings = block(
    main,
    'document.querySelectorAll<HTMLButtonElement>("[data-openrouter-scan]")',
    'document.querySelectorAll<HTMLButtonElement>("[data-account-daily-completion]")',
  );

  assert.match(main, /const accountIsOpenRouter =/);
  assert.match(main, /const chatModelCatalogErrors = new Map<string, string>\(\)/);
  assert.match(main, /const loadChatModelCatalog = async \([\s\S]*?force = false/);
  assert.match(main, /invoke<AccountModelView\[\]>\("account_model_catalog", \{ accountId \}\)/);
  assert.match(panel, /Catalogue OpenRouter/);
  assert.match(panel, /data-openrouter-scan=/);
  assert.match(panel, /data-openrouter-model=/);
  assert.match(panel, /data-openrouter-effort=/);
  assert.match(panel, /Automatique · OpenRouter/);
  assert.match(panel, /supportedReasoningEfforts/);
  assert.match(bindings, /account\.model = attemptedModel/);
  assert.match(bindings, /account\.reasoningEffort = attemptedReasoningEffort/);
  assert.match(bindings, /await persistChatPreferences\(accountId\)/);
});

test("la validation OpenRouter et la reconciliation du snapshot sont centralisees", () => {
  const validation = block(
    main,
    "const validatedAccountModelSelection =",
    "const reconcileOpenRouterAccountSelection =",
  );
  const reconciliation = block(
    main,
    "const reconcileOpenRouterAccountSelection =",
    "const reasoningEffortForChatModel =",
  );
  const loading = block(
    main,
    "const loadChatModelCatalog = async (",
    "const reasoningEffortOptions =",
  );

  assert.match(main, /type ValidatedAccountModelSelection =/);
  assert.match(validation, /accountIsOpenRouter/);
  assert.match(validation, /chatModelCatalogs\.get\(account\.id\)/);
  assert.match(validation, /toLocaleLowerCase\(\)/);
  assert.match(validation, /supportedReasoningEffortForModel/);
  assert.match(reconciliation, /validatedAccountModelSelection/);
  assert.match(reconciliation, /account\.model/);
  assert.match(reconciliation, /account\.reasoningEffort/);
  assert.match(loading, /reconcileOpenRouterAccountSelection\(currentAccount, catalog\)/);
  assert.match(loading, /await persistChatPreferences\(accountId\)/);

  const validationUses = main.match(/validatedAccountModelSelection/g) ?? [];
  assert.ok(validationUses.length >= 8, `propagation validation incomplete: ${validationUses.length}`);
});

test("la persistance est attendue, serialisee et rend l'echec visible", () => {
  const persistence = block(
    main,
    "const persistChatPreferences =",
    "const toggleChatFastMode =",
  );
  const bindings = block(
    main,
    'document.querySelectorAll<HTMLInputElement>("[data-openrouter-model]")',
    'document.querySelectorAll<HTMLButtonElement>("[data-account-daily-completion]")',
  );

  assert.match(persistence, /Promise<boolean>/);
  assert.match(persistence, /const save = chatPreferencesSave\.then/);
  assert.match(persistence, /chatPreferencesSave = save\.catch/);
  assert.match(persistence, /await invoke<AppSettings>\("save_settings"/);
  assert.match(persistence, /await provisionAccountHome\(account\)/);
  assert.match(persistence, /\.then\(\(\) => true\)/);
  assert.match(persistence, /catch[\s\S]*?statusText[\s\S]*?render\(\)[\s\S]*?return false/);
  const awaitedSaves = bindings.match(/await persistChatPreferences\(accountId\)/g) ?? [];
  assert.ok(awaitedSaves.length >= 2, `persistance non attendue: ${awaitedSaves.length}`);
});

test("le dernier snapshot sain reste utilisable pendant un refresh", () => {
  const loading = block(
    main,
    "const loadChatModelCatalog = async (",
    "const reasoningEffortOptions =",
  );
  const panel = block(
    main,
    "const renderOpenRouterAccountCatalog =",
    "const renderAccountsPanel =",
  );

  assert.match(loading, /dernier snapshot sain reste disponible/i);
  assert.doesNotMatch(loading, /chatModelCatalogs\.delete/);
  assert.match(panel, /dernier snapshot[^`]*reste disponible/i);
  assert.match(panel, /\$\{snapshot\.length \? "" : "disabled"\}/);
  assert.match(panel, /\$\{selectedModel \? "" : "disabled"\}/);
  assert.doesNotMatch(panel, /snapshot\.length && !loading/);
  assert.doesNotMatch(panel, /selectedModel && !loading/);
});

test("le chat applique effort explicite, fallback Compte, puis Automatique", () => {
  const selection = block(
    chat,
    "fn selected_reasoning_effort(",
    "fn validate_session_id(",
  );
  const frontendEfforts = block(
    main,
    "const reasoningEffortsForChatModel =",
    "const reasoningEffortForChatModel =",
  );
  const policy = block(
    chat,
    "fn openrouter_reasoning_effort_uses_request_then_account_fallback_then_automatic()",
    "fn openrouter_command_overlay_registers_selected_model_and_exact_variant()",
  );

  assert.match(selection, /Provider::OpenCode/);
  assert.match(selection, /inference_provider/);
  assert.match(selection, /request/);
  assert.match(selection, /fallback/);
  assert.match(frontendEfforts, /accountIsOpenRouter/);
  assert.match(frontendEfforts, /supportedReasoningEfforts/);
  assert.match(view, /<option value=""[^>]*>Automatique · OpenRouter<\/option>/);
  assert.match(chat, /command\.arg\("--variant"\)\.arg\(effort\)/);
  assert.match(policy, /Some\("high"\)[\s\S]*?Some\("medium"\)[\s\S]*?Some\("high"/);
  assert.match(policy, /Some\("OpenRouter"\)[\s\S]*?None[\s\S]*?Some\("medium"\)[\s\S]*?Some\("medium"/);
  assert.match(policy, /Some\("openrouter"\)[\s\S]*?None[\s\S]*?None[\s\S]*?None/);
});
