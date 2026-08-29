import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  AUTOMATIC_ORCHESTRATION_MARKER,
  MAX_ORCHESTRATION_WORKER_COUNT,
  automaticOrchestrationEnabledByDefault,
  automaticOrchestrationNotice,
  normalizeOrchestrationWorkerCount,
  orchestrationMinimumTaskCount,
  orchestrationRequestedProjectDir,
  orchestrationIsRunning,
  orchestrationOrchestratorAccountId,
  orchestrationPhaseLabel,
  orchestrationProgress,
  orchestrationStatusLabel,
  orchestrationTaskStatusLabel,
  orchestrationWorkerAccountId,
  orchestrationWorkerCountFromEnv,
  parseAutomaticOrchestrationDecision,
} from "../src/chat/orchestration.ts";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const chatView = readFileSync(new URL("../src/chat/view.ts", import.meta.url), "utf8");
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
const backend = readFileSync(new URL("../src-tauri/src/orchestration.rs", import.meta.url), "utf8");
const chat = readFileSync(new URL("../src-tauri/src/chat.rs", import.meta.url), "utf8");
const server = readFileSync(new URL("../src-tauri/src/server.rs", import.meta.url), "utf8");
const lib = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");

const run = (overrides = {}) => ({
  status: "active",
  phase: "working",
  currentTurnId: null,
  currentStartId: null,
  currentValidationId: null,
  tasks: [
    { id: "task-01", status: "accepted" },
    { id: "task-02", status: "revision_requested" },
  ],
  ...overrides,
});

test("les statuts et la progression du chat orchestré sont explicites", () => {
  assert.equal(orchestrationStatusLabel("needs_attention"), "Attention requise");
  assert.equal(orchestrationPhaseLabel("final_validation"), "Validation finale");
  assert.equal(orchestrationTaskStatusLabel("revision_requested"), "Correction demandée");
  assert.deepEqual(orchestrationProgress(run()), { accepted: 1, total: 2, percent: 50 });
  assert.equal(orchestrationIsRunning(run({ currentValidationId: "validation-1" })), true);
  assert.equal(orchestrationIsRunning(run({ status: "paused", currentTurnId: 42 })), false);
});

test("la décision d'orchestration automatique exige un marqueur structuré valide", () => {
  const decision = parseAutomaticOrchestrationDecision(
    `${AUTOMATIC_ORCHESTRATION_MARKER} {"decision":"orchestrate","workerCount":3,"reason":"trois lots indépendants"}`,
  );
  assert.deepEqual(decision, { workerCount: 3, reason: "trois lots indépendants" });
  assert.equal(
    automaticOrchestrationNotice(decision),
    "Orchestration automatique retenue · 3 workers · trois lots indépendants",
  );
  assert.equal(parseAutomaticOrchestrationDecision("Réponse normale"), null);
  assert.deepEqual(
    parseAutomaticOrchestrationDecision(
      `${AUTOMATIC_ORCHESTRATION_MARKER} {"decision":"orchestrate","workerCount":13}`,
    ),
    { workerCount: 20, reason: "" },
  );
  assert.deepEqual(parseAutomaticOrchestrationDecision(
    `${AUTOMATIC_ORCHESTRATION_MARKER} {"decision":"orchestrate","workerCount":101}`,
  ), { workerCount: 200, reason: "" });
  assert.equal(parseAutomaticOrchestrationDecision(
    `${AUTOMATIC_ORCHESTRATION_MARKER} {"decision":"orchestrate","workerCount":201}`,
  ), null);
  assert.equal(
    parseAutomaticOrchestrationDecision(
      `Le protocole cite ${AUTOMATIC_ORCHESTRATION_MARKER} mais sans ligne de décision.`,
    ),
    null,
  );
});

