import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { discussionIdentityKey } from "../src/chat/sidebar.ts";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const discussions = readFileSync(
  new URL("../src-tauri/src/discussions.rs", import.meta.url),
  "utf8",
);
const terminal = readFileSync(
  new URL("../src-tauri/src/terminal.rs", import.meta.url),
  "utf8",
);
const server = readFileSync(
  new URL("../src-tauri/src/server.rs", import.meta.url),
  "utf8",
);
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");

test("une discussion Freebuff propose les autres comptes Freebuff", () => {
  const start = main.indexOf("const continuationAccountsForDiscussion =");
  const end = main.indexOf("\n// freebuff n'autorise", start);
  assert.ok(start >= 0 && end > start);
  const selector = main.slice(start, end);

  assert.match(selector, /provider === "freebuff"/);
  assert.match(selector, /accountProvider\(account\) === "freebuff"/);
  assert.match(main, /const accounts = continuationAccountsForDiscussion\(discussion\)/);
  assert.match(main, /const copyLabel = "Copier \+ reprendre"/);
});

test("une discussion Codex propose aussi les comptes Freebuff comme cible", () => {
  const start = main.indexOf("const continuationAccountsForDiscussion =");
  const end = main.indexOf("\n// freebuff n'autorise", start);
  assert.ok(start >= 0 && end > start);
  const selector = main.slice(start, end);

  assert.match(
    selector,
    /freebuffAccounts = \(settings\?\s*\.accounts \?\? \[\]\)\.filter\(\(account\) => accountProvider\(account\) === "freebuff"/,
  );
  assert.match(
    selector,
    /provider === "codex"[\s\S]*chatCapableAccounts\(\), \s*\.\.\.freebuffAccounts\]/,
  );
});

test("la reprise Codex vers Freebuff importe le transcript et conserve la source", () => {
  const start = main.indexOf('if (targetProvider === "freebuff") {');
  const end = main.indexOf(
    'if (sourceProvider === "codex" && targetProvider === "codex") {',
    start,
  );
  assert.ok(start >= 0 && end > start);
  const transfer = main.slice(start, end);

  assert.match(transfer, /await assertFreebuffAccountAvailable\(target\)/);
  assert.match(transfer, /"export_discussion_transcript"/);
  assert.match(transfer, /"import_codex_transcript_to_freebuff"/);
  assert.match(transfer, /targetAccountId,/);
  assert.match(transfer, /folderPath,/);
  assert.match(transfer, /transcript,/);
  assert.match(transfer, /await launchFreebuffDiscussionTerminal\(copied, target, folderPath\)/);
  assert.match(transfer, /releaseTransferredDiscussion\(discussion, target, transferPane\)/);
  assert.doesNotMatch(transfer, /delete_discussion|archive:\s*true|archiveDiscussionById/);
});

