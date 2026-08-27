import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");
const keyboardShortcuts = readFileSync(
  new URL("../src/keyboard-shortcuts.ts", import.meta.url),
  "utf8",
);
const desktop = readFileSync(
  new URL("../src-tauri/src/terminal.rs", import.meta.url),
  "utf8",
);
const server = readFileSync(
  new URL("../src-tauri/src/server.rs", import.meta.url),
  "utf8",
);
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
const desktopApp = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const gitDockerBackend = readFileSync(
  new URL("../src-tauri/src/git_docker_environment.rs", import.meta.url),
  "utf8",
);

test("un menu separe est l'unique selecteur d'environnement", () => {
  for (const marker of [
    "chat-environment-selector",
    "renderTerminalEnvironmentMenu",
    "data-environment-menu-id",
    "selectEnvironment",
    "Choisir un environnement",
  ]) {
    assert.ok(main.includes(marker), `selecteur manquant: ${marker}`);
  }
  assert.doesNotMatch(main, /renderTerminalEnvironmentTabs|data-terminal-environment|Nouvel onglet/);
  assert.doesNotMatch(style, /\.terminal-environment-tab/);
});

test("la touche accent grave ouvre le menu des environnements", () => {
  assert.match(main, /keyboardShortcutMatchesAction\("toggle-environments", event\)/);
  assert.match(keyboardShortcuts, /id: "toggle-environments"[\s\S]*?defaultBinding: "Backquote"/);
  assert.match(keyboardShortcuts, /event\.code === "Digit7"/);
  assert.match(main, /renderTerminalEnvironmentMenu/);
  assert.match(main, /terminalEnvironmentMenuOpen/);
  assert.match(main, /data-environment-menu-id/);
  assert.match(style, /\.terminal-environment-menu-backdrop/);
});

test("l'environnement actif contient ses propres chats", () => {
  for (const marker of [
    "expertChatPanesForCurrentEnvironment",
    "expertChatPaneEnvironmentPath",
    "Chats de cet environnement",
  ]) {
    assert.ok(main.includes(marker), `contexte d'environnement incomplet: ${marker}`);
  }
  assert.match(main, /workspaceIdForPath\(panePath\) === environmentId/);
  assert.match(main, /discussion\.folderPath = capturedWorkspace/);
});

test("les terminaux ouverts apparaissent dans la sidebar de leur environnement", () => {
  assert.match(main, /const environmentTerminals = terminalSessions\.filter/);
  assert.match(main, /workspaceIdForPath\(path\) === environmentId/);
  assert.match(main, /Terminaux de cet environnement/);
  assert.match(main, /data-open-terminal=/);
});

