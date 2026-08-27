import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { resolveChatTurnWindow } from "../src/chat/render-window.ts";
import { precompressDirectory } from "../scripts/precompress-frontend.mjs";

const read = (path) => readFile(new URL("../" + path, import.meta.url), "utf8");

test("le chat mobile ne monte que la fenetre de tours recente", () => {
  assert.deepEqual(resolveChatTurnWindow(60, 24), {
    requestedLimit: 24,
    hiddenTurnCount: 36,
    visibleTurnCount: 24,
  });
  assert.deepEqual(resolveChatTurnWindow(60, null), {
    requestedLimit: null,
    hiddenTurnCount: 0,
    visibleTurnCount: 60,
  });
  assert.equal(resolveChatTurnWindow(60, 0).visibleTurnCount, 1);
});

test("le patch de streaming ne remplace que le dernier tour", async () => {
  const [view, main] = await Promise.all([
    read("src/chat/view.ts"),
    read("src/main.ts"),
  ]);

  assert.match(view, /export const renderChatLatestTurn/);
  assert.match(view, /const latest = turns\.at\(-1\)/);
  assert.match(view, /resolveChatTurnWindow\(turns\.length, model\.visibleTurnLimit\)/);
  assert.match(main, /const latestHtml = renderChatLatestTurn/);
  assert.match(main, /latest\.outerHTML = latestHtml/);
});

test("le rafraichissement de streaming compare les references avant de serialiser", async () => {
  const main = await read("src/main.ts");
  const start = main.indexOf("type ChatFeedSnapshot");
  const end = main.indexOf("const patchChatFeedContent", start);
  const implementation = main.slice(start, end);

  assert.ok(start >= 0 && end > start);
  assert.match(implementation, /const chatLatestContentStable/);
  assert.match(implementation, /previous\.messages === model\.messages && previous\.latestParts === model\.parts/);
  assert.doesNotMatch(implementation, /latestSignature: JSON\.stringify/);
});

test("le build produit des variantes Brotli et Gzip reutilisables", async () => {
  const root = await mkdtemp(join(tmpdir(), "cst-precompress-"));
  try {
    const assets = join(root, "assets");
    const sourcePath = join(assets, "app.js");
    const source = "const mobileFluidity = true;\n".repeat(400);
    await mkdir(assets);
    await writeFile(sourcePath, source);

    const result = await precompressDirectory({ root, minimumBytes: 1 });
    const [original, brotliInfo, gzipInfo, server] = await Promise.all([
      readFile(sourcePath, "utf8"),
      stat(sourcePath + ".br"),
      stat(sourcePath + ".gz"),
      read("src-tauri/src/server.rs"),
    ]);

    assert.equal(result.sourceFiles, 1);
    assert.equal(original, source);
    assert.ok(brotliInfo.size < Buffer.byteLength(source));
    assert.ok(gzipInfo.size < Buffer.byteLength(source));
    assert.match(server, /\.precompressed_br\(\)/);
    assert.match(server, /\.precompressed_gzip\(\)/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("le profil Android retire les effets couteux des surfaces fixes", async () => {
  const [html, css, main] = await Promise.all([
    read("index.html"),
    read("src/style.css"),
    read("src/main.ts"),
  ]);

  assert.match(html, /CodexTerminalAndroid/);
  assert.match(html, /classList\.add\("native-android"\)/);
  assert.match(css, /:root\.native-android \.m-topbar/);
  assert.match(css, /backdrop-filter: none/);
  assert.match(css, /:root\.native-android \.eh-accretion/);
  assert.match(main, /renderChatLatestTurn/);
  assert.match(main, /latest\.outerHTML = latestHtml/);
  assert.match(main, /scheduleIdleTask\(\(\) => \{\s*void refreshSkills\(\);\s*void refreshAutonomousAgents\(\);\s*void refreshLimitStatus\(true\);/);
});

test("le compositeur Android masque les outils et montre l'intensite du modele", async () => {
  const [css, view] = await Promise.all([
    read("src/style.css"),
    read("src/chat/view.ts"),
  ]);

  assert.match(
    css,
    /:root\.native-android \.chat-agent-tools\s*\{[^}]*display:\s*none\s*!important;/,
  );
  assert.match(
    css,
    /:root\.native-android \.chat-app-layout \.chat-effort-select\s*\{[^}]*display:\s*inline-flex;/,
  );
  assert.match(view, /data-chat-control="reasoning-effort"/);
  assert.match(view, /option\.value === model\.selectedReasoningEffort/);
});

test("la vue terminal mobile reserve presque tout l'ecran au PTY", async () => {
  const [css, main] = await Promise.all([
    read("src/style.css"),
    read("src/main.ts"),
  ]);

  assert.match(
    main,
    /classList\.toggle\(\s*"m-terminal-focus",\s*activeView === "terminal" && !!activeTerminal\(\),\s*\)/,
  );
  assert.match(main, /activeView === "terminal" \? "is-terminal" : ""/);
  assert.match(
    css,
    /body\.m-terminal-focus \.m-topbar,[\s\S]*body\.m-terminal-focus \.m-bottomnav,[\s\S]*body\.m-terminal-focus \.autonomous-monitor-host,[\s\S]*\.chat-app-layout\.is-terminal \.folder-terminal-head,[\s\S]*\.chat-app-layout\.is-terminal \.folder-agent-summary[\s\S]*display: none !important;/,
  );
  assert.match(
    css,
    /\.chat-app-layout\.is-terminal \.expert-terminal-pane \{[\s\S]*grid-template-rows: 30px minmax\(0, 1fr\);/,
  );
  assert.match(
    css,
    /\.chat-app-layout\.is-terminal \.expert-terminal-pane:not\(\.active\),[\s\S]*\.expert-terminal-pane\.active ~ \.expert-terminal-empty \{\s*display: none;/,
  );
  assert.match(
    css,
    /\.chat-app-layout\.is-terminal \.expert-pane-identity,[\s\S]*\.chat-app-layout\.is-terminal \.expert-pane-status \{\s*display: none;/,
  );
  assert.match(
    main,
    /class="expert-pane-mobile-menu" data-toggle-chat-sidebar[^>]*aria-label="Afficher le menu de gauche"[^>]*aria-controls="chatAppSidebar"/,
  );
  assert.match(main, /class="expert-pane-mobile-menu-arrow" aria-hidden="true"/);
  assert.match(
    css,
    /\.chat-app-layout\.is-terminal \.expert-pane-mobile-menu \{[\s\S]*display: inline-grid;[\s\S]*grid-column: 1;[\s\S]*background: #f5f5f5;[\s\S]*color: #090909;/,
  );
  assert.match(
    css,
    /\.chat-app-layout\.is-terminal \.expert-pane-mobile-menu-arrow \{[\s\S]*border-top: 3px solid currentColor;[\s\S]*border-right: 3px solid currentColor;[\s\S]*transform: rotate\(45deg\);/,
  );
  assert.match(
    css,
    /\.chat-app-layout\.is-terminal \.expert-pane-toggle-chat \{\s*display: none;/,
  );
});
