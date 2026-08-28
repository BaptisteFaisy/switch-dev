import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");
const theme = readFileSync(new URL("../src/theme.css", import.meta.url), "utf8");

test("l'interface charge directement la grille de chats", () => {
  assert.match(main, /const renderChatFirstShell = \(\) =>/);
  assert.match(style, /\.chat-app-layout/);
});

test("les agents sont accessibles à gauche et leur configuration reste dans les paramètres", () => {
  const settingsStart = main.indexOf("const renderSettingsPanel = (): string =>");
  const settingsEnd = main.indexOf("\nconst renderActiveAppPanel", settingsStart);
  assert.ok(settingsStart >= 0 && settingsEnd > settingsStart, "panneau Parametres introuvable");
  assert.match(main.slice(settingsStart, settingsEnd), /id="settingsAgents"/);

  const shellStart = main.indexOf("const renderChatFirstShell = () =>");
  const leftNavStart = main.indexOf('<nav class="chat-left-tools"', shellStart);
  const leftNavEnd = main.indexOf("</nav>", leftNavStart);
  assert.ok(leftNavStart > shellStart && leftNavEnd > leftNavStart, "menu gauche introuvable");
  assert.match(main.slice(leftNavStart, leftNavEnd), /id="autonomousToggle"/);

  const footerStart = main.indexOf('<footer class="chat-side-footer">', shellStart);
  const footerEnd = main.indexOf("</footer>", footerStart);
  assert.ok(shellStart >= 0 && footerStart > shellStart && footerEnd > footerStart, "pied de colonne droite introuvable");
  const sidebarFooter = main.slice(footerStart, footerEnd);
  assert.match(sidebarFooter, /id="settingsToggle"/);
  assert.doesNotMatch(sidebarFooter, /id="manageAgents"/);
  assert.match(main, /#settingsAgents"\)\?\.addEventListener\("click", \(\) => \{\s*openAgentsModal\(\);/);
});

test("la navigation mobile reste disponible dans la coque multi-chat", () => {
  const start = main.indexOf("const renderChatFirstShell = () =>");
  const end = main.indexOf("\nconst render = () =>", start);
  assert.ok(start >= 0 && end > start, "coque multi-chat introuvable");
  assert.match(main.slice(start, end), /ensureMobileChrome\(\);/);
});

test("les deux menus séparent les chats à gauche des outils à droite", () => {
  const shellStart = main.indexOf("const renderChatFirstShell = () =>");
  const leftStart = main.indexOf('<nav class="chat-left-tools"', shellStart);
  const leftEnd = main.indexOf("</nav>", leftStart);
  assert.ok(shellStart >= 0 && leftStart > shellStart && leftEnd > leftStart, "menu gauche introuvable");
  const left = main.slice(leftStart, leftEnd);
  for (const id of ["chatOverviewToggle", "autonomousToggle", "orchestrationToggle", "designToggle"]) {
    assert.match(left, new RegExp(`id="${id}"`));
  }
  for (const id of ["messagingToggle", "tasksToggle", "settingsToggle", "sideDiscussions"]) {
    assert.doesNotMatch(left, new RegExp(`id="${id}"`));
  }

  const rightStart = main.indexOf('<aside class="chat-context-sidebar"', shellStart);
  const rightEnd = main.indexOf("</aside>", rightStart);
  assert.ok(rightStart > shellStart && rightEnd > rightStart, "menu droit introuvable");
  const right = main.slice(rightStart, rightEnd);
  const navStart = right.indexOf('<nav class="chat-side-tools" aria-label="Menu droit">');
  const navEnd = right.indexOf("</nav>", navStart);
  assert.ok(navStart >= 0 && navEnd > navStart, "navigation du menu droit introuvable");

  const nav = right.slice(navStart, navEnd);
  const menuStart = nav.indexOf('<div class="chat-side-more-menu"');
  const menuEnd = nav.indexOf("</div>", menuStart);
  assert.ok(menuStart >= 0 && menuEnd > menuStart, "menu Plus introuvable");
  const menu = nav.slice(menuStart, menuEnd);
  const atRest = `${nav.slice(0, menuStart)}${nav.slice(menuEnd + "</div>".length)}`;

  for (const id of ["messagingToggle", "tasksToggle", "scheduledChatToggle", "proxyToggle", "promptsToggle", "sideDiscussions", "dashboardToggle", "limitsToggle", "chatSideMoreToggle"]) {
    assert.match(atRest, new RegExp(`id="${id}"`));
  }
  for (const id of ["messagingToggle", "tasksToggle", "scheduledChatToggle", "proxyToggle", "promptsToggle", "sideDiscussions", "dashboardToggle", "limitsToggle", "settingsToggle", "themeToggle"]) {
    assert.match(main, new RegExp(`#${id}"\\)\\?\\.addEventListener\\("click"`));
  }
  assert.match(atRest, /data-task-nav-count/);
  assert.match(atRest, /data-scheduled-chat-nav-count/);
  assert.match(atRest, /id="chatSideMoreToggle"[^>]*aria-haspopup="menu"[^>]*aria-expanded="\$\{chatSideMoreMenuOpen\}"[^>]*aria-controls="chatSideMoreMenu"/);
  assert.match(right, /id="settingsToggle"/);

  const secondaryIds = [
    "bugReportToggle",
    "poolToggle",
    "videoToggle",
    "forumToggle",
    "auditToggle",
    "skillsToggle",
    "vpsToggle",
  ];
  assert.equal([...menu.matchAll(/role="menuitem"/g)].length, secondaryIds.length);
  for (const id of secondaryIds) {
    assert.match(menu, new RegExp(`id="${id}"`));
    assert.match(main, new RegExp(`#${id}"\\)\\?\\.addEventListener\\("click"`));
  }
  assert.match(main, /querySelectorAll<HTMLButtonElement>\("\[data-open-design\]"\)/);
  assert.match(menu, /data-bug-report-nav-badge/);
  assert.match(main, /const chatSideMoreViews = new Set<AppView>/);
  assert.match(main, /let chatSideMoreMenuOpen = false/);
  assert.match(main, /chatSideMoreMenuOpen = open/);
  assert.match(main, /if \(!wrapper\.isConnected\) return/);
  assert.match(main, /menu\.hidden = !open/);
  assert.match(main, /event\.key === "ArrowDown" \|\| event\.key === "ArrowUp"/);
  assert.match(main, /event\.key === "Escape"/);
  assert.match(main, /trigger\.focus\(\)/);
  assert.match(main, /wrapper\.addEventListener\("focusout"/);
  assert.match(main, /document\.addEventListener\("pointerdown"/);
  assert.match(main, /const positionMenu = \(\): void =>/);
  assert.match(main, /window\.addEventListener\("resize", positionMenu/);

  assert.match(style, /\.chat-app-layout \{[^}]*--chat-context-sidebar-width:\s*236px;[^}]*grid-template-columns:/s);
  assert.match(style, /\.chat-context-sidebar \{[^}]*grid-column:\s*3;[^}]*border-left:/s);
  assert.match(main, /id="chatContextSidebarResizer"[\s\S]*?aria-label="Redimensionner le menu droit"[\s\S]*?aria-controls="chatContextSidebar chatMainWorkspace"/);
  assert.match(main, /const bindChatContextSidebarResizer = \(\) =>/);
  assert.match(main, /CHAT_CONTEXT_SIDEBAR_WIDTH_STORAGE_KEY/);
  assert.match(main, /widthAtPointerStart \+ pointerStartX - event\.clientX/);
  assert.match(main, /event\.key === "ArrowLeft"[\s\S]*?event\.key === "ArrowRight"/);
  assert.match(style, /\.chat-context-sidebar-resizer \{[^}]*cursor:\s*col-resize;[^}]*touch-action:\s*none;/s);
  assert.match(style, /\.chat-app-layout\.is-context-sidebar-compact \.chat-context-sidebar \.chat-context-copy/);
  assert.match(
    style,
    /@media \(max-width: 860px\) \{[\s\S]*?\.chat-context-sidebar \{[^}]*position:\s*fixed;[^}]*inset:\s*0 0 0 auto;[^}]*transform:\s*translateX\(102%\);/,
  );
  assert.match(
    style,
    /body\.chat-context-sidebar-open[\s\S]*?\.chat-context-sidebar \{[^}]*visibility:\s*visible;[^}]*pointer-events:\s*auto;[^}]*transform:\s*translateX\(0\);/,
  );
  assert.match(style, /@media \(max-width: 860px\) \{[\s\S]*?\.chat-context-sidebar-resizer \{ display:\s*none;/);
  assert.match(style, /\.chat-side-more-menu\[hidden\] \{ display: none; \}/);
  assert.match(style, /\.chat-side-more-menu \{[^}]*max-height:[^;]+;[^}]*overflow-y: auto;[^}]*overscroll-behavior-y: contain;/s);
  assert.match(style, /\.chat-context-sidebar \.chat-side-more-menu \{\s*position:\s*fixed;/);
  assert.match(style, /\.chat-side-tools \.chat-side-more-menu > button:focus-visible/);
  assert.match(theme, /:root\[data-theme="light"\] \.chat-side-more-menu/);
});