test("le nombre de workers UI valide la variable Vite et retombe sur trois", () => {
  assert.equal(orchestrationWorkerCountFromEnv("8"), 20);
  assert.equal(orchestrationWorkerCountFromEnv(" 12 "), 20);
  assert.equal(orchestrationWorkerCountFromEnv("21"), 100);
  assert.equal(orchestrationWorkerCountFromEnv("101"), 200);
  assert.equal(orchestrationWorkerCountFromEnv("200"), 200);
  for (const invalid of [undefined, null, "", "0", "201", "2.5", "workers", 8]) {
    assert.equal(orchestrationWorkerCountFromEnv(invalid), 3);
  }
  assert.deepEqual(
    [1, 2, 3, 4, 5, 20, 21, 100, 101, 200].map(normalizeOrchestrationWorkerCount),
    [1, 2, 3, 4, 20, 20, 100, 100, 200, 200],
  );
  assert.equal(MAX_ORCHESTRATION_WORKER_COUNT, 200);
  assert.match(main, /VITE_CST_ORCHESTRATION_WORKERS/);
  assert.match(main, /autonomousLaunchWorkerCount = defaultOrchestrationWorkerCount/);
  assert.match(main, /orchestrationWorkerCount = defaultOrchestrationWorkerCount/);
  assert.match(main, /MAX_ORCHESTRATION_WORKER_COUNT/);
});

test("le snapshot expose la source demandee et le plancher persistant", () => {
  assert.equal(orchestrationRequestedProjectDir({
    projectDir: "C:/private/sandbox",
    requestedProjectDir: "C:/workspace/project",
  }), "C:/workspace/project");
  assert.equal(orchestrationMinimumTaskCount({ workerCount: 3, minimumTaskCount: 20 }), 20);
  assert.match(backend, /pub source_kind: OrchestrationSourceKind/);
  assert.match(backend, /pub requested_project_dir: Option<String>/);
  assert.match(backend, /pub access_project_dir: Option<String>/);
  assert.match(backend, /pub minimum_task_count: u32/);
});

test("l'orchestration par défaut reste réservée aux points d'entrée racine", () => {
  assert.equal(automaticOrchestrationEnabledByDefault({ surface: "classic-chat" }), true);
  assert.equal(automaticOrchestrationEnabledByDefault({
    surface: "classic-chat",
    persistedEnabled: false,
  }), true, "l'ancienne préférence est migrée vers le nouveau défaut");
  assert.equal(automaticOrchestrationEnabledByDefault({
    surface: "classic-chat",
    orchestrationRole: "orchestrator",
  }), false);
  assert.equal(automaticOrchestrationEnabledByDefault({
    surface: "classic-chat",
    orchestrationRole: "worker",
  }), false);
  assert.equal(automaticOrchestrationEnabledByDefault({
    surface: "classic-chat",
    autonomousAgentId: "agent-1",
  }), false);
  assert.equal(automaticOrchestrationEnabledByDefault({
    surface: "freebuff-terminal",
    persistedEnabled: false,
  }), true);
});

test("chaque rôle résout son propre compte avec migration des anciens snapshots", () => {
  const legacy = { accountId: "legacy" };
  assert.equal(orchestrationOrchestratorAccountId(legacy), "legacy");
  assert.equal(
    orchestrationOrchestratorAccountId({ ...legacy, orchestratorAccountId: "pilot" }),
    "pilot",
  );
  assert.equal(
    orchestrationWorkerAccountId(
      { accountId: "legacy", workerAccountIds: ["worker-1", "worker-2"] },
      { position: 2 },
    ),
    "worker-2",
  );
  assert.equal(
    orchestrationWorkerAccountId(
      { accountId: "legacy", workerAccountIds: ["worker-1"] },
      { position: 1, accountId: "replacement" },
    ),
    "replacement",
  );
});

test("les comptes Freebuff ne peuvent plus bloquer une orchestration structurée", () => {
  const optionsStart = main.indexOf("const orchestrationAccountOptions =");
  const optionsEnd = main.indexOf("const normalizeOrchestrationWorkerDrafts", optionsStart);
  const options = main.slice(optionsStart, optionsEnd);
  assert.match(options, /const known = chatCapableAccounts\(\)/);
  assert.match(main, /const resolveOrchestrationDraftAccount =/);
  assert.match(main, /if \(account && accountSupportsChat\(account\)\) return account/);
  assert.match(main, /if \(!accountSupportsChat\(account\)\)/);
  assert.match(backend, /if !orchestration_provider_supported\(account\.provider\)/);
  assert.match(backend, /provider != settings::Provider::Freebuff/);
});

