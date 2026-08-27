import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const chatView = readFileSync(new URL("../src/chat/view.ts", import.meta.url), "utf8");
const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const chatBackend = readFileSync(new URL("../src-tauri/src/chat.rs", import.meta.url), "utf8");

test("le bandeau d'un chat propose de reprendre une discussion", () => {
  assert.match(chatView, /id="\$\{id\("chatResume"\)\}"/);
  assert.match(chatView, /data-open-discussions/);
  assert.match(chatView, />Reprendre une discussion<\/span>/);
});

test("le bandeau de la grille ouvre la meme liste de discussions", () => {
  assert.match(main, /class="tool-button resume-discussion-button"/);
  assert.match(main, /querySelectorAll<HTMLButtonElement>\("\[data-open-discussions\]"\)/);
});

test("le bouton simple reprendre restaure le dossier puis continue dans le chat", () => {
  assert.match(main, /const activateDiscussionFolder/);
  assert.match(main, /setCurrentWorkspace\(folderPath\)/);
  assert.match(main, /invoke<DiscussionSummary>\("move_discussion"/);
  assert.match(chatBackend, /resolve_project_dir\(&account, request\.project_dir\.as_deref\(\)\)/);

  const start = main.indexOf("const resumeDiscussion = async");
  const end = main.indexOf("\nconst transferredDiscussionStatus", start);
  assert.notEqual(start, -1);
  assert.notEqual(end, -1);
  const resume = main.slice(start, end);

  assert.match(resume, /restoreDiscussionFolder\(discussion\)/);
  assert.match(
    resume,
    /resumeDiscussionInChat\(\s*discussion,\s*discussion\.accountId,\s*folderPath,\s*"continue"/,
  );
  assert.doesNotMatch(resume, /createNewTerminal|activeView\s*=\s*"terminal"/);
  assert.doesNotMatch(main, /resumeSessionInTerminal|Reprendre dans un terminal/);
});

test("deplacer et reprendre continue dans un chat normal sans terminal", () => {
  const start = main.indexOf("const continueDiscussionWith = async");
  const end = main.indexOf("\nconst discussionHasRunningTurn", start);
  assert.notEqual(start, -1);
  assert.notEqual(end, -1);
  const moveAndResume = main.slice(start, end);

  assert.doesNotMatch(moveAndResume, /createNewTerminal/);
  assert.equal((moveAndResume.match(/resumeDiscussionInChat\(/g) ?? []).length, 2);
  assert.match(
    main,
    /const resumeDiscussionInChat = async[\s\S]*?openDiscussionInExpert\(discussion\)[\s\S]*?sendExpertChatMessage\(pane, root, prompt\)/,
  );
  assert.match(
    moveAndResume + main.slice(main.indexOf("const resumeDiscussionInChat = async")),
    /expertChatFullscreenKey = pane\.key/,
    "la continuation doit remplacer aussi un chat source en plein ecran",
  );
});

test("un chat refuse clairement un compte sans authentification", () => {
  assert.match(chatBackend, /account_has_auth_tokens\(&account\)/);
  assert.match(chatBackend, /Compte non authentifie/);
});