test("les tâches à faire peuvent rester visibles en bas de la colonne droite", () => {
  const settingsStart = main.indexOf("const renderSettingsPanel = (): string =>");
  const settingsEnd = main.indexOf("\nconst renderActiveAppPanel", settingsStart);
  const settingsPanel = main.slice(settingsStart, settingsEnd);
  assert.match(settingsPanel, /id="chatContextTasksSettingsTitle">Tâches dans la colonne de droite/);
  assert.match(settingsPanel, /data-chat-context-tasks="show"/);
  assert.match(settingsPanel, /data-chat-context-tasks="hide"/);

  const shellStart = main.indexOf("const renderChatFirstShell = () =>");
  const rightStart = main.indexOf('<aside class="chat-context-sidebar"', shellStart);
  const footerStart = main.indexOf('<footer class="chat-side-footer">', rightStart);
  const taskPanelStart = main.indexOf('${chatContextTasksVisible ? renderChatContextTasks(contextTasks) : ""}', rightStart);
  assert.ok(taskPanelStart > rightStart && taskPanelStart < footerStart, "les tâches doivent précéder le pied de colonne");
  assert.match(main, /CHAT_CONTEXT_TASKS_VISIBLE_STORAGE_KEY/);
  assert.match(main, /loadChatContextTasksVisible/);
  assert.match(main, /localStorage\.getItem\(CHAT_CONTEXT_TASKS_VISIBLE_STORAGE_KEY\) !== "false"/);
  assert.match(main, /const CHAT_CONTEXT_TASKS_MIN_WIDTH = 200/);
  assert.match(main, /Math\.max\(CHAT_CONTEXT_TASKS_MIN_WIDTH, preferredWidth\)/);
  assert.match(main, /localStorage\.setItem\(CHAT_CONTEXT_TASKS_VISIBLE_STORAGE_KEY, String\(visible\)\)/);
  assert.match(main, /\.filter\(\(task\) => !task\.completed\)/);
  assert.match(main, /data-context-task-toggle/);
  assert.match(main, /persistTaskItems\(next, accountScopedStorage, accountId\)/);
  assert.match(main, /#chatContextTasksOpenAll, #chatContextTasksMore, #chatContextTasksEmpty/);
  assert.match(style, /\.chat-context-tasks \{[^}]*flex-direction:\s*column;[^}]*border-top:/s);
  assert.match(style, /\.chat-context-tasks input:focus-visible \+ span/);
  assert.match(style, /\.is-context-sidebar-compact \.chat-context-sidebar \.chat-context-tasks \{ display:\s*none; \}/);
});

