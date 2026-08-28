import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  AUTOMATIC_ORCHESTRATION_MARKER,
  MAX_ORCHESTRATION_WORKER_COUNT,
  ORCHESTRATION_WORKERS_PER_TESTER,
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
  orchestrationTesterCount,
  orchestrationTesterStatusLabel,
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
  assert.equal(orchestrationPhaseLabel("designing_tests"), "Conception des tests par les orchestrateurs");
  assert.equal(orchestrationPhaseLabel("testing"), "Tests des orchestrateurs dédiés");
  assert.equal(
    orchestrationPhaseLabel("merging"),
    "Fusion des sous-chats par l’orchestrateur",
  );
  assert.equal(orchestrationTaskStatusLabel("revision_requested"), "Correction demandée");
  assert.equal(orchestrationTesterStatusLabel("revision_required"), "Corrections demandées");
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
  assert.deepEqual(parseAutomaticOrchestrationDecision(
    `${AUTOMATIC_ORCHESTRATION_MARKER} {"decision":"orchestrate","workerCount":5}`,
  ), { workerCount: 5, reason: "" });
  assert.equal(parseAutomaticOrchestrationDecision(
    `${AUTOMATIC_ORCHESTRATION_MARKER} {"decision":"orchestrate","workerCount":6}`,
  ), null);
  assert.equal(
    parseAutomaticOrchestrationDecision(
      `Le protocole cite ${AUTOMATIC_ORCHESTRATION_MARKER} mais sans ligne de décision.`,
    ),
    null,
  );
});

test("le nombre de workers UI est borné à cinq et retombe sur cinq", () => {
  assert.equal(orchestrationWorkerCountFromEnv("1"), 1);
  assert.equal(orchestrationWorkerCountFromEnv(" 4 "), 4);
  assert.equal(orchestrationWorkerCountFromEnv("5"), 5);
  for (const invalid of [undefined, null, "", "0", "6", "2.5", "workers", 5]) {
    assert.equal(orchestrationWorkerCountFromEnv(invalid), 5);
  }
  assert.deepEqual(
    [1, 2, 3, 4, 5, 6, 20].map(normalizeOrchestrationWorkerCount),
    [1, 2, 3, 4, 5, null, null],
  );
  assert.equal(MAX_ORCHESTRATION_WORKER_COUNT, 5);
  assert.equal(ORCHESTRATION_WORKERS_PER_TESTER, 5);
  assert.deepEqual(
    [1, 5, 6, 10, 11, 1_000].map(orchestrationTesterCount),
    [1, 1, 2, 2, 3, 200],
  );
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
  }), false, "le bouton désactivé reste désactivé après rechargement");
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
  assert.match(main, /workerCount \+ testerCount \+ 1/);
  assert.match(main, /renderOrchestrationTesters\(run\)/);
  assert.match(main, /Orchestrateur testeur/);
  assert.match(main, /orchestrationTesterId/);
  assert.match(main, /id="orchestrationToggle"/);
  assert.match(main, /data-view="orchestration"/);
  assert.match(main, /create_orchestration/);
  assert.match(main, /control_orchestration/);
  assert.match(main, /delete_orchestration/);
  assert.match(main, /data-orchestration-open-session/);
  assert.match(main, /class="orchestration-member-copy orchestration-member-open"/);
  assert.match(style, /\.orchestration-member-open:focus-visible/);
  assert.match(main, /openOrchestrationSession\(accountId, sessionId\)/);
  assert.match(main, /const orchestrationSessionTarget/);
  assert.match(main, /dismissedOrchestrationWorkerPanes\.delete/);
  assert.match(main, /dismissedOrchestrationTesterPanes\.delete/);
  assert.match(main, /sessionId,[\s\S]*?orchestrationId: target\.run\.id/);
  assert.match(main, /activateExpertChatPane\(pane\)/);
  assert.match(main, /transcript en cours d’initialisation/);
  assert.match(main, /if \(existing\?\.orchestrationId\) \{[\s\S]*?openDiscussionInExpert\(discussion, true\)/);
  assert.match(main, /pane\.resumeSessionId = normalizedSessionId/);
  assert.match(main, /Preuve du travailleur/);
  assert.match(main, /Dernière revue orchestrateur/);
  assert.match(main, /Conversation du groupe/);
  assert.match(main, /orchestrationRequestedProjectDir\(run\)/);
  assert.match(style, /\.orchestration-panel/);
  assert.match(style, /\.orchestration-task-list/);
  assert.match(style, /\.orchestration-workbench/);
  assert.match(style, /Fenêtre d’orchestration simplifiée/);
  assert.match(style, /\.orchestration-create-advanced/);
  assert.match(style, /\.orchestration-run-picker/);
  assert.match(style, /\.orchestration-member-account/);
  assert.match(style, /@media \(max-width: 860px\)[\s\S]*\.m-sheet-grid button\.m-orchestration-entry/);
});