test("la ligne d'historique Codex affiche la disponibilite des comptes Freebuff", () => {
  const start = main.indexOf("const renderDiscussionRow =");
  const end = main.indexOf("const compactOpenHistoryText =", start);
  assert.ok(start >= 0 && end > start);
  const row = main.slice(start, end);

  assert.match(row, /const targetIsFreebuff = accountProvider\(targetAccount\) === "freebuff"/);
  assert.match(row, /const isFreebuffOption = accountProvider\(account\) === "freebuff"/);
  assert.match(row, /unavailable \? " · indisponible \(terminal ouvert\)" : " · disponible"/);
  assert.match(row, /targetIsFreebuff \? `<span class="discussion-account-state/);
  assert.match(
    row,
    /Copier la discussion dans le compte Freebuff choisi, la reprendre dans son terminal et conserver la source dans l’Historique/,
  );
});

test("le bouton Vers Freebuff importe puis ouvre le terminal du compte cible", () => {
  const start = main.indexOf("const importCodexHistoryToFreebuff =");
  const end = main.indexOf("\nconst openDiscussionForSession =", start);
  assert.ok(start >= 0 && end > start);
  const importFlow = main.slice(start, end);

  assert.match(importFlow, /await assertFreebuffAccountAvailable\(target\)/);
  assert.match(
    importFlow,
    /const imported = await invoke<DiscussionSummary>\("import_codex_transcript_to_freebuff"/,
  );
  assert.match(importFlow, /await launchFreebuffDiscussionTerminal\(imported, target, folderPath\)/);
  assert.ok(
    importFlow.indexOf("const imported = await invoke<DiscussionSummary>") <
      importFlow.indexOf("await launchFreebuffDiscussionTerminal(imported"),
    "le terminal ne doit s'ouvrir qu'apres l'import du transcript",
  );
  assert.match(importFlow, /Historique Codex importé et repris dans Freebuff/);
  assert.match(importFlow, /terminal non ouvert : aucun environnement associé/);
});

test("la reprise Freebuff utilise l'identifiant natif et un terminal du compte cible", () => {
  assert.match(main, /const FREEBUFF_RESUME_ID_RE/);
  assert.match(main, /discussion\.resumeId\?\.trim\(\)/);
  assert.match(main, /\? `\$\{base\} --continue \$\{id\}`/);

  const start = main.indexOf("const launchFreebuffDiscussionTerminal =");
  const end = main.indexOf("\n// Reprise dans le compte D'ORIGINE", start);
  assert.ok(start >= 0 && end > start);
  const launch = main.slice(start, end);
  assert.match(launch, /await assertFreebuffAccountAvailable\(target\)/);
  assert.match(launch, /createNewTerminal\(/);
  assert.match(launch, /buildResumeCommand\(resumeId, target\)/);
  assert.match(launch, /providerAgentId\("freebuff"\)/);
  assert.match(launch, /providerAgentId\("freebuff"\),\s*resumeId,/);
});

test("la disponibilite Freebuff est visible et reverifiee avant toute ouverture", () => {
  assert.match(main, /indisponible \(terminal ouvert\)/);
  assert.match(main, /Indisponible · terminal ouvert/);
  assert.match(main, /const assertFreebuffAccountAvailable = async/);
  assert.match(main, /account_limit_status/);
  assert.match(main, /\$\{unavailable \? " disabled" : ""\}/);

  const createStart = main.indexOf("const createNewTerminalOnce =");
  const createEnd = main.indexOf("\nconst createNewTerminal =", createStart);
  const create = main.slice(createStart, createEnd);
  assert.match(create, /wantedProvider === "freebuff"/);
  assert.match(create, /await assertFreebuffAccountAvailable\(account\)/);
  assert.match(create, /activateTerminalSession\(session\)/);
  assert.match(create, /requestTerminalFocusKey = session\.key/);
  assert.match(create, /activeView = "terminal"/);
});

test("le serveur reserve atomiquement un seul terminal par compte Freebuff", () => {
  assert.match(terminal, /starting_freebuff_accounts/);
  assert.match(terminal, /freebuff_instance_busy\(&account_home\)/);
  assert.match(terminal, /freebuff_account_start_is_reserved_atomically/);
  assert.match(server, /starting_freebuff_accounts/);
  assert.match(server, /freebuff_instance_busy\(&canonical_home\)/);
  assert.match(server, /remote_freebuff_account_start_is_reserved_atomically/);
  assert.match(server, /row\.session_busy = true/);
  assert.match(terminal, /fn terminate_terminal_process_tree/);
  assert.match(server, /terminate_remote_terminal_process_tree\(child\.as_mut\(\)\)/);
});

test("le changement de compte Freebuff conserve la source apres la reprise", () => {
  const start = main.indexOf('if (sourceProvider === "freebuff")');
  const end = main.indexOf("\n    const folderPath =", start);
  assert.ok(start >= 0 && end > start);
  const transfer = main.slice(start, end);

  assert.match(transfer, /copy_discussion_to_account/);
  assert.match(transfer, /sessionId: discussion\.sessionId/);
  assert.match(transfer, /launchFreebuffDiscussionTerminal\(copied, target, folderPath\)/);
  assert.match(transfer, /releaseTransferredDiscussion\(discussion, target, transferPane\)/);
  assert.doesNotMatch(transfer, /delete_discussion|archive:\s*true|archiveDiscussionById/);
  assert.ok(
    transfer.indexOf("copy_discussion_to_account") <
      transfer.indexOf("launchFreebuffDiscussionTerminal"),
    "le dossier cible doit etre copie avant l'ouverture du nouveau terminal",
  );
  assert.ok(
    transfer.indexOf("launchFreebuffDiscussionTerminal") <
      transfer.indexOf("closeTerminalSession"),
    "la cible doit etre ouverte avant la fermeture du seul terminal source",
  );
});

test("les historiques ayant le meme identifiant natif restent isoles par compte", () => {
  const source = discussionIdentityKey({ accountId: "freebuff-a", sessionId: "chat-natif" });
  const target = discussionIdentityKey({ accountId: "freebuff-b", sessionId: "chat-natif" });
  assert.notEqual(source, target);

  assert.match(main, /latestDiscussions\.map\(\(discussion\) => \[discussionIdentityKey\(discussion\), discussion\]\)/);
  assert.match(main, /discussionTargetSel\.get\(discussionIdentityKey\(discussion\)\)/);
  assert.match(main, /discussionTargetSel\.set\(discussionIdentityKey\(discussion\), select\.value\)/);
  assert.match(main, /data-target-account="\$\{escapeAttr\(discussion\.accountId\)\}"/);
  assert.match(main, /data-resume-account="\$\{escapeAttr\(discussion\.accountId\)\}"/);
  assert.match(main, /data-open-account="\$\{escapeAttr\(discussion\.accountId\)\}"/);
  assert.match(main, /data-delete-account="\$\{escapeAttr\(discussion\.accountId\)\}"/);
  assert.match(main, /findDiscussion\(id, select\.dataset\.targetAccount\)/);
  assert.match(main, /button\.dataset\.deleteAccount/);
  assert.match(main, /button\.dataset\.openAccount/);
  assert.match(main, /button\.dataset\.resumeAccount/);

  const dragStart = main.indexOf("const clearChatDragUi =");
  const dragEnd = main.indexOf("\n// Rafraichit uniquement l'en-tete", dragStart);
  const drag = main.slice(dragStart, dragEnd);
  assert.match(drag, /const accountId = row\.dataset\.dragAccount/);
  assert.match(drag, /findDiscussion\(sessionId, accountId\)/);
  assert.match(drag, /discussionIdentityKey\(discussion\)/);

  const quotaStart = main.indexOf("const openChatAccountIdsForQuotaSelection =");
  const quotaEnd = main.indexOf("\nconst openChatCountForAccount", quotaStart);
  const quota = main.slice(quotaStart, quotaEnd);
  assert.match(quota, /const paneDiscussionKeys = new Set/);
  assert.match(quota, /discussionIdentityKey\(\{/);
  assert.doesNotMatch(quota, /paneSessionIds/);
});

test("seule une action utilisateur peut archiver ou supprimer une discussion", () => {
  const continuationStart = main.indexOf("const continueDiscussionWith =");
  const continuationEnd = main.indexOf("\nconst freebuffDiscussionForTerminal =", continuationStart);
  const manualStart = main.indexOf("const archiveDiscussionById =");
  const manualEnd = main.indexOf("\nconst openDiscussionArchiveModal =", manualStart);
  assert.ok(continuationStart >= 0 && continuationEnd > continuationStart);
  assert.ok(manualStart >= 0 && manualEnd > manualStart);

  const continuation = main.slice(continuationStart, continuationEnd);
  const manualArchive = main.slice(manualStart, manualEnd);
  assert.doesNotMatch(continuation, /delete_discussion|archiveDiscussionById/);
  assert.match(manualArchive, /"delete_discussion"/);
  // L'action manuelle relaie le choix utilisateur : jamais d'archivage
  // systematique, le flag peut aussi demander l'effacement du disque.
  assert.doesNotMatch(manualArchive, /archive:\s*true/);
  assert.match(manualArchive, /archive:\s*!permanentDelete/);
  assert.equal((main.match(/"delete_discussion"/g) ?? []).length, 1);
});

test("le terminal Freebuff permet de changer de compte dans sa bordure", () => {
  assert.match(main, /const renderFreebuffTerminalAccountSwitch =/);
  assert.match(main, /data-freebuff-terminal-account=/);
  assert.match(main, /switchFreebuffTerminalAccount\(session, select\.value\)/);
  assert.match(main, /sourceTerminalKey: session\.key/);
  assert.match(style, /\.expert-terminal-pane-head\.has-freebuff-account-switch/);
  assert.match(style, /\.freebuff-terminal-account-switch select/);
});

test("le transfert Freebuff retrouve un chat nouveau dans un dossier partage", () => {
  const start = main.indexOf("const freebuffDiscussionForTerminal =");
  const end = main.indexOf("\nconst discussionHasRunningTurn", start);
  assert.ok(start >= 0 && end > start);
  const matcher = main.slice(start, end);
  assert.match(matcher, /session\.startedAtUnix/);
  assert.match(matcher, /discussion\.startedAt/);
  assert.match(matcher, /discussion\.lastActivity/);
  assert.match(matcher, /FREEBUFF_TERMINAL_MATCH_WINDOW_SECONDS/);
  assert.match(matcher, /const findFreebuffDiscussionForTerminal = async/);
  assert.match(matcher, /await refreshDiscussions\(\)/);
  assert.match(matcher, /\[0, 200, 600, 1_200\]/);
  assert.match(
    main.slice(main.indexOf("const switchFreebuffTerminalAccount ="), end),
    /await findFreebuffDiscussionForTerminal\(session\)/,
  );
});

test("le backend copie atomiquement tout le dossier Freebuff", () => {
  assert.match(discussions, /fn copy_freebuff_discussion\(/);
  assert.match(discussions, /copy_directory_tree\(&canonical_chat, &temp_chat\)/);
  assert.match(discussions, /fs::rename\(&temp_chat, &target_chat\)/);
  assert.match(discussions, /freebuff_discussion_summary\(&target_messages, target\)/);
  assert.match(discussions, /resume_id: chat_dir/);
});

test("la suppression Freebuff archive tout le dossier et le retire de l'historique", () => {
  assert.match(
    discussions,
    /settings::Provider::Freebuff => \{\s*delete_freebuff_discussion_impl/,
  );
  assert.match(discussions, /fn delete_freebuff_discussion_impl\(/);
  assert.match(discussions, /\.join\("\.config\/manicode\/projects-archive"\)/);
  assert.match(discussions, /fs::rename\(&canonical_chat, &destination\)/);
  assert.match(discussions, /fs::remove_dir_all\(&canonical_chat\)/);
  assert.doesNotMatch(
    discussions.slice(
      discussions.indexOf("pub fn delete_discussion_for_account"),
      discussions.indexOf("/// OpenCode sait supprimer une session"),
    ),
    /Les sessions freebuff ne sont pas exposees par Switch/,
  );
});

test("la suppression Freebuff evite les rescans bloquants", () => {
  assert.match(discussions, /filter_map\(\|file\| cached_freebuff_summary\(file, account\)\)/);
  assert.match(discussions, /fn find_freebuff_chat_file\(/);
  assert.match(discussions, /cached_rollout_path_for_id\(account, session_id\)/);
  assert.match(discussions, /pub fn discussion_cwd_for_authorization\(/);
  assert.match(server, /discussion_cwd_for_authorization\(account_id, session_id\)/);
  assert.match(server, /spawn_blocking\(move \|\| \{\s*discussions::delete_discussion_for_account/);

  const start = main.indexOf("const deleteDiscussion = async");
  const end = main.indexOf("\nconst openTerminalDeleteModal", start);
  assert.ok(start >= 0 && end > start);
  const deletion = main.slice(start, end);
  assert.match(deletion, /void refreshDiscussions\(\)/);
  assert.doesNotMatch(deletion, /await refreshDiscussions\(\)/);
  assert.match(main, /applyDiscussionsSnapshot\(\{/);
});

test("la reprise Freebuff conserve le defilement et reste collee en bas", () => {
  assert.match(
    main,
    /const discussionGroupsScroller =\s*\n?\s*document\.querySelector<HTMLElement>\("#discussionGroups"\);/,
  );
  assert.match(
    main,
    /const discussionGroupsPinnedToEnd =[\s\S]*discussionGroupsScroller\.scrollHeight -[\s\S]*discussionGroupsScroller\.scrollTop -[\s\S]*discussionGroupsScroller\.clientHeight <=\s*2;/,
  );
  assert.match(
    main,
    /const restoredDiscussionGroups = document\.querySelector<HTMLElement>\("#discussionGroups"\);/,
  );
  assert.match(
    main,
    /restoredDiscussionGroups\.scrollTop = discussionGroupsPinnedToEnd\s*\? restoredDiscussionGroups\.scrollHeight\s*: discussionGroupsScrollTop;/,
  );
});