test("la vue dédiée crée et expose chaque chat de l'équipe", () => {
  assert.match(main, /\| "orchestration"/);
  assert.match(main, /id="orchestrationCreateForm"/);
  assert.match(main, /id="orchestrationCreateAdvanced"/);
  assert.match(main, /class="orchestration-required-grid"/);
  assert.match(main, /id="orchestrationRunSelect"/);
  assert.match(main, /class="orchestration-run-details"/);
  assert.doesNotMatch(main, /class="orchestration-overview"/);
  assert.doesNotMatch(main, /class="orchestration-run-rail"/);
  assert.match(main, /id="orchestrationWorkerCount"[^>]*min="1"[^>]*max="\$\{MAX_ORCHESTRATION_WORKER_COUNT\}"/);
  assert.match(main, /id="orchestrationConvertWorkerCount"[^>]*min="1"[^>]*max="\$\{MAX_ORCHESTRATION_WORKER_COUNT\}"/);
  assert.match(main, /orchestratorSessionId: sessionId,[\s\S]*?workerCount|workerCount,[\s\S]*?orchestratorSessionId: sessionId/);
  // Le bouton « Orchestration auto » a été retiré : le défaut est désormais
  // calculé au même endroit que les exclusions anti-récursion testées ci-dessus.
  assert.match(
    main,
    /automaticOrchestrationEnabled:\s*automaticOrchestrationEnabledByDefault\(/,
  );
  assert.match(main, /testCommand: orchestrationTestCommandDraft\.trim\(\) \|\| "git diff --check"/);
  assert.match(main, /discussionForSession\(allDiscussions\(\), accountId, sessionId\)/);
  assert.match(main, /workerCount,/);
  assert.match(main, /workerAccountIds,/);
  assert.match(main, /data-orchestration-account-role/);
  assert.match(main, /Adresse e-mail \/ compte/);
  assert.match(main, /reassign_orchestration_account/);
  assert.match(main, /\$\{workerCount \+ 1\} agents/);
  assert.match(main, /id="orchestrationToggle"/);
  assert.match(main, /data-view="orchestration"/);
  assert.match(main, /create_orchestration/);
  assert.match(main, /control_orchestration/);
  assert.match(main, /delete_orchestration/);
  assert.match(main, /data-orchestration-open-session/);
  assert.match(main, /Preuve du travailleur/);
  assert.match(main, /Dernière revue orchestrateur/);
  assert.match(style, /\.orchestration-panel/);
  assert.match(style, /\.orchestration-task-list/);
  assert.match(style, /\.orchestration-workbench/);
  assert.match(style, /Fenêtre d’orchestration simplifiée/);
  assert.match(style, /\.orchestration-create-advanced/);
  assert.match(style, /\.orchestration-run-picker/);
  assert.match(style, /\.orchestration-member-account/);
  assert.match(style, /@media \(max-width: 860px\)[\s\S]*\.m-sheet-grid button\.m-orchestration-entry/);
});

test("un chat normal route automatiquement ses demandes sans ancien bouton dans le bandeau", () => {
  // Le bouton « Orchestration auto » du composer a ete retire de l'UI ; la
  // machinerie d'orchestration (manuelle et systeme) reste en place.
  assert.doesNotMatch(chatView, /toggle-automatic-orchestration/);
  assert.doesNotMatch(chatView, /Orchestration auto ·/);
  assert.doesNotMatch(main, /toggle-automatic-orchestration/);
  assert.match(chatView, /data-chat-action="open-orchestration"/);
  assert.doesNotMatch(chatView, /data-chat-action="[^"\n]*orchestrate/);
  assert.doesNotMatch(chatView, /role: "available" \| "orchestrator" \| "worker"/);
  assert.match(style, /\.chat-agent-tool--orchestration\[aria-pressed="true"\]/);
  assert.match(chatView, /managedByOrchestration \? `<footer class="chat-orchestration-managed"/);
  assert.match(main, /automaticOrchestrationRoutingSkill/);
  assert.match(main, /parseAutomaticOrchestrationDecision/);
  assert.match(main, /launchAutomaticOrchestration/);
  const routingDecision = main.indexOf("const shouldLaunchAutomaticOrchestration");
  const launchReservation = main.indexOf("pane.automaticOrchestrationLaunching = true", routingDecision);
  const sessionAttachment = main.indexOf("attached = await attachment", routingDecision);
  assert.ok(routingDecision >= 0 && launchReservation > routingDecision && sessionAttachment > launchReservation);
  assert.match(main, /text: automaticOrchestrationNotice\(automaticDecision\)/);
  assert.match(main, /else if \(automaticOrchestrationPending\) \{[\s\S]*?else if \(!chatTurnIsBusy\(snapshot\.status\)\)/);
  assert.match(main, /id="autonomousOrchestrationAccount"/);
  assert.match(main, /data-autonomous-orchestration-worker=/);
  assert.match(main, /workerAccountIds: state\.workerAccountIds\.slice/);
  assert.match(main, /orchestratorSessionId: sessionId/);
  assert.match(main, /pane\.orchestrationRole = "orchestrator"/);
  assert.match(main, /run\.tasks\.forEach\(\(task\) =>/);
  assert.match(main, /orchestrationRole: "worker"/);
  assert.match(main, /syncOrchestrationChatPanes/);
  assert.match(main, /activeView === "chat" && expertChatPanes\.some\(\(pane\) => !!pane\.orchestrationId\)/);
  assert.match(backend, /pub orchestrator_session_id: Option<String>/);
  assert.match(backend, /orchestrator_session_id: orchestrator_session_id\.clone\(\)/);
  assert.match(backend, /session_is_busy\(&account_id, session_id\)/);
  assert.match(backend, /Ok\(true\) => return/);
});

test("desktop et serveur partagent le contrat API orchestré", () => {
  for (const command of [
    "list_orchestrations",
    "create_orchestration",
    "control_orchestration",
    "reassign_orchestration_account",
    "delete_orchestration",
  ]) {
    assert.match(platform, new RegExp(`case "${command}"`));
    assert.match(lib, new RegExp(`orchestration::${command}`));
  }
  assert.match(server, /"\/orchestrations"/);
  assert.match(server, /"\/orchestrations\/:id\/control"/);
  assert.match(server, /"\/orchestrations\/:id\/account"/);
  assert.match(server, /check_admin_header\(&state, &headers\)/);
});

test("le pilotage est asynchrone et concurrent pour les grandes équipes", () => {
  assert.match(backend, /DRIVER_COUNT_ENV: &str = "CST_ORCHESTRATION_DRIVERS"/);
  assert.match(backend, /DEFAULT_DRIVER_COUNT: usize = 4/);
  assert.match(backend, /fn spawn_drivers/);
  assert.match(backend, /fn driver_loop/);
  assert.match(backend, /run_locks: Mutex<HashMap<String, Arc<Mutex<\(\)>>>>/);
  assert.match(backend, /lock\.try_lock\(\)/);
  assert.match(backend, /cst-orchestrated-chats-/);
  assert.match(backend, /const MAX_WORKER_COUNT: u32 = 200/);
  assert.match(backend, /MAX_MAX_CONCURRENCY: u32 = 200/);
  assert.match(backend, /MAX_TASK_COUNT: u32 = 200/);
});

test("le moteur impose isolation, preuve, revue, test réel et publication prudente", () => {
  assert.match(backend, /worktree", "add", "--detach"/);
  assert.match(backend, /ORCHESTRATION_PLAN:/);
  assert.match(backend, /pub worker_count: u32/);
  assert.match(backend, /validate_worker_count/);
  assert.match(backend, /exactement cette cardinalite de taches/);
  assert.match(backend, /validate_plan\(plan, run\)/);
  assert.match(backend, /\[1, 2, 3, 4, 20, 100, 200\]/);
  assert.match(backend, /clone", "--no-local", "--no-checkout", "--no-tags"/);
  assert.match(chat, /ChatFilesystemScope::OrchestrationWorkspace/);
  assert.match(backend, /copy_discussion_between/);
  assert.match(backend, /export_transcript_for_account/);
  assert.match(backend, /handoff_pending/);
  assert.match(backend, /ORCHESTRATION_PROOF:/);
  assert.match(backend, /ORCHESTRATION_REVIEW:/);
  assert.match(backend, /ORCHESTRATION_FINAL:/);
  assert.match(backend, /proof\.tests\.iter\(\)\.any\(\|test\| !test\.passed\)/);
  assert.match(backend, /run_validation_command/);
  assert.match(backend, /protocol_failures/);
  assert.match(backend, /reprise automatique en cours/);
  assert.match(backend, /"diff", "--cached"/);
  assert.match(backend, /Le projet source a change de commit pendant l'orchestration/);
  assert.match(backend, /git_sandboxes_apply_review_commit_and_publish_patch/);
});
