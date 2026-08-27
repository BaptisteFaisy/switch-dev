import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const discussions = readFileSync(
  new URL("../src-tauri/src/discussions.rs", import.meta.url),
  "utf8",
);
const server = readFileSync(new URL("../src-tauri/src/server.rs", import.meta.url), "utf8");

test("les appareils partagent un seul scan court de l'index des discussions", () => {
  assert.match(discussions, /DISCUSSION_REVISION_CACHE_TTL/);
  assert.match(discussions, /discussion_revision_cache\(\)[\s\S]*?\.lock\(\)/);
  assert.match(
    discussions,
    /cached\.checked_at\.elapsed\(\) < DISCUSSION_REVISION_CACHE_TTL/,
  );
  assert.match(server, /spawn_blocking\(discussions::list_discussions_dashboard\)/);
});

test("les autorisations de workspace sont reutilisees pendant un snapshot", () => {
  assert.match(server, /let mut workspace_authorizations = HashMap::<String, bool>::new\(\)/);
  assert.match(server, /owned_account_ids_for_identity/);
  assert.match(server, /workspace_authorizations[\s\S]*?\.entry\(cwd\.to_string\(\)\)/);
});

test("les pastilles sont actualisees avant la reconciliation detaillee des chats", () => {
  const refreshStart = main.indexOf("const refreshActiveChatTurns = async");
  const refreshEnd = main.indexOf("const clearActiveChatTurnsPoll", refreshStart);
  const source = main.slice(refreshStart, refreshEnd);

  const sidebarRefresh = source.indexOf("if (sidebarChanged) refreshChatSidebarConversations();");
  const detailedReconciliation = source.indexOf("await adoptMissingActiveChatTurns(next);");

  assert.ok(sidebarRefresh >= 0, "le changement de catalogue doit rafraichir la colonne");
  assert.ok(
    sidebarRefresh < detailedReconciliation,
    "la colonne ne doit pas attendre les snapshots detailles des chats",
  );
});

test("un statut recu pendant un glisser-deposer est rejoue a la fin du geste", () => {
  const refreshStart = main.indexOf("const refreshChatSidebarConversations = () =>");
  const refreshEnd = main.indexOf("const activeChatTurnsStatusSignature", refreshStart);
  const source = main.slice(refreshStart, refreshEnd);

  assert.match(
    source,
    /if \(draggedChatSessionId\) \{\s*chatSidebarRefreshPending = true;\s*return;\s*\}/,
  );
  assert.match(
    main,
    /if \(chatSidebarRefreshPending\) \{\s*window\.requestAnimationFrame\(refreshChatSidebarConversations\);\s*\}/,
  );
});
