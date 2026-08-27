import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const panelStart = main.indexOf("const renderAccountsPanel =");
const panelEnd = main.indexOf("const renderAccountsAndPool =", panelStart);
const accountsPanel = main.slice(panelStart, panelEnd);
const wrapperStart = panelEnd;
const wrapperEnd = main.indexOf("const renderNewChatModal =", wrapperStart);
const accountsWrapper = main.slice(wrapperStart, wrapperEnd);

test("la vue Comptes affiche uniquement la gestion simple des comptes", () => {
  assert.match(main, /case "pool":\s*\n\s*return renderAccountsAndPool\(\);/);
  assert.match(main, /const renderAccountsAndPool = \(\): string =>/);
  assert.match(accountsWrapper, /renderAccountsPanel\(\)/);
  assert.doesNotMatch(accountsWrapper, /renderPoolPanel\(\)/);
  assert.match(accountsPanel, /<strong>Comptes<\/strong>/);

  // Aucun reglage avance ni outil de pool ne doit revenir dans ce panneau.
  assert.doesNotMatch(
    accountsPanel,
    /CODEX_HOME|Repo Git|Proxy compte|Modele Codex|Intensite|Mode bypass|JSON pool|Pool de service|Shell|Commande|Auto-detect/,
  );
});

test("renderAccountsPanel n'est plus du code mort", () => {
  // Au moins deux occurrences : la definition + un appel effectif.
  const occurrences = main.match(/renderAccountsPanel/g) ?? [];
  assert.ok(
    occurrences.length >= 2,
    `renderAccountsPanel devrait etre appele au moins une fois (trouve ${occurrences.length} occurrence(s))`,
  );
});

