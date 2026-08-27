import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

const main = read("src/main.ts");
const platform = read("src/platform.ts");
const messaging = read("src/messaging.ts");
const server = read("src-tauri/src/server.rs");
const chat = read("src-tauri/src/chat.rs");
const autonomous = read("src-tauri/src/autonomous.rs");
const runtimeSync = read("src-tauri/src/runtime_sync.rs");
const rustLib = read("src-tauri/src/lib.rs");
const measurement = read("scripts/measure-open-ui-baseline.mjs");

test("un canal partage authentifie remplace les polls tours, agents et messagerie", () => {
  assert.match(rustLib, /mod runtime_sync;/);
  assert.match(server, /\.route\("\/runtime", get\(ws_runtime\)\)/);
  assert.match(
    server,
    /async fn ws_runtime[\s\S]*?constant_time_eq[\s\S]*?check_admin_header[\s\S]*?identity_from_headers[\s\S]*?handle_runtime_socket/,
  );
  assert.match(server, /"type": "hello"[\s\S]*?sync\.revision\(\)/);
  assert.match(server, /RecvError::Lagged[\s\S]*?"type": "resync"/);
  assert.match(runtimeSync, /broadcast::channel\(RUNTIME_SYNC_BUFFER\)/);
  assert.match(runtimeSync, /fetch_add\(1, Ordering::Relaxed\)/);
  assert.match(runtimeSync, /PrivateMessages[\s\S]*?is_visible_to/);
  assert.match(server, /Ok\(event\) if event\.is_visible_to\(&user_id\)/);
});

test("les mutations de tours et d'agents publient seulement un sujet leger", () => {
  assert.match(chat, /notify\(RuntimeSyncTopic::ActiveChatTurns\)/);
  assert.match(chat, /active_turn_signal_state\(&snapshot\)/);
  assert.match(chat, /waiting_for_user:[\s\S]*?part_waits_for_user_input/);
  assert.match(
    autonomous,
    /persist_store\(&self\.storage_path, &store\)[\s\S]*?drop\(store\);[\s\S]*?notify_autonomous_agents_changed\(\)/,
  );
  assert.match(server, /conversation_with_read_status[\s\S]*?notify_private_message_participants/);
  assert.match(server, /send_with_images\([\s\S]*?actor\.clone\(\),[\s\S]*?recipient\.clone\(\),[\s\S]*?request\.body,[\s\S]*?request\.images,[\s\S]*?notify_private_message_participants/);
  assert.doesNotMatch(server, /"type": "change"[\s\S]{0,180}(snapshot|agents|turns)/i);
});

test("les ouvertures et fermetures de terminaux publient leur catalogue", () => {
  assert.match(runtimeSync, /ActiveTerminals/);
  assert.match(server, /notify\(RuntimeSyncTopic::ActiveTerminals\)/);
  assert.match(platform, /\| "activeTerminals"/);
  assert.match(main, /queueRuntimeSyncUpdate\("activeTerminals"\)/);
  assert.match(main, /message\.topic === "activeTerminals"/);
});

test("la fin quotidienne d'un compte est synchronisee entre web, desktop et mobile", () => {
  assert.match(runtimeSync, /AccountCompletions/);
  assert.match(server, /notify\(RuntimeSyncTopic::AccountCompletions\)/);
  assert.match(platform, /\| "accountCompletions"/);
  assert.match(main, /const refreshAccountCompletions = async \(\): Promise<boolean>/);
  assert.match(main, /freshSettings\.accounts\.map\(\(account\) => \[account\.id, account\.completedOn \?\? null\]\)/);
  assert.match(main, /message\.topic === "accountCompletions"/);
  assert.match(main, /queueRuntimeSyncUpdate\("accountCompletions"\)/);
});

test("le client se reconnecte et ne poll plus tant que tous les sockets sont actifs", () => {
  assert.match(platform, /subscribeRuntimeUpdates\(/);
  assert.match(platform, /new WebSocket\(`\$\{wsBase\}\/ws\/runtime\?\$\{query\.toString\(\)\}`\)/);
  assert.match(platform, /states\.every\(\(state\) => state\.live\)[\s\S]*?onState\?\.\("live"\)/);
  assert.match(platform, /Math\.min\(10_000, 500 \* 2 \*\* Math\.min\(state\.retryCount - 1, 5\)\)/);
  assert.match(main, /const fallback = runtimeSyncState !== "live"/);
  assert.match(main, /fallback && autonomousAgentsTracking[\s\S]*?2_000/);
  assert.match(main, /fallback && activeChatTurnsTracking[\s\S]*?1_000/);
  assert.match(main, /setMessagingRealtimeAvailable\(!fallback\)/);
  assert.match(messaging, /messagingRealtimeAvailable \|\| !messagingPollRerender[\s\S]*?clearMessagingPollTimer/);
  assert.match(messaging, /MESSAGING_POLL_INTERVAL_MS = 8_000/);
  assert.match(main, /else \{\s*clearAutonomousAgentsPoll\(\);\s*\}/);
  assert.match(main, /else \{\s*clearActiveChatTurnsPoll\(\);\s*\}/);
});

test("un echec REST live est retente sans boucle rapide ni sujet arbitraire", () => {
  assert.match(main, /const runtimeSyncRetryTimers = new Map<RuntimeSyncTopic, number>\(\)/);
  assert.match(main, /topic === "activeChatTurns"[\s\S]*?return 1_000[\s\S]*?topic === "autonomousAgents"[\s\S]*?return 2_000[\s\S]*?topic === "accountCompletions"[\s\S]*?return 3_000[\s\S]*?return 8_000/);
  assert.match(main, /if \(success\) clearRuntimeSyncRetry\(topic\);\s*else scheduleRuntimeSyncRetry\(topic\);/);
  assert.match(
    main,
    /message\.topic === "activeChatTurns"[\s\S]*?message\.topic === "autonomousAgents"[\s\S]*?message\.topic === "accountCompletions"[\s\S]*?message\.topic === "privateMessages"/,
  );
  assert.doesNotMatch(main, /setTimeout\(flushRuntimeSyncUpdates, 100\)/);
});

test("le scenario navigateur refuse toute reapparition des quatre polls signales", () => {
  assert.match(measurement, /syncMode === "runtime-signal"/);
  assert.match(measurement, /count !== 0[\s\S]*?attendu 0 avec \/ws\/runtime/);
  assert.match(measurement, /runtimeConnectionOpenBeforeWindow/);
  assert.match(measurement, /fermeture du WebSocket runtime pendant la mesure/);
});
