import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const types = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const tutorial = readFileSync(new URL("../src/tutorial.ts", import.meta.url), "utf8");
const styles = readFileSync(new URL("../src/tutorial.css", import.meta.url), "utf8");
const mainStyles = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");

test("le Tuto est une vue accessible sur ordinateur et mobile", () => {
  assert.match(types, /\| "tutorial"/);
  assert.match(main, /id="proxyToggle"[^>]*title="Gérer les proxys des comptes"/);
  assert.match(main, /data-act="proxies"[^>]*>[\s\S]*?<span>Proxy<\/span>/);
  assert.match(main, /type TutorialModule = typeof import\("\.\/tutorial"\)/);
  assert.match(main, /if \(view === "tutorial" && !tutorialModule\)/);
  assert.match(main, /case "tutorial":\s*return tutorialModule\?\.renderTutorialPanel\(\) \?\? "";/);
  assert.match(main, /case "tutorial":\s*return "Tuto · Découvrir Switch";/);
  assert.match(main, /#proxyToggle"\)\?\.addEventListener\("click", \(\) => \{\s*openProxyManagerModal\(\);/);
  assert.match(main, /tutorialModule\?\.bindTutorialUi\(\{\s*currentView: activeView,\s*navigate: setActiveView,/);
  const mobileGrid = main.slice(main.indexOf('<div class="m-sheet-grid">'));
  assert.match(mobileGrid, /<div class="m-sheet-grid">\s*<button[^>]+data-act="proxies"/);
});

test("le tableau d'accueil conserve et permet de reprendre la progression", () => {
  assert.match(tutorial, /codex-switch-terminal\.tutorial-progress\.v1/);
  assert.match(tutorial, /localStorage\.getItem\(TUTORIAL_STORAGE_KEY\)/);
  assert.match(tutorial, /localStorage\.setItem\(TUTORIAL_STORAGE_KEY/);
  assert.match(tutorial, /id="tutorialStart"/);
  assert.match(tutorial, /id="tutorialReset"/);
  assert.match(tutorial, /role="progressbar"/);
  assert.match(tutorial, /data-tutorial-step=/);
  assert.match(tutorial, /Reprendre à l’étape/);
});

test("le parcours montre les zones essentielles sans déclencher d'action métier", () => {
  for (const id of [
    "welcome",
    "environment",
    "chats",
    "terminal",
    "activity",
    "agents",
    "tools",
    "settings",
    "finish",
  ]) {
    assert.match(tutorial, new RegExp(`id: "${id}"`));
  }
  assert.match(tutorial, /view: "terminal"/);
  assert.match(tutorial, /view: "settings"/);
  assert.match(tutorial, /view: "tutorial"/);
  assert.match(tutorial, /runtime\.navigate\(step\.view\)/);
  assert.doesNotMatch(tutorial, /invoke\(|start_chat_turn|autonomous_agent_create/);
});

test("le coach contextuel est accessible au clavier et s'adapte à la cible", () => {
  assert.match(tutorial, /role="dialog" aria-modal="true"/);
  assert.match(tutorial, /data-tutorial-previous/);
  assert.match(tutorial, /data-tutorial-next/);
  assert.match(tutorial, /event\.key === "Escape"/);
  assert.match(tutorial, /event\.key === "ArrowRight"/);
  assert.match(tutorial, /event\.key === "ArrowLeft"/);
  assert.match(tutorial, /event\.key === "Tab"/);
  assert.match(tutorial, /window\.addEventListener\("resize", reposition/);
  assert.match(tutorial, /window\.addEventListener\("scroll", reposition/);
  assert.match(styles, /\.tutorial-tour-spotlight/);
  assert.match(styles, /body:has\(\.chat-app-layout\.is-tutorial\) #autonomousMonitorLauncher/);
  assert.match(styles, /\.chat-app-layout\.is-tutorial \.chat-admin-head \{[\s\S]*?display: none !important;/);
  assert.match(styles, /@media \(max-width: 700px\)/);
  assert.match(styles, /@media \(prefers-reduced-motion: reduce\)/);
});