test("un chat normal force immédiatement l'équipe quand le bouton orchestrateur est actif", () => {
  assert.match(chatView, /toggle-automatic-orchestration/);
  assert.match(chatView, /Orchestrateur ·/);
  assert.match(main, /toggle-automatic-orchestration/);
  assert.match(chatView, /data-chat-action="open-orchestration"/);
  assert.doesNotMatch(chatView, /data-chat-action="[^"\n]*orchestrate/);
  assert.doesNotMatch(chatView, /role: "available" \| "orchestrator" \| "worker"/);
  assert.match(style, /\.chat-agent-tool--orchestration\[aria-pressed="true"\]/);
  assert.match(chatView, /managedByOrchestration \? `<footer class="chat-orchestration-managed"/);
  assert.doesNotMatch(main, /automaticOrchestrationRoutingSkill/);
  assert.match(main, /parseAutomaticOrchestrationDecision/);
  assert.match(main, /launchAutomaticOrchestration/);
  assert.match(main, /Le bouton est une commande, pas une suggestion adressee au modele/);
  assert.match(main, /workerCount: 1/);
  assert.match(main, /adaptiveFanout: true/);
  assert.match(main, /maxTaskCount: MAX_ORCHESTRATION_WORKER_COUNT/);
  assert.match(main, /maxConcurrency: MAX_ORCHESTRATION_WORKER_COUNT/);
  assert.match(main, /planification adaptative jusqu’à \$\{MAX_ORCHESTRATION_WORKER_COUNT\} workers/);
  assert.match(main, /reason: "Mode orchestrateur actif · cardinalité choisie par l’orchestrateur"/);
  const sendExpert = main.indexOf("const sendExpertChatMessage");
  const forcedCreation = main.indexOf("if (submission.automaticOrchestration)", sendExpert);
  const ordinaryTurn = main.indexOf('invoke<ChatTurnSnapshot>("start_chat_turn"', sendExpert);
  assert.ok(sendExpert >= 0 && forcedCreation > sendExpert && ordinaryTurn > forcedCreation);
  const orchestrationFallback = main.indexOf(
    'statusText = "Orchestrateur indisponible · lancement du chat direct"',
    forcedCreation,
  );
  assert.ok(
    orchestrationFallback > forcedCreation && orchestrationFallback < ordinaryTurn,
    "le repli direct doit rester dans le chemin d'echec de l'orchestration",
  );
  assert.match(
    main.slice(forcedCreation, ordinaryTurn),
    /pane\.automaticOrchestrationEnabled = false;[\s\S]*?return sendExpertChatMessage\([\s\S]*?submission\.prompt,[\s\S]*?intent/,
  );
  const routingDecision = main.indexOf("const shouldLaunchAutomaticOrchestration");
  const launchReservation = main.indexOf("pane.automaticOrchestrationLaunching = true", routingDecision);
  const sessionAttachment = main.indexOf("attached = await attachment", routingDecision);
  assert.ok(routingDecision >= 0 && launchReservation > routingDecision && sessionAttachment > launchReservation);
  assert.match(main, /text: automaticOrchestrationNotice\(automaticDecision\)/);
  assert.match(main, /else if \(automaticOrchestrationPending\) \{[\s\S]*?else if \(!chatTurnIsBusy\(snapshot\.status\)\)/);
  assert.match(main, /id="autonomousOrchestrationAccount"/);
  assert.doesNotMatch(main, /data-autonomous-orchestration-worker=/);
  assert.match(main, /workerAccountIds: state\.workerAccountIds\.slice/);
  assert.match(main, /Array\.from\(\{ length: decision\.workerCount \}, \(\) => account\.id\)/);
  assert.match(main, /orchestratorSessionId: sessionId \|\| null/);
  assert.match(main, /pane\.orchestrationRole = "orchestrator"/);
  assert.match(main, /run\.tasks\.forEach\(\(task\) =>/);
  assert.match(main, /orchestrationRole: "worker"/);
  assert.match(main, /syncOrchestrationChatPanes/);
  assert.match(main, /activeView === "orchestration"\s*\|\| activeView === "chat"/);
  assert.match(backend, /pub orchestrator_session_id: Option<String>/);
  assert.match(backend, /orchestrator_session_id: orchestrator_session_id\.clone\(\)/);
  assert.match(backend, /session_is_busy\(&account_id, session_id\)/);
  assert.match(backend, /Ok\(true\) => return/);
});

test("les equipes actives sont retrouvees apres reload et dans un autre onglet", () => {
  const syncStart = main.indexOf("const syncOrchestrationChatPanes =");
  const syncEnd = main.indexOf("const releaseOrchestrationChatPanes =", syncStart);
  const sync = main.slice(syncStart, syncEnd);

  assert.match(sync, /orchestrations\.forEach\(\(run\) =>/);
  assert.match(sync, /const activeEnvironment = userEnvironmentPath\(currentWorkspace\(\)\)/);
  assert.match(sync, /const alreadyLinked = expertChatPanes\.some/);
  assert.match(sync, /if \(!groupEnvironment\) return/);
  assert.match(sync, /!alreadyLinked\s*&& run\.status === "completed"/);
  assert.match(sync, /workspaceIdForPath\(groupEnvironment\) !== activeEnvironmentId/);
  assert.match(sync, /if \(!orchestratorPane\) \{[\s\S]*?orchestrationRole: "orchestrator"/);
  assert.doesNotMatch(sync, /boundRunIds\.forEach/);
  assert.match(
    main,
    /activeView === "orchestration"\s*\|\| activeView === "chat"[\s\S]*?startOrchestrationsPoll\(\)/,
  );
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
  assert.match(server, /fn orchestration_access_project_dir\(run: &OrchestrationSnapshot\)/);
  assert.match(server, /run\.access_project_dir[\s\S]*?\.or\(run\.requested_project_dir\.as_deref\(\)\)/);
  assert.match(server, /request\.access_project_dir = Some\(authorized_project_dir/);
  assert.match(server, /request\.owner_id = actor\.user\(\)\.map/);
  assert.match(server, /authorize_existing_environment\(identity, orchestration_access_project_dir\(run\)\)/);
});

test("le pilotage est asynchrone et concurrent pour les grandes équipes", () => {
  assert.match(backend, /DRIVER_COUNT_ENV: &str = "CST_ORCHESTRATION_DRIVERS"/);
  assert.match(backend, /DEFAULT_DRIVER_COUNT: usize = 4/);
  assert.match(backend, /fn spawn_drivers/);
  assert.match(backend, /fn driver_loop/);
  assert.match(backend, /run_locks: Mutex<HashMap<String, Arc<Mutex<\(\)>>>>/);
  assert.match(backend, /lock\.try_lock\(\)/);
  assert.match(backend, /cst-orchestrated-chats-/);
  assert.match(backend, /const DEFAULT_WORKER_LIMIT: u32 = 5/);
  assert.match(backend, /const MAX_WORKER_COUNT: u32 = 1_000/);
  assert.match(backend, /MAX_MAX_CONCURRENCY: u32 = 1_000/);
  assert.match(backend, /MAX_TASK_COUNT: u32 = MAX_WORKER_COUNT/);
  assert.match(backend, /WORKER_LIMIT_ENV: &str = "CST_ORCHESTRATION_MAX_WORKERS"/);
});

test("le moteur impose isolation, preuve, revue, test réel et publication prudente", () => {
  assert.match(backend, /worktree", "add", "--detach"/);
  assert.match(backend, /ORCHESTRATION_PLAN:/);
  assert.match(backend, /pub worker_count: u32/);
  assert.match(backend, /const WORKERS_PER_TESTER: u32 = 5/);
  assert.match(backend, /fn required_tester_count/);
  assert.match(backend, /pub tester_count: u32/);
  assert.match(backend, /pub testers: Vec<OrchestrationTester>/);
  assert.match(backend, /OrchestrationTurnKind::TesterPlan/);
  assert.match(backend, /OrchestrationTurnKind::TesterValidation/);
  assert.match(backend, /OrchestrationTurnKind::MergeReview/);
  assert.match(backend, /ORCHESTRATION_TEST_PLAN:/);
  assert.match(backend, /ORCHESTRATION_TEST_RESULT:/);
  assert.match(backend, /ORCHESTRATION_MERGE:/);
  assert.match(backend, /fn pending_merge_review_task_ids/);
  assert.match(backend, /fn apply_merge_review_acceptance/);
  assert.match(backend, /merge_reviewed_task_ids/);
  assert.match(backend, /testers_all_passed/);
  assert.match(backend, /reset_testers_for_validation/);
  assert.match(backend, /validate_worker_count/);
  assert.match(backend, /exactement cette cardinalite de taches/);
  assert.match(backend, /validate_plan\(plan, run\)/);
  assert.match(backend, /MIN_TASK_COUNT\.\.=max_task_count\.min\(MAX_TASK_COUNT\)/);
  assert.match(backend, /clone", "--no-local", "--no-checkout", "--no-tags"/);
  assert.match(chat, /ChatFilesystemScope::OrchestrationWorkspace/);
  assert.match(backend, /copy_discussion_between/);
  assert.match(backend, /export_transcript_for_account/);
  assert.match(backend, /handoff_pending/);
  assert.match(backend, /ORCHESTRATION_PROOF:/);
  assert.match(backend, /ORCHESTRATION_REVIEW:/);
  assert.match(backend, /ORCHESTRATION_FINAL:/);
  assert.match(backend, /pub team_messages: Vec<OrchestrationTeamMessage>/);
  assert.match(backend, /fn append_team_messages/);
  assert.match(backend, /Fil de coordination du groupe/);
  assert.match(backend, /proof\.tests\.iter\(\)\.any\(\|test\| !test\.passed\)/);
  assert.match(backend, /run_validation_command/);
  assert.match(backend, /protocol_failures/);
  assert.match(backend, /reprise automatique en cours/);
  assert.match(backend, /"diff", "--cached"/);
  assert.match(backend, /Le projet source a change de commit pendant l'orchestration/);
  assert.match(backend, /git_sandboxes_merge_two_finished_workers_before_publishing/);
});