test("la sidebar garde la corbeille et la fenetre porte seule la croix", () => {
  assert.doesNotMatch(main, /data-hide-environment-terminal=/);
  assert.match(main, /data-delete-terminal-history=/);
  assert.match(main, /data-delete-terminal-history[\s\S]*?<i data-lucide="trash-2"><\/i>/);
  assert.match(
    main,
    /class="expert-pane-fullscreen"[\s\S]*?<\/button>\s*<button[^>]*class="expert-pane-close"[^>]*data-close-terminal[\s\S]*?<i data-lucide="x"><\/i>/,
  );
  assert.match(main, /\[data-close-terminal\][\s\S]*?closeTerminalSession\(key\)/);
  assert.match(main, /const deleteTerminalHistoryAndChat = async/);
  assert.match(
    main,
    /deleteTerminalHistoryAndChat[\s\S]*?closeTerminalSession\(key\)[\s\S]*?archiveDiscussionById\([\s\S]*?true/,
  );
  assert.match(main, /id="terminalDeleteBackdrop"/);
  assert.match(main, /Supprimer ce terminal et son chat \?/);
  assert.match(style, /\.chat-side-terminal-delete/);
  assert.doesNotMatch(style, /\.chat-side-terminal-hide/);
  assert.match(style, /\.expert-pane-fullscreen/);
  assert.match(style, /\.expert-pane-close/);
  assert.match(
    style,
    /\.expert-terminal-pane-head\.has-new-chat\s*\{[\s\S]*?grid-template-columns:\s*minmax\(0, 1fr\) 28px auto 28px 28px 32px;/,
  );
  assert.match(
    style,
    /\.expert-terminal-pane-head\.has-freebuff-account-switch\.has-new-chat\s*\{[\s\S]*?grid-template-columns:\s*minmax\(0, 1fr\) minmax\(108px, 150px\) 28px auto 28px 28px 32px;/,
  );
});

test("les terminaux restent separes des chats de l'environnement", () => {
  assert.match(main, /const renderChatSidebarOpenTerminals =/);
  assert.match(main, /<span>Terminaux ouverts<\/span>/);
  assert.match(
    main,
    /\[data-open-terminal\][\s\S]*?activateTerminalSession\(session\);\s*activeView = "terminal";/,
  );
  assert.doesNotMatch(main, /expertChatWallTerminals|wallTerminals/);
  assert.doesNotMatch(
    main,
    /renderExpertChatGrid[\s\S]*?pagePanes\.map\(renderExpertChatPane\)[\s\S]*?renderExpertTerminalPane/,
  );
});

test("un nouveau chat attribue l'agent automatiquement avec un reglage facultatif", () => {
  // La fenetre garde le modele et le mode, tandis que le routage de compte est
  // automatique dans le parcours principal et facultatif dans un <details>.
  assert.doesNotMatch(main, /id="newChatAgent"/);
  assert.match(main, /const renderNewChatModal =/);
  assert.match(main, /data-new-chat-account/);
  assert.match(main, /new-chat-routing-details/);
  assert.match(main, /data-new-chat-routing-auto/);
  assert.match(main, /id="newChatModel"/);
  assert.match(main, /id="newChatMode"/);
  assert.doesNotMatch(main, /Compte \/ agent|Choisis le compte, le modele et le mode/);
  // Tous les points d'entree "nouveau chat" passent par la fenetre.
  assert.match(main, /const openNewChat = \(\) => \{\s*openNewChatModal\(\);/);
  assert.match(main, /#addExpertChat"\)\?\.addEventListener\("click", \(\) => \{\s*openNewChatModal\(\);/);
  // La fenetre cree le pane avec le routage, le modele et le mode retenus.
  assert.match(
    main,
    /addExpertChatPane\(account\.id, \{\s*mode,\s*pendingWorkspace,\s*executionTargetId,\s*\}\)/,
  );
  assert.match(main, /accountId: accountId \?\?/);
  assert.match(style, /\.new-chat-account-option/);
});

test("le choix du compte affiche son pourcentage d'utilisation", () => {
  assert.match(main, /const newChatAccountUsageFor =/);
  assert.match(main, /Math\.round\(100 - status\.remainingPercent\)/);
  assert.match(main, /data-new-chat-account-usage=/);
  assert.match(main, /syncNewChatAccountUsageUi\(\)/);
  assert.match(
    main,
    /newChatModalOpen = true;[\s\S]*?render\(\);\s*void refreshLimitStatus\(true\);/,
  );
  assert.match(style, /\.new-chat-account-usage/);
});

test("un environnement vide ne recoit jamais de chat de base", () => {
  assert.doesNotMatch(main, /createExpertChatPane\(\s*\)/);
  assert.doesNotMatch(
    main,
    /!expertChatPanes\.length\)[^\n]*createExpertChatPane/,
  );
  assert.match(main, /Commencez une nouvelle conversation/);
  assert.match(main, /id="emptyNewChat"/);
  assert.match(main, /Dernière discussion/);
});

test("le choix d'environnement propose un explorateur de dossiers navigable", () => {
  for (const marker of [
    "Créer un autre environnement",
    "workspacePathBreadcrumbs",
    "workspaceFolderSearch",
    "ws-quick-access",
    "data-ws-dir",
    "Choisir ce dossier",
  ]) {
    assert.ok(main.includes(marker), `explorateur incomplet: ${marker}`);
  }
  assert.match(style, /\.workspace-browser-modal/);
  assert.match(style, /\.ws-breadcrumb/);
  assert.match(style, /\.ws-folder-toolbar/);
});

test("les dossiers choisis sont utilises directement", () => {
  assert.match(main, /userWorkspacePath\(stored\)/);
  assert.match(main, /userEnvironmentPath\(discussion\?\.folderPath\)/);
  assert.match(desktop, /provider\.home_env_var\(\)/);
  assert.match(desktop, /builder\.cwd\(project_dir\.as_os_str\(\)\)/);
});

test("un environnement peut etre retire depuis son menu sans effacer ses fichiers", () => {
  assert.match(main, /data-delete-environment-id/);
  assert.match(main, /Supprimer l'environnement/);
  assert.match(main, /Le repertoire et ses fichiers resteront sur le disque/);
  assert.match(main, /closeWorkspace\(workspace, true\)/);
  assert.match(main, /!workspaceIsClosed\(path\)/);
  assert.match(style, /\.terminal-environment-menu-delete/);
});

test("la creation exige un environnement avant tout appel PTY", () => {
  assert.match(main, /const environmentPath = loginOnly \? null : userWorkspacePath\(folderPath\)/);
  assert.match(main, /Creation bloquee: choisis d'abord un environnement/);
  assert.match(main, /Environnement de ce terminal \/ session \(obligatoire\)/);
  assert.match(main, /aria-required="true"/);
});

test("un chat actif sans discussion listee reste visible dans la barre laterale", () => {
  // Regression : un nouveau chat (ou un chat dont le dossier n'est pas resolu)
  // apparait dans la grille mais disparaissait de « Chats de cet environnement ».
  // La barre laterale doit unir les discussions persistees et les panes ouverts.
  assert.match(main, /draftEnvironmentChatPanes\(\s*\n?\s*expertChatPanesForCurrentEnvironment\(\)/);
  // Le compteur additionne les brouillons aux discussions persistees.
  assert.match(main, /discussions\.length \+ draftPanes\.length/);
  // Chaque brouillon est focalisable et fermable par cle de pane.
  assert.ok(main.includes("data-open-pane"), "bouton d'ouverture de pane manquant");
  assert.ok(main.includes("data-close-pane"), "bouton de fermeture de pane manquant");
  assert.match(
    main,
    /expertChatPanes\.find\(\(item\) => item\.key === button\.dataset\.openPane\)/,
  );
  assert.match(
    main,
    /expertChatPanes\.find\(\(item\) => item\.key === button\.dataset\.closePane\)/,
  );
});

test("le chat actif n'est pas entoure d'un cadre de selection", () => {
  assert.doesNotMatch(
    style,
    /\.expert-chat-wall\s+\.chat-panel\.active\s*\{[^}]*\bborder(?:-color)?\s*:/,
  );
});

test("les backends desktop et serveur refusent un environnement implicite", () => {
  const error = "Environnement obligatoire avant d'ouvrir un terminal";
  assert.ok(desktop.includes(error));
  assert.ok(server.includes(error));
  assert.doesNotMatch(server, /prepare_local\(&agent_id, &canonical_home, None\)/);
});

test("le bouton Git Docker est retire de l'UI mais le backend reste disponible", () => {
  // Plus aucun point d'entree UI : ni le bouton du menu environnements, ni
  // l'import GitHub du modal workspace, ni le modal lui-meme.
  for (const marker of [
    'id="createGitDockerEnvironmentFromMenu"',
    'id="createWorkspaceFromGitHub"',
    'id="gitDockerRepositoryUrl"',
    "renderGitDockerEnvironmentModal",
    "create_git_docker_environment",
  ]) {
    assert.ok(!main.includes(marker), `UI Git Docker encore presente: ${marker}`);
  }
  // La route reste cablee cote transport et backend (API directe).
  assert.match(platform, /case "create_git_docker_environment":\s*return api<T>\("POST", "\/api\/workspaces\/git-docker", args\.request\)/);
  assert.match(desktopApp, /git_docker_environment::create_git_docker_environment/);
  assert.match(server, /"\/workspaces\/git-docker",\s*post\(api_create_git_docker_environment\)/);
  const handlerStart = server.indexOf("async fn api_create_git_docker_environment");
  const handlerEnd = server.indexOf("async fn api_delete_workspace", handlerStart);
  const handler = server.slice(handlerStart, handlerEnd);
  assert.match(handler, /require_user_actor\(&state, &headers\)/);
  assert.match(handler, /personal_root\(&identity\)/);
  assert.match(handler, /claim_or_authorize_environment\(&identity/);
  assert.doesNotMatch(handler, /RequestActor::Administrator|PROJECTS_DIRECTORY/);
});

test("le backend Git Docker clone sans shell et conserve l'environnement si Docker echoue", () => {
  assert.match(gitDockerBackend, /Command::new\("git"\)/);
  assert.match(
    gitDockerBackend,
    /\.arg\("--"\)\s*\.arg\(repository\)\s*\.arg\(command_path\(target\)\)/,
  );
  assert.match(gitDockerBackend, /GIT_TERMINAL_PROMPT/);
  assert.match(gitDockerBackend, /docker_status: "failed"\.to_string\(\)/);
  assert.match(gitDockerBackend, /Le projet a ete clone, mais/);
  assert.doesNotMatch(gitDockerBackend, /cmd\.exe|\/bin\/sh|-Command/);
});

test("le terminal temporaire de login reste dans le home du compte sans projet", () => {
  assert.match(
    desktop,
    /let project_dir = if login_only \{\s*account_home\.clone\(\)\s*\} else \{\s*resolve_terminal_environment/,
  );
  assert.match(
    server,
    /if request\.login_only \{[\s\S]*?workspace_id_for_dir\(&canonical_home\)[\s\S]*?canonical_home\.clone\(\)/,
  );
});

test("le navigateur ne relance plus les anciens terminaux absents", () => {
  const restoreStart = main.indexOf("const restoreTerminals = async () =>");
  const restoreEnd = main.indexOf("const readClaudeDesignSessions", restoreStart);
  const restore = main.slice(restoreStart, restoreEnd);

  assert.match(restore, /const liveRemoteKeys = new Set<string>\(\)/);
  assert.match(restore, /await attachRestoredTerminal\(session, record\.ptyId, \{[\s\S]*?nodeId: record\.nodeId[\s\S]*?remoteId: record\.remoteId/);
  assert.match(restore, /const staleSessions = restored\.filter/);
  assert.match(restore, /session\.terminal\.dispose\(\)/);
  assert.match(restore, /const restartable = isRemoteMode\(\) \? \[\] : restored/);
  assert.doesNotMatch(
    restore,
    /const restartable = isRemoteMode\(\) \? restored/,
  );
});

test("les terminaux actifs sont adoptes entre mobile et PC", () => {
  assert.match(server, /\.route\("\/terminals\/active", get\(api_list_active_terminals\)\)/);
  assert.match(server, /fn active_for_actor[\s\S]*?!session\.login_only[\s\S]*?session\.owner_id == actor\.owner_id\(\)/);
  assert.match(server, /source_terminal_key: Option<String>/);
  assert.match(platform, /case "list_active_terminals":[\s\S]*?listRemoteActiveTerminals/);
  assert.match(platform, /"\/api\/terminals\/active"/);
  assert.match(platform, /status === 404 \|\| status === 405/);
  assert.match(platform, /"\/legacy-active-terminals\.json"/);
  assert.match(platform, /compatibilityFallback: true/);
  assert.match(main, /const refreshActiveTerminals = async \(forceReattach = false\): Promise<boolean>/);
  assert.match(main, /await ensureTerminalsRestored\(\)[\s\S]*?await attachRestoredTerminal\(session, ptyId\)/);
  assert.match(main, /compatibilityFallback \? selectedAccount\(\)/);
  assert.match(main, /Terminal existant #\$\{String\(ptyId\)\.slice\(-6\)\}/);
  assert.match(main, /sourceTerminalKey: session\.key/);
});

test("l'arret Freebuff distant termine tout l'arbre de processus", () => {
  assert.match(server, /fn terminate_remote_terminal_process_tree[\s\S]*?taskkill\.exe[\s\S]*?\/T[\s\S]*?\/F/);
  const stopStart = server.indexOf("fn stop_for_actor");
  const stopEnd = server.indexOf("fn get(&self", stopStart);
  const stop = server.slice(stopStart, stopEnd);
  assert.match(stop, /terminate_remote_terminal_process_tree\(/);
  assert.doesNotMatch(stop, /\.child[\s\S]*?\.kill\(\)/);
  assert.match(server, /fn abort_started_session[\s\S]*?terminate_remote_terminal_process_tree/);
  assert.match(
    server,
    /async fn api_stop_terminal[\s\S]*?terminals\.stop_for_actor\(id, &actor\)/,
  );
  assert.match(server, /socket_generation: AtomicU64/);
  assert.match(server, /session\.socket_generation\.load\(Ordering::Acquire\) == socket_generation/);
});

test("le transport terminal gere les trames invalides et les suppressions idempotentes", () => {
  assert.match(platform, /let message: RemoteWsMessage/);
  assert.match(platform, /Message WebSocket terminal invalide/);
  assert.match(platform, /hasCamelCaseWorkspace/);
  assert.match(platform, /hasSnakeCaseWorkspace/);
  assert.match(platform, /hasCamelCaseWorkspace \|\| hasSnakeCaseWorkspace/);
  assert.match(server, /#\[serde\(rename = "workspaceId"\)\][\s\S]*?workspace_id: String/);
  assert.match(server, /#\[serde\(rename = "workspacePath"\)\][\s\S]*?workspace_path: String/);
  assert.match(platform, /for \(let attempt = 0; attempt < 3; attempt \+= 1\)/);
  assert.match(platform, /httpStatus === 404/);
  assert.match(platform, /remoteTerminalPendingWrites\.delete\(id\)/);
  assert.match(platform, /function isRemoteWsMessage\(value: unknown\)/);
  assert.match(platform, /terminalId: remoteTerminalServerId\(Number\(args\.terminalId\)\)/);
  assert.match(platform, /Ne jamais rejouer ni router vers un autre noeud un POST/);
});

test("les IDs de terminaux restent uniques quand plusieurs noeuds repartent de 1", () => {
  assert.match(platform, /remoteTerminalLocalIds/);
  assert.match(platform, /nextRemoteTerminalVirtualId/);
  assert.match(platform, /registerRemoteTerminalId\(localId, result\.route\)/);
  assert.match(platform, /remoteTerminalServerId\(id\)/);
  assert.match(platform, /const virtualId = registerRemoteTerminalId\(response\.id, route\)/);
});

test("une coupure de transport ne ferme jamais un PTY encore vivant", () => {
  const reconnectStart = platform.indexOf("function scheduleRemoteTerminalReconnect");
  const reconnectEnd = platform.indexOf("function emitTerminalTransportError", reconnectStart);
  const reconnect = platform.slice(reconnectStart, reconnectEnd);
  assert.match(reconnect, /terminalReconnectPlan/);
  assert.match(reconnect, /openTerminalSocket/);
  assert.doesNotMatch(reconnect, /emit\("pty-exit"/);
  assert.doesNotMatch(reconnect, /remoteTerminalRoutes\.delete/);
  assert.doesNotMatch(reconnect, /remotePendingTerminalInput\.clear/);

  const socketStart = platform.indexOf("function openTerminalSocket");
  const socketEnd = platform.indexOf("function writeRemoteTerminal", socketStart);
  const socket = platform.slice(socketStart, socketEnd);
  assert.match(socket, /message\.type === "exit"[\s\S]*?emit\("pty-exit"/);

  const writeStart = platform.indexOf("function writeRemoteTerminal");
  const writeEnd = platform.indexOf("function resizeRemoteTerminal", writeStart);
  const write = platform.slice(writeStart, writeEnd);
  assert.match(write, /session terminal introuvable[\s\S]*?emit\("pty-exit"/i);
});

test("revenir sur Freebuff rattache le transport et restaure le clavier", () => {
  const viewStart = main.indexOf("const setActiveView = (view: AppView)");
  const viewEnd = main.indexOf("const openDiscussionHistory", viewStart);
  const view = main.slice(viewStart, viewEnd);
  assert.match(
    view,
    /activeView === "terminal"[\s\S]*?requestTerminalFocusKey = activeTerminalKey;[\s\S]*?render\(\);[\s\S]*?recoverActiveTerminalInput\(\)/,
  );

  const refreshStart = main.indexOf("const refreshActiveTerminals = async");
  const refreshEnd = main.indexOf("const readClaudeDesignSessions", refreshStart);
  const refresh = main.slice(refreshStart, refreshEnd);
  assert.match(refresh, /activeTerminalsForceReattachRequested \|\|= forceReattach/);
  assert.match(
    refresh,
    /forceReattach && session\.key === activeTerminalKey[\s\S]*?attachRestoredTerminal\(session, ptyId\)/,
  );
  assert.match(
    refresh,
    /recoverActiveTerminalInput[\s\S]*?refreshActiveTerminals\(true\)[\s\S]*?terminal\.focus\(\)/,
  );

  const createStart = main.indexOf("const createTerminalSession = async");
  const mountStart = main.indexOf("const mountExpertTerminals", createStart);
  const create = main.slice(createStart, mountStart);
  const mountEnd = main.indexOf("const createNewTerminalOnce", mountStart);
  const mount = main.slice(mountStart, mountEnd);
  assert.doesNotMatch(create, /terminal\.element\?\.addEventListener/);
  assert.match(
    mount,
    /terminal\.open\(host\)[\s\S]*?addEventListener\("pointerdown", restorePointerFocus, true\)/,
  );
  assert.match(mount, /addEventListener\("pointerup"[\s\S]*?requestAnimationFrame/);
  assert.match(
    mount,
    /querySelector<HTMLTextAreaElement>\("\.xterm-helper-textarea"\)[\s\S]*?focus\(\{ preventScroll: true \}\)/,
  );
  assert.match(
    mount,
    /addEventListener\("keydown"[\s\S]*?event\.key !== "Enter"[\s\S]*?\.xterm-helper-textarea[\s\S]*?restorePointerFocusAfterPaint\(\)/,
  );
  assert.match(mount, /activeModalDialog\(\)[\s\S]*?requestTerminalFocusKey/);

  const eventsStart = main.indexOf("const setupEvents = async");
  const events = main.slice(eventsStart);
  assert.match(events, /visibilitychange[\s\S]*?recoverActiveTerminalInput/);
  assert.match(events, /addEventListener\("pageshow", recoverTerminalOnPageReturn\)/);
  assert.match(events, /addEventListener\("online", recoverTerminalOnPageReturn\)/);
});
