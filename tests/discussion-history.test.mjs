import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");

test("ouvrir l'Historique depuis la navigation efface toujours un ancien filtre", () => {
  assert.match(
    main,
    /const openDiscussionHistory = \(search = ""\) => \{\s*discussionSearch = search;\s*setActiveView\("discussions"\);\s*\};/,
  );
  assert.match(
    main,
    /#sideDiscussions"\)\?\.addEventListener\("click", \(\) => \{\s*openDiscussionHistory\(\);/,
  );
  assert.match(main, /\[data-open-discussions\][\s\S]{0,180}openDiscussionHistory\(\)/);
});

test("la recherche technique d'une session reste explicite et temporaire", () => {
  const start = main.indexOf("const openDiscussionForSession =");
  const end = main.indexOf("\n// Rattache une session restauree", start);
  assert.ok(start >= 0 && end > start, "openDiscussionForSession introuvable");
  const source = main.slice(start, end);
  assert.match(source, /openDiscussionHistory\(sessionId\)/);
  assert.doesNotMatch(source, /discussionSearch\s*=/);
});

test("l'Historique expose le total, le nombre filtre et une action Tout afficher", () => {
  assert.match(main, /id="discussionCountSummary"/);
  assert.match(main, /\$\{visible\} affichée\(s\) sur \$\{total\}/);
  assert.match(
    main,
    /id="clearDiscussionSearch"[^>]*>\s*<i data-lucide="list"><\/i><span>Tout afficher<\/span>/,
  );
  assert.match(
    main,
    /#clearDiscussionSearch"\)\?\.addEventListener\("click", \(\) => \{\s*openDiscussionHistory\(\);/,
  );
  const buttonStart = main.indexOf('<button id="clearDiscussionSearch"');
  const buttonEnd = main.indexOf(">", buttonStart);
  assert.ok(buttonStart >= 0 && buttonEnd > buttonStart, "bouton Tout afficher introuvable");
  assert.doesNotMatch(main.slice(buttonStart, buttonEnd), /\bdisabled\b/);
  assert.doesNotMatch(main, /clearButton\.disabled\s*=/);
});

test("l'Historique inclut les chats ouverts qui ne sont pas encore indexes", () => {
  assert.match(main, /const unindexedOpenHistoryPanes\s*=\s*\(\)/);
  assert.match(main, /<strong>Chats ouverts<\/strong>/);
  assert.match(main, /data-open-pane="\$\{escapeAttr\(pane\.key\)\}"/);
  assert.match(main, /const discussionHistoryCounts\s*=\s*\(\)/);
  assert.match(main, /total:\s*groups\.reduce[\s\S]*\+ openPanes\.length/);
  assert.match(main, /Chat en cours"\s*:\s*"Chat ouvert/);
  assert.match(main, /const shortId = identity\.slice\(-8\)/);
});

test("aucune pagination ne tronque les discussions renvoyees par le serveur", () => {
  const start = main.indexOf("const renderDiscussionGroups = () =>");
  const end = main.indexOf("\nconst renderDiscussionsPanel", start);
  assert.ok(start >= 0 && end > start, "renderDiscussionGroups introuvable");
  const source = main.slice(start, end);
  assert.match(source, /group\.discussions\.filter/);
  assert.match(source, /rows\.map\(\(discussion\) => renderDiscussionRow/);
  assert.doesNotMatch(source, /\.slice\s*\(/);
});
