import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const deviceFleet = read("../src-tauri/src/device_fleet.rs");
const server = read("../src-tauri/src/server.rs");
const chat = read("../src-tauri/src/chat.rs");
const auth = read("../src-tauri/src/auth.rs");
const cargo = read("../src-tauri/Cargo.toml");
const compose = read("../compose.yaml");

test("les deux connecteurs USB acceptent une concurrence de développement réglable à 40", () => {
  assert.match(deviceFleet, /CST_DEVICE_ACTION_CONCURRENCY/);
  assert.match(deviceFleet, /MAX_DEVICE_ACTION_CONCURRENCY: usize = 128/);
  assert.match(deviceFleet, /parse_device_action_concurrency\(Some\("40"\)\), 40/);
  assert.match(deviceFleet, /Semaphore::new\(\s*configured_device_action_concurrency\(\)/);
  assert.match(server, /Semaphore::new\(\s*device_fleet::configured_device_action_concurrency\(\)/);
  assert.doesNotMatch(deviceFleet, /Semaphore::new\(8\)/);
  assert.doesNotMatch(server, /Semaphore::new\(8\)/);
  assert.match(
    compose,
    /CST_DEVICE_ACTION_CONCURRENCY: \$\{CST_DEVICE_ACTION_CONCURRENCY:-8\}/,
  );
});

test("les actions de quarante appareils restent sérialisées par téléphone et évitent les reprobes en rafale", () => {
  assert.match(deviceFleet, /manager_claims_forty_distinct_devices_concurrently/);
  assert.match(deviceFleet, /assert_eq!\(claimed_devices\.len\(\), 40\)/);
  assert.match(deviceFleet, /local_device_locks/);
  assert.match(deviceFleet, /TOOL_STATUS_CACHE_TTL: Duration = Duration::from_secs\(5\)/);
  assert.match(deviceFleet, /struct CachedDeviceFleetTools/);
  assert.match(deviceFleet, /ACTION_CLAIM_LEASE_SECONDS: i64 = 300/);
  assert.match(deviceFleet, /DeviceActionKind::PushFile => 180_000/);
});

test("quarante chats et quarante sessions client sont couverts avec une sonde mémoire Windows", () => {
  assert.match(chat, /global_chat_capacity_accepts_forty_and_rejects_the_forty_first_atomically/);
  assert.match(chat, /ChatTurnManager::with_max_active\(40\)/);
  assert.match(chat, /capacite chats atteinte: 40\/40/);
  assert.match(auth, /MAX_SESSIONS_PER_USER: usize = 40/);
  assert.match(auth, /one_user_keeps_forty_concurrent_sessions/);
  assert.match(chat, /GlobalMemoryStatusEx/);
  assert.match(cargo, /Win32_System_SystemInformation/);
});