test("un ancien chunk recharge une seule fois le build courant et restaure la vue", () => {
  assert.match(main, /window\.addEventListener\("vite:preloadError"/);
  assert.match(main, /window\.setTimeout\(\(\) => window\.location\.replace\(target\), 0\)/);
  assert.match(main, /scheduleStaleChunkRecovery\(error\)/);
  assert.doesNotMatch(main, /vite:preloadError[\s\S]{0,240}event\.preventDefault\(\)/);
  assert.match(main, /searchParams\.get\(STALE_CHUNK_BUILD_PARAM\) === __CST_BUILD_ID__/);
  assert.match(main, /searchParams\.set\(STALE_CHUNK_VIEW_PARAM, lazyChunkTargetView\)/);
  assert.match(main, /const recoveredLazyView = consumeRecoveredLazyView\(\);/);
  assert.match(main, /if \(recoveredLazyView && recoveredLazyView !== "chat"\) \{\s*setActiveView\(recoveredLazyView\);/);
});

test("l'ancienne rubrique Discussions s'appelle Historique", () => {
  assert.match(main, /id="sideDiscussions"[^>]*title="Historique [^"]*"[^>]*>[\s\S]*?<strong>Historique<\/strong>/);
  assert.match(main, /data-view="discussions"[^>]*>[\s\S]*?<span>Historique<\/span>/);

  const panelStart = main.indexOf("const renderDiscussionsPanel = () =>");
  const panelEnd = main.indexOf("\nconst refreshDiscussionList", panelStart);
  assert.ok(panelStart >= 0 && panelEnd > panelStart, "panneau Historique introuvable");
  assert.match(main.slice(panelStart, panelEnd), /<strong>Historique<\/strong>/);
});

test("le retour des vues administratives porte un nom accessible", () => {
  assert.match(
    main,
    /id="adminBackChat"[^>]*title="Retour aux chats"[^>]*aria-label="Retour aux chats"/,
  );
  assert.match(main, /data-lucide="arrow-left" aria-hidden="true"/);
});

test("la navigation mobile expose six destinations et une action adaptee au chat", () => {
  const start = main.indexOf("function ensureMobileChrome(): void");
  const end = main.indexOf("\ntype ChatWorkspaceSidebarGroup", start);
  assert.ok(start >= 0 && end > start, "coque mobile introuvable");
  const mobile = main.slice(start, end);
  assert.equal([...mobile.matchAll(/class="m-tab"/g)].length, 6);
  assert.match(mobile, /data-view="chat"/);
  assert.match(mobile, /data-view="mail"/);
  assert.match(mobile, /data-gmail-nav-count/);
  assert.match(mobile, /activeView === "chat" \|\| activeView === "discussions"\) openNewChat\(\)/);
  assert.match(main, /newAction\.classList\.toggle\("is-placeholder", !available\)/);
  assert.match(main, /case "settings":\s*return "Paramètres";/);
  assert.match(style, /padding-top: calc\(var\(--m-topbar-h\)/);
  assert.match(style, /chat-panel--expert:not\(\.active\)/);
  assert.match(style, /\.chat-admin-actions \{\s*display: none !important;/);
});

test("les chats et le centre d'activite redeviennent accessibles apres le dernier terminal", () => {
  assert.match(main, /data-m="drawer"[^>]*aria-label="Voir tous les chats"/);
  assert.match(main, /data-m="context"[^>]*aria-label="Ouvrir la colonne de droite"/);
  assert.match(main, /case "context":[\s\S]*?classList\.toggle\("chat-context-sidebar-open"\)/);
  assert.match(
    main,
    /classList\.toggle\(\s*"m-terminal-focus",\s*activeView === "terminal" && !!activeTerminal\(\),\s*\)/,
  );
  assert.match(
    style,
    /@media \(max-width: 860px\) \{[\s\S]*?\.chat-context-sidebar \{[^}]*position:\s*fixed;[^}]*inset:\s*0 0 0 auto;[^}]*visibility:\s*hidden;[^}]*transform:\s*translateX\(102%\);/,
  );
  assert.match(
    style,
    /body\.chat-context-sidebar-open[\s\S]*?\.chat-context-sidebar \{[^}]*visibility:\s*visible;[^}]*pointer-events:\s*auto;[^}]*transform:\s*translateX\(0\);/,
  );
  assert.match(style, /body\.chat-context-sidebar-open \.chat-sidebar-scrim/);
});