test("le formulaire minimal ajoute un compte natif ou un fournisseur OpenCode", () => {
  assert.match(accountsPanel, /id="addAccountForm"/);
  assert.match(accountsPanel, /id="newAccountLabel"/);
  assert.match(accountsPanel, /name="newAccountProvider" value="codex"/);
  assert.match(accountsPanel, /name="newAccountProvider" value="claude"/);
  assert.match(accountsPanel, /OPENCODE_PROVIDER_OPTIONS\.map/);
  assert.match(main, /id: "zai"/);
  assert.match(main, /id: "minimax"/);
  assert.match(main, /id: "deepseek"/);
  assert.match(main, /id: "openrouter"/);
  assert.match(accountsPanel, /Ajouter et se connecter/);
  assert.match(
    main,
    /querySelector<HTMLFormElement>\("#addAccountForm"\)\?\.addEventListener\("submit"/,
  );
  assert.match(main, /const \{ provider, inferenceProvider \} = parseProviderChoice\(providerValue\)/);
  assert.match(main, /uniqueCodexHomeForLabel\(label, provider\)/);
  assert.match(main, /\{ provider, inferenceProvider \}/);
  assert.match(accountsPanel, /npm install -g opencode-ai/);
});

test("la connexion API annexe passe par OpenCode et cible le bon provider", () => {
  assert.match(main, /provider === "opencode"/);
  assert.match(main, /auth login --provider \$\{inferenceProvider\}/);
  assert.match(main, /agentProvider\(agent\) === "opencode"/);
});

test("les suggestions de modeles Claude proposent les alias CLI et les modeles actuels", () => {
  const start = main.indexOf("const CLAUDE_MODEL_SUGGESTIONS = [");
  const end = main.indexOf("];", start);
  assert.ok(start >= 0 && end > start, "liste CLAUDE_MODEL_SUGGESTIONS introuvable");
  const block = main.slice(start, end);
  for (const model of [
    '"sonnet"',
    '"opus"',
    '"haiku"',
    '"claude-fable-5"',
    '"claude-opus-5"',
    '"claude-opus-4-8"',
    '"claude-sonnet-5"',
    '"claude-haiku-4-5"',
  ]) {
    assert.ok(block.includes(model), `${model} manquant dans CLAUDE_MODEL_SUGGESTIONS`);
  }
  // Les modeles retires/deprecies ne doivent plus etre proposes.
  assert.ok(!block.includes('"claude-opus-4-1"'), "claude-opus-4-1 est deprecie");
  // La liste alimente bien le champ modele des comptes Claude, et le modele
  // choisi est materialise dans le settings.json du home Claude (terminaux).
  assert.match(main, /if \(provider === "claude"\) return \[\.\.\.CLAUDE_MODEL_SUGGESTIONS\];/);
});

test("le terminal OpenCode reprend le modele du compte", () => {
  assert.match(main, /if \(agentProvider\(agent\) === "opencode"\)/);
  assert.match(main, /const requestedModel = account \? accountModel\(account\) : null;/);
  assert.match(main, /safeCliModel\(requestedModel\)/);
  assert.match(main, /command \+= ` --model \$\{model\}`/);
  assert.match(main, /command \+= " --auto"/);
});

test("chaque compte expose le suivi quotidien, la connexion et la suppression", () => {
  assert.match(accountsPanel, /data-account-daily-completion=/);
  assert.match(accountsPanel, /data-lucide="star"/);
  assert.match(accountsPanel, /aria-pressed="\$\{completedToday\}"/);
  assert.match(accountsPanel, /data-login-account=/);
  assert.match(accountsPanel, /data-delete-account=/);
  assert.match(accountsPanel, />Se connecter<\/span>/);
  assert.match(
    main,
    /querySelectorAll<HTMLElement>\("\[data-login-account\]"\)/,
  );
  assert.match(
    main,
    /querySelectorAll<HTMLButtonElement>\("\[data-account-daily-completion\]"\)/,
  );
  assert.match(
    main,
    /querySelectorAll<HTMLButtonElement>\("\[data-delete-account\]"\)/,
  );
});

test("creer un compte ouvre une fenetre de connexion (login)", () => {
  // « + Compte » delegue au flux qui persiste puis ouvre le terminal de login.
  assert.match(main, /const addAccountAndLogin = async \(\) =>/);
  assert.match(main, /addAccountAndLogin\(\)/);
  // Le compte est persiste (supprimable) puis un terminal de login est ouvert.
  assert.match(main, /save_settings/);
  assert.match(main, /await reloginAccount\(account\.id, deferredMobileLogin\)/);
});

test("reconnecter Codex supprime l'ancienne session avant le nouveau login", () => {
  // Un auth.json revoque existe encore : `codex login` seul n'est pas une
  // reconnexion fiable. Le terminal doit enchainer logout puis login.
  assert.match(main, /provider === "codex"/);
  assert.match(main, /agentSubcommand\(agent, "logout"\)/);
  assert.match(main, /shellCommandSeparator\(\)/);
  assert.match(main, /reconnectCommand,/);
});

test("la reconnexion Codex utilise le device flow uniquement sur un serveur distant", () => {
  assert.match(
    main,
    /provider === "codex" && isRemoteMode\(\) && loginSub === "login"/,
  );
  assert.match(main, /\? "login --device-auth"\s*:\s*loginSub/);
  assert.match(main, /const loginCommand = agentSubcommand\(agent, effectiveLoginSub\);/);
});

test("la reconnexion utilise un terminal strictement reserve au login", () => {
  assert.match(main, /if \(!account\.codexHome\.trim\(\)\)/);
  assert.match(main, /reconnectCommand,\s*\n\s*agentId,\s*\n\s*null,\s*\n\s*null,\s*\n\s*true/);
  assert.match(main, /loginOnly\s*\? userEnvironmentPath\(startedAccount\?\.codexHome\)/);
  assert.match(main, /workspacePath: isRemoteMode\(\) && !loginOnly/);
  assert.match(main, /loginOnly,\s*\n/);
  assert.match(main, /!loginOnly && settings\.autoRunCodex/);
  assert.match(main, /!loginOnly &&\s*\n\s*\(session\.resumeSessionId/);
});

test("une reconnexion ne peut ouvrir qu'un terminal temporaire a la fois", () => {
  // Le login ne restaure pas implicitement les terminaux de travail et ne sera
  // lui-meme jamais sauvegarde pour une restauration ulterieure.
  assert.match(
    main,
    /if \(!loginOnly \|\| terminalRestoreAttempted\) \{\s*await ensureTerminalsRestored\(\);/,
  );
  assert.match(main, /loginOnly,\s*\n\s*folderPath: loginOnly \? null : capturedEnvironment/);
  assert.match(
    main,
    /\.filter\(\(session\) => session\.status !== "Ferme" && session\.ptyId !== null && !session\.loginOnly\)/,
  );
  assert.match(main, /if \(!terminalRestoreAttempted\) return;/);

  // Deux declenchements concurrents pour le meme compte reutilisent la meme
  // promesse au lieu de reserver puis demarrer deux PTY.
  assert.match(
    main,
    /const loginTerminalCreations = new Map<string, Promise<TerminalSession \| null>>\(\);/,
  );
  assert.match(main, /const inFlightLogin = loginTerminalCreations\.get\(accountId\);/);
  assert.match(main, /if \(inFlightLogin\) \{[\s\S]*?return inFlightLogin;/);
  assert.match(main, /loginTerminalCreations\.set\(accountId, creation\);/);
  assert.match(main, /loginTerminalCreations\.delete\(accountId\);/);

  // Une fois le PTY cree, recliquer doit refocaliser ce terminal au lieu d'en
  // creer un deuxieme qui consommerait un slot VPS.
  assert.match(main, /const existingLogin = terminalSessions\.find\(/);
  assert.match(main, /session\.loginOnly &&[\s\S]*?session\.accountId === accountId/);
  assert.match(main, /activateTerminalSession\(existingLogin\);/);
  assert.match(main, /return existingLogin;/);
});

test("le terminal de connexion sans workspace reste visible", () => {
  assert.match(main, /const activeLoginTerminal = \(\) =>/);
  assert.match(main, /if \(loginSession\) return \[loginSession\];/);
  assert.match(main, /if \(!folderPath && !loginSession\)/);
  assert.match(main, /Aucun dossier projet requis/);
  assert.match(main, /Terminal temporaire/);
});

test("le login depuis Nouveau terminal persiste aussi dans le home du compte", () => {
  const handlerStart = main.indexOf(
    'querySelector<HTMLButtonElement>("#loginNewTerminal")?.addEventListener',
  );
  const handlerEnd = main.indexOf(
    'querySelector<HTMLButtonElement>("#poolTerminal")?.addEventListener',
    handlerStart,
  );
  assert.ok(handlerStart >= 0 && handlerEnd > handlerStart, "gestionnaire login introuvable");
  const handler = main.slice(handlerStart, handlerEnd);

  // Le meme logout + login est utilise, et loginOnly=true empeche le lancement
  // automatique de la commande de travail du compte.
  assert.match(handler, /reconnectCommandForAccount\(account, agent\)/);
  assert.match(handler, /null,\s*\n\s*true,\s*\n\s*\);/);
  assert.doesNotMatch(handler, /upsertWorkspaceRegistry/);
});

test("une erreur d'auth reste prioritaire sur les limites locales en cache", () => {
  assert.match(main, /isQuotaAuthenticationError\(account\.error\)/);
  assert.match(
    main,
    /if \(isQuotaAuthenticationError\(account\.error\)\) return "error";/,
  );
});

test("la connexion cible directement le compte de la carte", () => {
  assert.match(accountsPanel, /data-login-account="\$\{escapeAttr\(item\.id\)\}"/);
  assert.match(
    main,
    /const accountId = button\.dataset\.loginAccount;[\s\S]*?selectedAccountId = accountId;[\s\S]*?void reloginAccount\(accountId\);/,
  );
});

test("la suppression persiste vraiment (backend + repli)", () => {
  // La suppression passe par une fonction dediee, pas un simple filtre local.
  assert.match(main, /const deleteSelectedAccount = async \(\) =>/);
  assert.match(
    main,
    /const accountId = button\.dataset\.deleteAccount;[\s\S]*?selectedAccountId = accountId;[\s\S]*?void deleteSelectedAccount\(\);/,
  );
  // Suppression backend (compte persiste) avec repli save_settings (compte neuf).
  assert.match(main, /invoke<AppSettings>\("remove_account", \{ accountId: id, deleteFiles \}\)/);
});

test("la suppression tient compte de l'auto-detection (sinon le compte revient)", () => {
  // Avec l'auto-detection active, la confirmation unique inclut aussi les
  // fichiers, sinon le compte serait re-decouvert au prochain chargement.
  assert.match(main, /const deleteFiles = settings\.autoDiscoverAccounts;/);
  assert.match(main, /const confirmed = window\.confirm\(/);
  assert.match(main, /if \(!confirmed\) return;/);
});

test("le re-rendu preserve le scroll des vues admin (ne fait plus remonter)", () => {
  // render() capture puis restaure le scrollTop de .chat-admin-panel autour du
  // remplacement complet du DOM.
  assert.match(main, /const adminScrollTop =\s*\n?\s*document\.querySelector<HTMLElement>\("\.chat-admin-panel"\)\?\.scrollTop \?\? 0;/);
  assert.match(main, /restoredAdminPanel\.scrollTop = adminScrollTop;/);
});