test("le web active automatiquement l'unique environnement du serveur", () => {
  assert.match(main, /!currentWorkspace\(\) && byId\.size === 1/);
  assert.match(main, /setCurrentWorkspace\(onlyWorkspace\.path\)/);
});

test("l'accueil sans chat propose des actions utiles", () => {
  assert.match(main, /class="expert-chat-empty-recent" data-open-chat/);
  assert.match(main, /id="emptyNewChat"/);
  assert.match(main, /data-open-discussions class="tool-button"/);
  assert.match(style, /\.expert-chat-empty-actions/);
});

test("la colonne gauche ne propose plus de bouton nouveau chat", () => {
  assert.doesNotMatch(main, /newChatSide|chat-side-new/);
  assert.doesNotMatch(style, /\.chat-side-new/);
});

test("ouvrir une conversation referme le tiroir mobile", () => {
  assert.match(main, /const openDiscussionChat = async \(discussion:[^{]+\{\s*closeMobileOverlays\(\);/);
});

test("les fenetres gardent le focus clavier et isolent l'arriere-plan", () => {
  assert.match(main, /const syncActiveDialogAccessibility = \(\) =>/);
  assert.match(main, /layout\.inert = !!dialog/);
  assert.match(main, /window\.addEventListener\("keydown", trapActiveDialogFocus, true\)/);
  assert.match(main, /event\.key === "Escape" && workspaceModalOpen/);
  assert.match(main, /restoreDialogTrigger\(returnFocus\)/);
  assert.match(style, /\.terminal-environment-menu[\s\S]*:focus-visible/);
  assert.match(style, /body\.has-active-dialog #app/);
});

test("le pool peut etre demarre et arrete depuis sa vue", () => {
  const start = main.indexOf("const renderPoolPanel = () =>");
  const end = main.indexOf("\nconst renderLimitsPanel = () =>", start);
  assert.ok(start >= 0 && end > start, "panneau pool introuvable");
  const panel = main.slice(start, end);
  assert.match(panel, /id="poolStart"/);
  assert.match(panel, /id="poolStop"/);
  assert.match(panel, /id="poolRuntimeStatus"/);
  assert.match(panel, /Démarrer le pool/);
  assert.match(panel, /Arrêter le pool/);
  assert.match(main, /runtimeStatus\.textContent = poolRuntimeSummary\(\);/);
});

test("le tableau du pool ne pousse pas les actions hors ecran", () => {
  assert.match(main, /class="pool-table-wrap table-wrap" tabindex="0"/);
  assert.match(style, /\.pool-table-wrap \{[^}]*overflow-x: auto;/);
  assert.match(
    style,
    /\.accounts-pool-view > \.pool-panel \{[^}]*flex: 0 0 auto;[^}]*grid-template-rows: repeat\(4, auto\);/,
  );
  assert.match(
    style,
    /\.pool-add,\s*\.pool-import \{\s*grid-template-columns: minmax\(0, 1fr\);/,
  );
});

test("les limites utilisent des cartes responsives sans tableau horizontal", () => {
  assert.doesNotMatch(main, /class="limits-table-wrap table-wrap"/);
  assert.doesNotMatch(main, /class="limits-table"/);
  assert.match(main, /class="limit-card-grid"/);
  assert.match(
    style,
    /\.limit-card-grid \{[^}]*grid-template-columns: repeat\(2, minmax\(0, 1fr\)\);/,
  );
  assert.match(
    style,
    /@media \(max-width: 980px\) \{[\s\S]*?\.limit-card-grid \{\s*grid-template-columns: minmax\(0, 1fr\);/,
  );
  assert.match(
    style,
    /@media \(max-width: 620px\) \{[\s\S]*?\.limit-card\.without-five-hour-window \{[\s\S]*?grid-template-areas:\s*"identity"\s*"weekly"\s*"updated";/,
  );
  assert.match(
    style,
    /\.limits-accounts \{[^}]*overflow: hidden;[^}]*flex: 0 0 auto;/,
  );
});

test("le menu mobile gere son etat et le focus au clavier", () => {
  const menuStart = main.indexOf('id="mobileActionSheet"');
  const gridStart = main.indexOf('<div class="m-sheet-grid">', menuStart);
  const gridEnd = main.indexOf("</div>", gridStart);
  assert.ok(menuStart >= 0 && gridStart > menuStart && gridEnd > gridStart, "menu mobile introuvable");
  const mobileMenu = main.slice(gridStart, gridEnd);
  assert.match(main, /data-m="menu" aria-haspopup="menu" aria-expanded="false"/);
  assert.match(main, /id="mobileActionSheet" aria-hidden="true" inert/);
  const menuButtonCount = [...mobileMenu.matchAll(/<button\b/g)].length;
  assert.ok(menuButtonCount >= 14, "entrees du menu mobile incompletes");
  assert.equal([...mobileMenu.matchAll(/role="menuitem"/g)].length, menuButtonCount);
  assert.match(mobileMenu, /role="menuitem" data-view="tasks"/);
  assert.match(mobileMenu, /role="menuitem" data-view="scheduled-chat"/);
  assert.match(mobileMenu, /role="menuitem" data-view="prompts"/);
  assert.doesNotMatch(mobileMenu, /role="menuitem" data-view="doctolib-lab"/);
  assert.match(mobileMenu, /role="menuitem" data-view="autonomous"/);
  assert.match(mobileMenu, /role="menuitem" data-view="orchestration"/);
  assert.match(main, /const syncMobileSheetAccessibility = \(open: boolean/);
  assert.match(main, /sheet\.inert = !open/);
  assert.match(main, /chrome\.addEventListener\("keydown"/);
  assert.match(main, /event\.key === "ArrowDown" \|\| event\.key === "ArrowRight"/);
  assert.match(style, /\.m-sheet-grid button:focus-visible/);
});

test("le menu mobile reste borne et defilable quand la hauteur manque", () => {
  assert.match(
    style,
    /\.m-sheet-panel \{[^}]*max-height: calc\(100dvh - var\(--m-topbar-h\) - env\(safe-area-inset-top\)\);[^}]*overflow-y: auto;[^}]*overscroll-behavior-y: contain;[^}]*-webkit-overflow-scrolling: touch;/,
  );
});

test("le tiroir mobile est retire du clavier lorsqu'il est ferme", () => {
  assert.match(main, /data-m="drawer"[^>]*aria-expanded="false"[^>]*aria-controls="chatAppSidebar"/);
  assert.match(main, /const syncMobileDrawerAccessibility = \(open: boolean/);
  assert.match(main, /sidebar\.inert = mobile && !open/);
  assert.match(main, /#chatSidebarClose"\)\?\.focus\(\)/);
  assert.match(main, /dialogFocusableElements\(sidebar\)/);
  assert.match(main, /window\.addEventListener\("resize", refit\)/);
});

test("la coque admin tablette retire le tiroir et son scrim du flux", () => {
  const mobileSidebarIndex = style.indexOf(".chat-sidebar-collapse,");
  const mobileShellIndex = style.lastIndexOf("@media (max-width: 860px) {", mobileSidebarIndex);
  const mobileShellEnd = style.indexOf("@media (max-height: 700px) {", mobileSidebarIndex);
  const mobileShell = style.slice(mobileShellIndex, mobileShellEnd);

  assert.ok(
    mobileShellIndex >= 0 && mobileSidebarIndex > mobileShellIndex && mobileShellEnd > mobileSidebarIndex,
  );
  assert.match(
    mobileShell,
    /\.chat-app-layout \.chat-app-sidebar\s*\{[^}]*position:\s*fixed;[^}]*inset:\s*0 auto 0 0;[^}]*transform:\s*translateX\(-102%\);/s,
  );
  assert.match(mobileShell, /\.chat-sidebar-resizer\s*\{[^}]*display:\s*none;/s);
  assert.match(
    mobileShell,
    /\.chat-sidebar-scrim\s*\{[^}]*position:\s*fixed;[^}]*inset:\s*0;[^}]*display:\s*block;[^}]*visibility:\s*hidden;/s,
  );
});

test("la coque desktop reste navigable en faible hauteur", () => {
  assert.match(
    style,
    /@media \(max-height: 700px\) \{[\s\S]*?\.chat-app-layout \.chat-app-sidebar \{[^}]*overflow-x:\s*hidden;[^}]*overflow-y:\s*auto;[^}]*overscroll-behavior-y:\s*contain;/,
  );
  assert.match(
    style,
    /@media \(max-height: 700px\) \{[\s\S]*?\.chat-app-layout \.chat-side-conversations \{[^}]*flex:\s*0 0 auto;[^}]*overflow:\s*visible;/,
  );
});

test("le bandeau des chats replie le selecteur de pagination sur tablette", () => {
  assert.match(
    style,
    /@media \(max-width: 1040px\) \{[\s\S]*?\.expert-chat-toolbar-actions > \.expert-page-size-control \{\s*display:\s*none;/,
  );
});

test("toutes les icones statiques ont un composant Lucide", () => {
  const start = main.indexOf("const lucideIcons = {");
  const end = main.indexOf("\n};", start);
  assert.ok(start >= 0 && end > start, "registre d'icones introuvable");
  const registry = main.slice(start, end);
  const componentName = (name) =>
    name.replace(/(\w)(\w*)(_|-|\s*)/g, (_match, first, rest) =>
      first.toUpperCase() + rest.toLowerCase());
  const names = [...main.matchAll(/data-lucide="([a-z0-9-]+)"/g)].map((match) => match[1]);
  const missing = [...new Set(names)].filter(
    (name) => !new RegExp(`\\b${componentName(name)}\\b`).test(registry),
  );
  assert.deepEqual(missing, []);
});

test("la gestion minimale des comptes reste utilisable sur mobile", () => {
  const panelStart = main.indexOf("const renderAccountsPanel =");
  const panelEnd = main.indexOf("const renderAccountsAndPool =", panelStart);
  const panel = main.slice(panelStart, panelEnd);
  assert.match(panel, /id="addAccountForm"/);
  assert.doesNotMatch(panel, /proxyUrlInput|addProxy|proxySelect/);
  assert.match(style, /@media \(max-width: 720px\) \{[\s\S]*?\.simple-account-add \{\s*grid-template-columns: 1fr;/);
  assert.match(style, /@media \(max-width: 520px\) \{[\s\S]*?\.simple-account-row \{[\s\S]*?flex-direction: column;/);
});

test("les informations d'etat visibles respectent une taille et un contraste lisibles", () => {
  assert.match(style, /--muted-2: #8a8a8a;/);
  assert.match(style, /--muted: #858585;/);
  assert.match(style, /\.chat-side-tools button \{[^}]*color: #858585;/);
  assert.match(style, /\.folder-terminal-copy small \{ color: #858585;/);
  assert.match(style, /\.audit-badge \{[^}]*color: #000;/);
  assert.match(style, /\.chat-side-search:focus-within \{[^}]*outline: 2px solid var\(--chat-accent\);/);
  assert.match(style, /\.expert-grid-control:focus-within \{[^}]*outline: 2px solid var\(--accent\);/);
  assert.match(style, /\.chat-admin-head span \{[^}]*font-size: 12px;/);
  assert.match(style, /\.chat-status-toast \{[^}]*font-size: 12px;/);
  assert.match(
    style,
    /\.chat-workspace-empty \{[^}]*color: var\(--chat-muted\);[^}]*font-size: 12px;/,
  );
  assert.match(style, /\.chat-workspace-overview-copy \{[^}]*overflow: hidden;/);
  assert.match(
    style,
    /\.chat-workspace-overview-copy strong \{[^}]*font: 600 13px[^}]*overflow-wrap: anywhere;/,
  );
  assert.match(
    style,
    /\.chat-workspace-overview-copy small \{[^}]*font-size: 12px;[^}]*overflow-wrap: anywhere;/,
  );
  assert.match(
    style,
    /\.limits-head strong,\s*\.discussions-head > div > strong \{[^}]*text-transform: none;/,
  );
  assert.match(style, /\.discussion-search input \{\s*min-height: 44px;/);
  assert.match(style, /\.account-form-grid input,[^}]*\.discussion-target \{[^}]*min-height: 44px;/);
});
