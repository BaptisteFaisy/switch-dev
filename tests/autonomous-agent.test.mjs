import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  AUTONOMOUS_AGENT_TEMPLATES,
  autonomousAgentIsProjectRadar,
  autonomousAgentIsRunning,
  autonomousAgentsToPause,
  autonomousHumanReportContent,
  autonomousAgentProposals,
  autonomousAgentReports,
  autonomousAgentTemplateById,
  autonomousAgentUnreadReports,
  autonomousConversationEntries,
  autonomousConnectorLabel,
  autonomousInitialMemoryFromChat,
  autonomousMemoryKindLabel,
  autonomousPaymentCheckoutUrl,
  autonomousPaymentStatusLabel,
  autonomousStatusLabel,
  autonomousTestStatusLabel,
  autonomousTriggerLabel,
  autonomousWorkItemStatusLabel,
  autonomousWorkPlanProgress,
  formatAutonomousInterval,
  formatAutonomousPaymentAmount,
  formatAutonomousSchedule,
  normalizeAutonomousConnectors,
  parseAutonomousWatchPaths,
  autonomousRadarIdeaProposal,
  autonomousRadarImplementationAgent,
  autonomousRadarReportIdeas,
  autonomousProposalExecutionAgent,
  toggleAutonomousConnector,
} from "../src/chat/autonomous.ts";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const chatView = readFileSync(new URL("../src/chat/view.ts", import.meta.url), "utf8");
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
const backend = readFileSync(new URL("../src-tauri/src/autonomous.rs", import.meta.url), "utf8");
const mobilePushBackend = readFileSync(new URL("../src-tauri/src/mobile_push.rs", import.meta.url), "utf8");
const chatBackend = readFileSync(new URL("../src-tauri/src/chat.rs", import.meta.url), "utf8");
const modelTools = readFileSync(new URL("../src-tauri/src/chat_model_tools.rs", import.meta.url), "utf8");
const orchestration = readFileSync(new URL("../src-tauri/src/orchestration.rs", import.meta.url), "utf8");
const desktop = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const desktopCapability = readFileSync(new URL("../src-tauri/capabilities/default.json", import.meta.url), "utf8");
const server = readFileSync(new URL("../src-tauri/src/server.rs", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");
const theme = readFileSync(new URL("../src/theme.css", import.meta.url), "utf8");
const gitignore = readFileSync(new URL("../.gitignore", import.meta.url), "utf8");

const agent = (overrides = {}) => ({
  id: "agent-1",
  systemManaged: false,
  name: "Optimiseur web",
  objective: "Optimiser les ressources de la page web",
  role: "Ingénieur performance",
  sourceProposalId: null,
  sourceReportId: null,
  sourceReportIdeaIndex: null,
  accountId: "account-1",
  projectDir: "/project",
  sessionId: null,
  mode: "build",
  model: null,
  reasoningEffort: null,
  connectors: [],
  intervalSeconds: 900,
  status: "active",
  createdAt: 1,
  updatedAt: 1,
  nextRunAt: 1_900,
  lastRunStartedAt: null,
  lastRunFinishedAt: null,
  currentTurnId: null,
  currentStartId: null,
  attemptCount: 0,
  runCount: 0,
  consecutiveFailures: 0,
  modelCapacityRetryCount: 0,
  lastError: null,
  lastSummary: null,
  reports: [],
  proposals: [],
  memory: [],
  memoryStrategy: null,
  workItems: [],
  nextTaskId: null,
  testCommand: null,
  testTimeoutSeconds: 300,
  testStatus: "not_configured",
  currentTestId: null,
  testCompletionPending: false,
  consecutiveTestFailures: 0,
  lastTestStartedAt: null,
  lastTestFinishedAt: null,
  lastTestExitCode: null,
  lastTestDurationMs: null,
  lastTestOutput: null,
  events: [],
  ...overrides,
});

test("les statuts autonomes et leur planning sont lisibles", () => {
  assert.equal(autonomousStatusLabel("active"), "Actif");
  assert.equal(autonomousStatusLabel("needs_attention"), "Attention requise");
  assert.equal(formatAutonomousInterval(900), "15 min");
  assert.equal(formatAutonomousSchedule(agent(), 1_000), "Prochaine étape dans 15 min");
  assert.equal(
    formatAutonomousSchedule(agent({ nextRunAt: 1_006, modelCapacityRetryCount: 2 }), 1_000),
    "Modèle saturé · nouvel essai dans 6 s · tentative #2",
  );
  assert.equal(
    formatAutonomousSchedule(agent({ currentTurnId: 42 }), 1_000),
    "Travail en cours · tour #42",
  );
  assert.equal(autonomousAgentIsRunning(agent({ currentTurnId: 42 })), true);
  assert.equal(autonomousAgentIsRunning(agent({ status: "paused", currentTurnId: 42 })), false);
  assert.deepEqual(
    autonomousAgentsToPause([
      agent({ id: "active" }),
      agent({ id: "system", systemManaged: true }),
      agent({ id: "attention", status: "needs_attention" }),
      agent({ id: "paused", status: "paused" }),
      agent({ id: "completed", status: "completed" }),
    ]).map((candidate) => candidate.id),
    ["active", "attention"],
  );
  assert.equal(formatAutonomousSchedule(agent({ currentTestId: "test-1" }), 1_000), "Validation des tests en cours");
  assert.equal(formatAutonomousSchedule(agent({ currentStartId: "start-1" }), 1_000), "Démarrage du prochain tour");
  assert.equal(autonomousTestStatusLabel("passed"), "Réussi");
  assert.equal(autonomousMemoryKindLabel("user"), "Vous");
  assert.equal(autonomousMemoryKindLabel("supervisor"), "Superviseur");
  assert.equal(autonomousWorkItemStatusLabel("in_progress"), "En cours");
  assert.equal(
    formatAutonomousSchedule(agent({ triggerKind: "workspace_change", nextRunAt: null }), 1_000),
    "En veille · attend une modification",
  );
  assert.equal(
    autonomousTriggerLabel(agent({ triggerKind: "workspace_change" })),
    "Modification du projet",
  );
  assert.deepEqual(
    autonomousWorkPlanProgress(agent({
      workItems: [
        { id: "ui", status: "done" },
        { id: "api", status: "todo" },
        { id: "obsolete", status: "cancelled" },
      ],
    })),
    { done: 1, remaining: 1, total: 2 },
  );
});

test("les connecteurs autonomes sont explicites, ordonnes et dedupliques", () => {
  assert.deepEqual(
    normalizeAutonomousConnectors(["google_calendar", "gmail", "gmail", "inconnu"]),
    ["gmail", "google_calendar"],
  );
  assert.deepEqual(toggleAutonomousConnector(["gmail"], "google_calendar"), ["gmail", "google_calendar"]);
  assert.deepEqual(toggleAutonomousConnector(["gmail", "google_calendar"], "gmail"), ["google_calendar"]);
  assert.equal(autonomousConnectorLabel("google_calendar"), "Google Agenda");
});

test("les paiements affichent le montant ISO et refusent les checkouts dangereux", () => {
  const payment = {
    amountMinor: 1299,
    currency: "EUR",
  };
  assert.match(formatAutonomousPaymentAmount(payment), /12,99/);
  assert.equal(autonomousPaymentStatusLabel("confirmed"), "Confirmé");
  assert.equal(
    autonomousPaymentCheckoutUrl("https://checkout.stripe.com/c/pay/test#fragment")?.toString(),
    "https://checkout.stripe.com/c/pay/test",
  );
  for (const unsafeUrl of [
    "http://checkout.example/pay",
    "https://localhost/pay",
    "https://router/pay",
    "https://checkout.internal/pay",
    "https://127.0.0.1/pay",
    "https://127.1/pay",
    "https://2130706433/pay",
    "https://0x7f000001/pay",
    "https://[::1]/pay",
    "https://[::ffff:127.0.0.1]/pay",
    "https://user:secret@checkout.example/pay",
  ]) {
    assert.equal(autonomousPaymentCheckoutUrl(unsafeUrl), null, unsafeUrl);
  }
});

test("les comptes rendus autonomes restent ordonnes et signalent ceux qui sont nouveaux", () => {
  const legacy = agent({
    runCount: 3,
    lastRunFinishedAt: 120,
    lastSummary: "Idee SUG-001 : ajouter une validation continue.",
  });
  assert.deepEqual(autonomousAgentReports(legacy), [{
    id: "run:agent-1:3",
    createdAt: 120,
    runCount: 3,
    content: "Idee SUG-001 : ajouter une validation continue.",
    readAt: null,
    general: false,
  }]);

  const withHistory = agent({
    lastSummary: "ne doit pas remplacer l'historique",
    reports: [
      { id: "run:agent-1:1", createdAt: 100, runCount: 1, content: "Premier resultat" },
      { id: "run:agent-1:2", createdAt: 200, runCount: 2, content: "Deuxieme resultat" },
    ],
  });
  assert.deepEqual(
    autonomousAgentReports(withHistory).map((report) => report.id),
    ["run:agent-1:2", "run:agent-1:1"],
  );
  assert.deepEqual(
    autonomousAgentUnreadReports(withHistory, new Set(["run:agent-1:1"]))
      .map((report) => report.id),
    ["run:agent-1:2"],
  );

  const serverRead = agent({
    reports: [
      { id: "run:agent-1:3", createdAt: 300, runCount: 3, content: "Deja lu", readAt: 310 },
    ],
  });
  assert.deepEqual(autonomousAgentUnreadReports(serverRead, new Set()), []);

  const supervisor = agent({
    systemManaged: true,
    reports: [
      { id: "run:supervisor:1", createdAt: 100, runCount: 1, content: "Controle interne" },
      { id: "run:supervisor:2", createdAt: 200, runCount: 2, content: "Compte rendu general", general: true },
    ],
  });
  assert.deepEqual(
    autonomousAgentReports(supervisor).map((report) => report.id),
    ["run:supervisor:2"],
  );
});

test("les anciennes references internes disparaissent des comptes rendus affiches", () => {
  assert.equal(
    autonomousHumanReportContent(
      "Compte rendu general - HAUTE : resultat valide. Sources: run:agent-a:1, run:agent-b:2 MOYENNE : suite utile.",
    ),
    "Compte rendu general - HAUTE : resultat valide. MOYENNE : suite utile.",
  );
  assert.equal(
    autonomousHumanReportContent(
      "Le correctif est valide (run:agent-a:3, run:agent-b:4). Action : poursuivre SUG-005 et UI-02.",
    ),
    "Le correctif est valide. Action : poursuivre SUG-005 et UI-02.",
  );

  const legacySupervisor = agent({
    systemManaged: true,
    reports: [{
      id: "run:supervisor:3",
      createdAt: 300,
      runCount: 3,
      general: true,
      content: "CRITIQUE : aucune. Sources: run:agent-a:1, run:agent-b:2 HAUTE : resultat lisible.",
    }],
  });
  assert.equal(
    autonomousAgentReports(legacySupervisor)[0]?.content,
    "CRITIQUE : aucune. HAUTE : resultat lisible.",
  );
});

test("Radar expose chaque idee comme une action implementable", () => {
  assert.equal(autonomousAgentIsProjectRadar(agent({ name: "Radar projet" })), true);
  assert.equal(autonomousAgentIsProjectRadar(agent({
    name: "Veille produit",
    role: "Analyste produit et architecture en lecture seule",
  })), true);
  assert.equal(autonomousAgentIsProjectRadar(agent()), false);

  assert.deepEqual(autonomousRadarReportIdeas({
    content: "IDÉE: Ajouter un cache local — effort faible.\n- Preuve: requêtes répétées.\nIDÉE 2 — Afficher les erreurs réseau — confiance haute.",
  }), [
    "Ajouter un cache local — effort faible. - Preuve: requêtes répétées.",
    "Afficher les erreurs réseau — confiance haute.",
  ]);
  assert.deepEqual(autonomousRadarReportIdeas({
    content: "- Ajouter un cache local.\n- Afficher les erreurs réseau.",
  }), ["Ajouter un cache local.", "Afficher les erreurs réseau."]);
  assert.deepEqual(autonomousRadarReportIdeas({
    content: "Ancien compte rendu libre avec une seule suggestion.",
  }), ["Ancien compte rendu libre avec une seule suggestion."]);

  const report = {
    id: "run:radar:4",
    content: "IDÉE: Ajouter un cache local — effort faible.\nIDÉE 2: Afficher les erreurs réseau — confiance haute.",
  };
  const radar = agent({
    proposals: [
      { id: "proposal-cache", title: "Cache local", objective: "Ajouter un cache local sûr", createdAt: 400, runCount: 4, reportId: report.id },
      { id: "proposal-network", title: "Erreurs réseau", objective: "Afficher une erreur réseau actionnable", createdAt: 400, runCount: 4, reportId: report.id },
    ],
  });
  assert.equal(autonomousRadarIdeaProposal(radar, report, 0)?.id, "proposal-cache");
  assert.equal(autonomousRadarIdeaProposal(radar, report, 1)?.id, "proposal-network");

  const implementation = autonomousRadarImplementationAgent(report.id, 1, [
    agent({ id: "old", sourceReportId: report.id, sourceReportIdeaIndex: 1, createdAt: 10 }),
    agent({ id: "new", sourceReportId: report.id, sourceReportIdeaIndex: 1, createdAt: 20 }),
    agent({ id: "other", sourceReportId: report.id, sourceReportIdeaIndex: 0, createdAt: 30 }),
  ]);
  assert.equal(implementation?.id, "new");
});

test("les propositions autonomes sont normalisees et reliees a leur agent d'execution", () => {
  const proposals = autonomousAgentProposals(agent({
    proposals: [
      { id: " proposal-2 ", title: " ", objective: " Ajouter une alerte réseau ", createdAt: 200, runCount: 2 },
      { id: "proposal-1", title: "Cache local", objective: "Mettre en cache les préférences", createdAt: 100, runCount: 1, reportId: "run:1" },
      { id: "proposal-1", title: "Doublon", objective: "Ignorer", createdAt: 300, runCount: 3 },
      { id: "", title: "Invalide", objective: "Sans identifiant", createdAt: 400, runCount: 4 },
    ],
  }));
  assert.deepEqual(proposals.map((proposal) => proposal.id), ["proposal-2", "proposal-1"]);
  assert.equal(proposals[0].title, "Ajouter une alerte réseau");
  assert.equal(proposals[1].reportId, "run:1");

  const execution = autonomousProposalExecutionAgent("proposal-1", [
    agent({ id: "old", sourceProposalId: "proposal-1", createdAt: 10 }),
    agent({ id: "new", sourceProposalId: "proposal-1", createdAt: 20 }),
    agent({ id: "other", sourceProposalId: "proposal-2", createdAt: 30 }),
  ]);
  assert.equal(execution?.id, "new");
  assert.equal(autonomousProposalExecutionAgent("unknown", []), null);
});

test("le fil autonome fusionne les messages memorises et les reponses de l'agent", () => {
  const conversation = autonomousConversationEntries(agent({
    memory: [
      { id: "m1", kind: "user", content: "Commence par le mobile", createdAt: 100 },
      { id: "m2", kind: "test", content: "npm test: ok", createdAt: 150 },
      { id: "m3", kind: "user", content: "Garde la compatibilite", createdAt: 300 },
    ],
    reports: [
      { id: "r1", createdAt: 200, runCount: 1, content: "Le correctif mobile est valide." },
    ],
  }), 3);
  assert.deepEqual(conversation.map((entry) => [entry.author, entry.content]), [
    ["user", "Commence par le mobile"],
    ["agent", "Le correctif mobile est valide."],
    ["user", "Garde la compatibilite"],
  ]);
});

test("le contexte recent d'un chat normal amorce l'agent sans partager sa session", () => {
  const memory = autonomousInitialMemoryFromChat([
    { role: "user", text: "Ancienne demande " + "x".repeat(700) },
    { role: "assistant", text: "Décision intermédiaire" },
    { role: "user", text: "Nouvel objectif prioritaire" },
  ], 260);
  assert.match(memory, /chat normal/);
  assert.match(memory, /Nouvel objectif prioritaire/);
  assert.ok(memory.length <= 260);
});

test("les agents suggeres couvrent les idees et les corrections frontend et backend", () => {
  assert.deepEqual(
    AUTONOMOUS_AGENT_TEMPLATES.map((template) => template.id),
    ["project_radar", "frontend_bug_fixer", "backend_bug_fixer", "build_publisher"],
  );

  const radar = autonomousAgentTemplateById("project_radar");
  assert.equal(radar?.name, "Radar projet");
  assert.equal(radar?.mode, "plan");
  assert.equal(radar?.requireUserReview, false);
  assert.match(radar?.objective ?? "", /trois id.es nouvelles/);
  assert.match(radar?.objective ?? "", /AUTONOMOUS_PROPOSAL/);
  assert.match(radar?.objective ?? "", /mission pr.cise, born.e et directement ex.cutable/);
  assert.match(radar?.objective ?? "", /AUTONOMOUS_REPORT/);
  assert.match(radar?.objective ?? "", /ne modifie aucun fichier/);

  for (const id of ["frontend_bug_fixer", "backend_bug_fixer"]) {
    const fixer = autonomousAgentTemplateById(id);
    assert.equal(fixer?.mode, "build");
    assert.equal(fixer?.requireUserReview, true);
    assert.match(fixer?.objective ?? "", /un seul bug v.rifi. . la fois/);
    assert.match(fixer?.objective ?? "", /test de non-r.gression/);
  }

  assert.match(autonomousAgentTemplateById("frontend_bug_fixer")?.name ?? "", /frontend/);
  assert.match(autonomousAgentTemplateById("backend_bug_fixer")?.name ?? "", /backend/);
  const publisher = autonomousAgentTemplateById("build_publisher");
  assert.equal(publisher?.triggerKind, "workspace_change");
  assert.equal(publisher?.allowGitPublish, true);
  assert.equal(publisher?.debounceSeconds, 120);
  assert.match(publisher?.objective ?? "", /git push origin HEAD/);
  assert.match(publisher?.objective ?? "", /autres agents du même projet/);
  assert.match(publisher?.objective ?? "", /deploy:web:local/);
  assert.deepEqual(
    parseAutonomousWatchPaths("src\npublic, src; package.json"),
    ["src", "public", "package.json"],
  );
  assert.equal(autonomousAgentTemplateById("unknown"), undefined);
});

test("la nouvelle vue cree et pilote un agent autonome", () => {
  assert.match(main, /\| "autonomous"/);
  assert.match(main, /id="autonomousCreateForm"/);
  assert.match(main, /id="autonomousTemplateTitle">Modèles/);
  assert.match(main, /data-autonomous-template=/);
  assert.match(main, /autonomousAgentTemplateById\(button\.dataset\.autonomousTemplate\)/);
  assert.match(main, /autonomousNameDraft = template\.name/);
  assert.match(main, /autonomousRequireUserReview = template\.requireUserReview/);
  assert.match(main, /id="autonomousToggle"/);
  assert.match(main, /class="chat-side-autonomous-entry/);
  assert.match(main, /class="m-autonomous-entry" role="menuitem" data-view="autonomous"/);
  assert.match(main, /Agents autonomes<\/strong><small>Création et suivi 24\/7/);
  assert.match(main, /create_autonomous_agent/);
  assert.match(main, /control_autonomous_agent/);
  assert.match(main, /send_autonomous_agent_message/);
  assert.match(main, /data-autonomous-message-form=/);
  assert.match(main, /Aiguiller le travail/);
  assert.match(main, /Changer sa mission/);
  assert.match(main, /Chaque message rejoint sa mémoire durable/);
  assert.match(main, /delete_autonomous_agent/);
  assert.match(main, /add_autonomous_agent_memory/);
  assert.match(main, /delete_autonomous_agent_memory/);
  assert.match(main, /schedule_autonomous_agent/);
  assert.match(main, /type="datetime-local"/);
  assert.match(main, /data-autonomous-schedule-open/);
  assert.match(main, /data-autonomous-frequency-input/);
  assert.match(main, /Fréquence récurrente/);
  assert.match(main, /data-autonomous-schedule-open=.*?<span>Planning<\/span>/s);
  assert.match(main, /intervalSeconds/);
  assert.match(main, /id="autonomousTriggerKind"/);
  assert.match(main, /value="workspace_change"/);
  assert.match(main, /id="autonomousWatchPaths"/);
  assert.match(main, /id="autonomousDebounceSeconds"/);
  assert.match(main, /id="autonomousAllowGitPublish"/);
  assert.match(main, /Créer et mettre en veille/);
  assert.match(main, /triggerKind: eventTriggered \? "workspace_change" : "schedule"/);
  assert.match(main, /allowGitPublish: eventTriggered && autonomousAllowGitPublish/);
  assert.match(main, /id="autonomousTestCommand"/);
  assert.match(main, /id="autonomousLaunchMode"/);
  assert.match(main, /value="orchestrator"/);
  assert.match(main, /id="autonomousLaunchWorkerCount"/);
  assert.doesNotMatch(main, /data-autonomous-launch-worker=/);
  assert.match(main, /Phase de test : même compte que l’orchestrateur/);
  assert.match(main, /deferFirstRun: launchOrchestration/);
  assert.match(main, /Agent lancé directement en mode orchestrateur/);
  assert.match(main, /id="autonomousEnvironmentPreset"/);
  assert.match(main, /Autre chemin…/);
  assert.match(main, /id="autonomousRequireUserReview" type="checkbox"/);
  assert.match(main, /requireUserReview: autonomousRequireUserReview/);
  assert.match(main, /data-autonomous-connector=/);
  assert.match(main, /connectors: accountProvider\(account\) === "codex" \? autonomousConnectors : \[\]/);
  assert.match(main, /Lecture autonome\. Envoi d’e-mail/);
  assert.match(main, /ouvre \/plugins, installe Gmail et\/ou Google Calendar/);
  assert.match(main, /L’agent et les connecteurs s’exécutent sur l’hôte du site :8080/);
  assert.match(main, /review\.externalAction/);
  assert.match(main, /Review par l'utilisateur avant d'appliquer les changements/);
  assert.match(main, /data-autonomous-action="testNow"/);
  assert.match(main, /Créer un agent autonome/);
  assert.match(main, /id="autonomousCreateShell"/);
  assert.match(main, /id="autonomousCreateOptions"/);
  assert.match(main, /Plus d’options/);
  assert.match(main, /id="autonomousNewAgent"/);
  assert.match(main, /autonomous-agent-details/);
  assert.match(main, /class="autonomous-agent-more"/);
  assert.match(main, /data-autonomous-agent-more=/);
  assert.match(main, /data-autonomous-agent-card-toggle=/);
  assert.match(
    main,
    /class="autonomous-agent-card-controls">[\s\S]*?\$\{headerLifecycleAction\}[\s\S]*?data-autonomous-agent-card-toggle=/,
  );
  assert.match(main, /class="tool-button autonomous-agent-lifecycle-button" data-autonomous-action="pause"/);
  assert.match(main, /title="Mettre cet agent en pause" aria-label="Mettre /);
  assert.match(main, /id="autonomousPauseAll"/);
  assert.match(main, /autonomousAgentsToPause\(autonomousAgents\)/);
  assert.match(main, /const pauseAllAutonomousAgents = async/);
  assert.match(main, /Promise\.allSettled\(/);
  assert.match(main, /action: "pause"/);
  assert.match(style, /\.autonomous-list-pause-all\s*\{[^}]*min-height:\s*34px/);
  assert.match(style, /\.autonomous-agent-lifecycle-button\s*\{[^}]*min-height:\s*32px/);
  assert.match(
    style,
    /@media \(max-width: 480px\)[\s\S]*?\.autonomous-agent-lifecycle-button span,[\s\S]*?\.autonomous-agent-collapse-button span \{ display: none; \}/,
  );
  assert.match(main, /data-autonomous-agent-section=/);
  assert.match(main, /data-autonomous-section-hide=/);
  assert.match(main, /data-autonomous-panel-toggle="overview"/);
  assert.match(main, /data-autonomous-panel-toggle="templates"/);
  assert.match(main, /id="autonomousCollapseAll"/);
  assert.match(main, /autonomousExpandedAgentIds\.clear\(\)/);
  assert.match(main, /autonomousExpandedAgentSectionKeys\.clear\(\)/);
  assert.match(main, /autonomousAgents\.forEach\(\(agent\) => autonomousCollapsedAgentCards\.set\(agent\.id, true\)\)/);
  assert.match(main, /data-autonomous-edit-open=/);
  assert.match(main, /data-autonomous-edit-form=/);
  assert.match(main, /Enregistrer les modifications/);
  assert.match(main, /Plan de travail/);
  assert.match(main, /Strat.gie de m.moire/);
  assert.match(main, /Prochaine boucle/);
  assert.match(main, /id="autonomousMonitorHost"/);
  assert.match(main, /id="autonomousMonitorLauncher"/);
  assert.match(main, /id="autonomousMonitorWindow"/);
  assert.match(main, /AUTONOMOUS_SEEN_REPORTS_STORAGE_KEY/);
  assert.match(main, /renderAutonomousUnreadInbox/);
  assert.match(main, /data-autonomous-report-open=/);
  assert.match(main, /data-autonomous-report-seen=/);
  assert.match(main, /data-autonomous-radar-implement=/);
  assert.match(main, /prepareAutonomousRadarImplementation/);
  assert.match(main, /autonomousRadarImplementationAgent/);
  assert.match(main, /sourceReportId: autonomousRadarSourceDraft\?\.reportId/);
  assert.match(main, /sourceReportIdeaIndex: autonomousRadarSourceDraft\?\.ideaIndex/);
  assert.match(main, /data-autonomous-monitor-open=.*?Aller au chat/s);
  assert.match(main, /data-autonomous-tab="proposals"/);
  assert.match(main, /id="autonomousProposals"/);
  assert.match(main, /Propositions des agents/);
  assert.match(main, /data-autonomous-proposal-execute=/);
  assert.match(main, /executeAutonomousProposal/);
  assert.match(main, /sourceProposalId: proposal\.id/);
  assert.match(main, /requireUserReview: true/);
  assert.match(main, /autonomousRequireUserReview = true/);
  assert.match(main, /autonomousReportDeliveries\(true\)/);
  assert.match(main, /renderChatTurnParts/);
  assert.match(main, /chat_turn_status/);
  assert.match(main, /data-autonomous-monitor-review="approveReview"/);
  assert.match(main, /data-autonomous-monitor-review="rejectReview"/);
  assert.match(main, /data-autonomous-monitor-delete=/);
  assert.match(main, /data-autonomous-monitor-delete-confirm=/);
  assert.match(main, /data-autonomous-monitor-delete-cancel/);
  assert.match(main, /data-autonomous-account=/);
  assert.match(main, /Adresse e-mail \/ compte/);
  assert.match(main, /data-autonomous-orchestrate=/);
  assert.match(main, /id="autonomousOrchestrationForm"/);
  assert.match(main, /id="autonomousOrchestrationAccount"/);
  assert.doesNotMatch(main, /data-autonomous-orchestration-worker=/);
  assert.match(main, /Même compte que l’orchestrateur pendant le test/);
  assert.doesNotMatch(main, /id="autonomousOrchestrationWorkersUseOrchestrator"/);
  assert.match(main, /promote_autonomous_agent_to_orchestration/);
  assert.match(main, /Bascule sans double exécution/);
  assert.match(main, /Connecteurs non transférés/);
  assert.match(main, /Aucun chat autonome n'est conservé dans l'historique/);
  assert.match(main, /autonomous-agent-delete-trigger/);
  assert.doesNotMatch(main, /data-autonomous-open-chat/);
  assert.doesNotMatch(main, /data-autonomous-monitor-chat/);
  assert.match(main, /vrai fonctionnement 24\/7/);
  assert.match(style, /\.autonomous-agent-card/);
  assert.match(style, /\.autonomous-agent-conversation/);
  assert.match(style, /\.autonomous-agent-message-actions/);
  assert.match(style, /\.autonomous-visibility-bar/);
  assert.match(style, /\.autonomous-create-options/);
  assert.match(style, /\.autonomous-agent-quick-state/);
  assert.match(style, /\.autonomous-agent-more/);
  assert.match(style, /\.autonomous-create-shell:not\(\[open\]\) \{ display: none; \}/);
  assert.match(style, /\.chat-app-layout\.is-autonomous \.chat-admin-head \{ display: none; \}/);
  assert.match(style, /\.autonomous-agent-editor/);
  assert.match(style, /\.autonomous-agent-card\.is-editing/);
  assert.match(theme, /data-theme="light"\] \.autonomous-agent-editor/);
  assert.match(style, /\.autonomous-template-grid/);
  assert.match(style, /\.autonomous-template-card\.tone-frontend/);
  assert.match(style, /\.autonomous-template-card\.tone-backend/);
  assert.match(style, /\.autonomous-template-card\.tone-deploy/);
  assert.match(style, /\.autonomous-event-config/);
  assert.match(style, /\.autonomous-agent-edit-trigger/);
  assert.match(style, /\.autonomous-work-plan/);
  assert.match(style, /@media \(max-width: 760px\)[\s\S]*\.autonomous-panel/);
  assert.match(style, /\.chat-side-tools \.chat-side-autonomous-entry/);
  assert.match(style, /\.m-sheet-grid button\.m-autonomous-entry/);
  assert.match(style, /\.chat-admin-panel > \.autonomous-panel/);
  assert.match(style, /\.chat-app-layout\.is-autonomous \.chat-admin-head/);
  assert.match(style, /\.autonomous-monitor-window/);
  assert.match(style, /\.autonomous-report-inbox/);
  assert.match(style, /\.autonomous-reports/);
  assert.match(style, /\.autonomous-page-tabs/);
  assert.match(style, /\.autonomous-proposal-card/);
  assert.match(style, /\.autonomous-proposals-hero/);
  assert.match(style, /\.autonomous-radar-idea/);
  assert.match(style, /\.autonomous-monitor-host\.tone-report/);
  assert.match(style, /\.autonomous-monitor-delete-confirm/);
  assert.match(style, /\.autonomous-review-toggle/);
  assert.match(style, /\.autonomous-agent-review-policy/);
  assert.match(style, /\.autonomous-connector-access/);
  assert.match(style, /\.autonomous-agent-connectors/);
  assert.match(style, /\.autonomous-monitor-external-notice/);
  assert.match(style, /\.autonomous-schedule-editor/);
  assert.match(style, /@media \(max-width: 860px\)[\s\S]*#autonomousMonitorLauncher/);
});

test("le lanceur des agents autonomes peut etre reduit durablement", () => {
  assert.match(main, /AUTONOMOUS_MONITOR_COMPACT_STORAGE_KEY/);
  assert.match(main, /id="autonomousMonitorCompactToggle"/);
  assert.match(main, /Réduire le bouton Agents autonomes/);
  assert.match(main, /accountScopedStorage\.setItem\(AUTONOMOUS_MONITOR_COMPACT_STORAGE_KEY/);
  assert.match(style, /\.autonomous-monitor-host\.is-compact #autonomousMonitorLauncher/);
  assert.match(style, /\.autonomous-monitor-host\.is-compact \.autonomous-monitor-launcher-copy \{ display: none; \}/);
  assert.match(theme, /data-theme="light"\] #autonomousMonitorCompactToggle/);
});

test("un message pilote l'agent et devient une memoire durable", () => {
  assert.match(backend, /pub struct SendAutonomousAgentMessageRequest/);
  assert.match(backend, /pub enum AutonomousAgentMessageMode/);
  assert.match(backend, /pub fn send_message\(/);
  assert.match(backend, /push_memory\(agent, AutonomousMemoryKind::User, content\.clone\(\), now\)/);
  assert.match(backend, /"objective_changed_by_message"/);
  assert.match(backend, /agent\.memory_strategy = None/);
  assert.match(backend, /agent\.work_items\.clear\(\)/);
  assert.match(backend, /user_message_after_latest_run_start/);
  assert.match(backend, /"stale_run_ignored_after_message"/);
  assert.match(backend, /Les entrees \[utilisateur\] sont des messages explicites/);
  assert.match(theme, /autonomous-agent-conversation/);
});

test("la fenetre de suivi autonome peut etre deplacee et garde sa position", () => {
  assert.match(main, /AUTONOMOUS_MONITOR_POSITION_STORAGE_KEY/);
  assert.match(main, /data-autonomous-monitor-drag-handle/);
  assert.match(main, /handle\.setPointerCapture\(event\.pointerId\)/);
  assert.match(main, /placeAutonomousMonitorWindow/);
  assert.match(main, /persistAutonomousMonitorPosition/);
  assert.match(main, /autonomousMonitorIsFullscreen\(\)/);
  assert.match(style, /\.autonomous-monitor-head \{[^}]*cursor: grab;[^}]*touch-action: none;/s);
  assert.match(style, /\.autonomous-monitor-window\.is-dragging/);
});

test("la fenetre de suivi autonome peut etre redimensionnee et garde sa taille", () => {
  assert.match(main, /AUTONOMOUS_MONITOR_SIZE_STORAGE_KEY/);
  assert.match(main, /data-autonomous-monitor-resize="nw"/);
  assert.match(main, /bindAutonomousMonitorResizeUi/);
  assert.match(main, /handle\.setPointerCapture\(event\.pointerId\)/);
  assert.match(main, /persistAutonomousMonitorSize/);
  assert.match(main, /autonomousMonitorSize = nextSize/);
  assert.match(style, /\.autonomous-monitor-window\.has-custom-size/);
  assert.match(style, /\.autonomous-monitor-resize-handle\[data-autonomous-monitor-resize="se"\]/);
  assert.match(style, /@media \(max-width: 860px\)[\s\S]*\.autonomous-monitor-resize-handle \{ display: none; \}/);
});

test("le moniteur autonome reste compact et masque les details secondaires", () => {
  assert.match(main, /AUTONOMOUS_MONITOR_MIN_WIDTH = 340/);
  assert.match(main, /AUTONOMOUS_MONITOR_MIN_HEIGHT = 300/);
  assert.match(main, /class="autonomous-monitor-agent-picker"/);
  assert.match(main, /class="autonomous-monitor-overview"/);
  assert.match(main, /class="autonomous-monitor-disclosure"/);
  assert.match(main, /class="autonomous-monitor-actions-menu"/);
  assert.match(main, /data-autonomous-monitor-section/);
  assert.match(main, /autonomousMonitorDisclosureState/);
  assert.match(style, /\.autonomous-monitor-window \{[^}]*width: min\(480px,[^}]*height: min\(560px,/s);
  assert.match(style, /\.autonomous-monitor-agent-tabs \{[^}]*position: absolute;/s);
  assert.match(style, /\.autonomous-monitor-actions-menu > div \{[^}]*position: absolute;/s);
  assert.match(style, /\.autonomous-monitor-agent-picker:not\(\[open\]\) > \.autonomous-monitor-agent-tabs \{ display: none; \}/);
  assert.match(style, /\.autonomous-monitor-overview:not\(\[open\]\) > \.autonomous-monitor-agent-head \{ display: none; \}/);
  assert.match(style, /\.autonomous-reports-monitor:not\(\[open\]\) > \.autonomous-reports-content \{ display: none; \}/);
  assert.match(style, /\.autonomous-monitor-disclosure:not\(\[open\]\) > div \{ display: none; \}/);
  assert.match(style, /\.autonomous-monitor-actions-menu:not\(\[open\]\) > div \{ display: none; \}/);
  assert.match(theme, /data-theme="light"\] \.autonomous-monitor-disclosure/);
});

test("desktop et serveur partagent le meme contrat API autonome", () => {
  for (const command of [
    "list_autonomous_agents",
    "create_autonomous_agent",
    "update_autonomous_agent",
    "control_autonomous_agent",
    "schedule_autonomous_agent",
    "reassign_autonomous_agent_account",
    "send_autonomous_agent_message",
    "read_autonomous_review_evidence",
    "add_autonomous_agent_memory",
    "mark_autonomous_agent_report_read",
    "delete_autonomous_agent_memory",
    "delete_autonomous_agent",
    "promote_autonomous_agent_to_orchestration",
  ]) {
    assert.match(platform, new RegExp(`case "${command}"`));
  }
  assert.match(server, /"\/autonomous-agents"/);
  assert.match(
    server,
    /AutonomousAgentManager::new\(chat\.clone\(\), config\.data_dir\.join\("autonomous-agents\.json"\)\)/,
  );
  assert.match(server, /"\/autonomous-agents\/:id\/control"/);
  assert.match(server, /"\/autonomous-agents\/:id\/schedule"/);
  assert.match(server, /"\/autonomous-agents\/:id\/account"/);
  assert.match(server, /"\/autonomous-agents\/:id\/messages"/);
  assert.match(server, /"\/autonomous-agents\/:id\/reviews\/:review_id\/evidence"/);
  assert.match(server, /"\/autonomous-agents\/:id\/memories"/);
  assert.match(server, /"\/autonomous-agents\/:id\/reports\/:report_id\/read"/);
  assert.match(server, /"\/autonomous-agents\/:id\/memories\/:memory_id"/);
  assert.match(server, /"\/autonomous-agents\/:id\/orchestration"/);
  assert.match(server, /"\/autonomous-agents\/:id\/review-policy"/);
  assert.match(server, /api_apply_autonomous_review_policy/);
  assert.match(server, /check_admin_header\(&state, &headers\)/);
  assert.match(platform, /case "create_autonomous_agent":\s*return api<T>\("POST", "\/api\/autonomous-agents", args\.request\)/);
  assert.match(platform, /case "update_autonomous_agent":[\s\S]*?\/api\/autonomous-agents\/\$\{encodeURIComponent/);
  assert.match(server, /post\(api_update_autonomous_agent\)\.delete\(api_delete_autonomous_agent\)/);
  assert.match(desktop, /autonomous::update_autonomous_agent/);
  assert.match(platform, /case "reassign_autonomous_agent_account":[\s\S]*?\/account`/);
  assert.match(platform, /case "send_autonomous_agent_message":[\s\S]*?\/messages`/);
  assert.match(platform, /case "read_autonomous_review_evidence":[\s\S]*?\/evidence`/);
  assert.match(desktop, /autonomous::reassign_autonomous_agent_account/);
  assert.match(desktop, /autonomous::send_autonomous_agent_message/);
  assert.match(desktop, /autonomous::read_autonomous_review_evidence/);
  assert.match(desktop, /autonomous::mark_autonomous_agent_report_read/);
  assert.match(desktop, /orchestration::promote_autonomous_agent_to_orchestration/);
});

test("un chat normal cree, conserve et modifie son agent autonome", () => {
  assert.match(chatView, /data-chat-action="\$\{autonomous\.role === "linked" \? "edit-autonomous" : "autonomize"\}"/);
  assert.match(main, /id="autonomousChatForm"/);
  assert.match(main, /id="autonomousChatAccount"/);
  assert.match(main, /id="autonomousChatModel"/);
  assert.match(main, /id="autonomousChatReasoningEffort"/);
  assert.match(main, /autonomousInitialMemoryFromChat\(pane\.messages\)/);
  assert.match(main, /pane\.autonomousAgentId = saved\.id/);
  assert.match(main, /autonomousAgentId: pane\.autonomousAgentId/);
  assert.match(main, /"update_autonomous_agent"/);
  assert.match(main, /accountId: account\.id/);
  assert.match(main, /Le chat reste une conversation normale/);
  assert.match(main, /Le moteur autonome n'efface ni ne verrouille ce chat/);
  assert.match(main, /id="autonomousChatPause"/);
  assert.match(backend, /pub struct UpdateAutonomousAgentRequest/);
  assert.match(backend, /pub fn update\(/);
  assert.match(backend, /Mets l'agent en pause et attends l'arret du cycle courant/);
  assert.match(backend, /agent\.work_items\.clear\(\)/);
  assert.match(backend, /agent\.memory_strategy = None/);
  assert.match(style, /\.expert-chat-autonomous-action/);
  assert.match(style, /\.autonomous-chat-modal/);
});

test("le modele d'un chat web cree et modifie lui-meme l'agent avec des outils natifs", () => {
  assert.match(main, /sourceChatKey: pane\.key/);
  assert.match(platform, /sourceChatKey: args\.sourceChatKey \?\? null/);
  assert.match(backend, /pub source_chat_key: Option<String>/);
  assert.match(server, /"\/mcp\/chat-tools"/);
  assert.match(server, /start_with_model_tools\(request, tool_server\)/);
  assert.match(server, /chat_tool_capabilities\.claim_call\(token\)/);
  assert.match(chatBackend, /fn autonomous_agent_tool_instructions\(\)/);
  assert.match(chatBackend, /configure_codex_model_tool\(command, model_tool_server\)/);
  assert.match(chatBackend, /--mcp-config/);
  assert.match(chatBackend, /--allowedTools/);
  assert.match(modelTools, /AUTONOMOUS_AGENT_TOOL_NAME: &str = "create_autonomous_agent"/);
  assert.match(
    modelTools,
    /UPDATE_AUTONOMOUS_AGENT_TOOL_NAME: &str = "update_autonomous_agent"/,
  );
  assert.match(
    modelTools,
    /PAUSE_AUTONOMOUS_AGENT_TOOL_NAME: &str = "pause_autonomous_agent"/,
  );
  assert.match(
    modelTools,
    /APPLY_AUTONOMOUS_AGENT_POLICY_TOOL_NAME: &str = "apply_autonomous_agent_policy"/,
  );
  assert.match(
    modelTools,
    /ACTIVATE_SUPERVISOR_GENERAL_REPORT_TOOL_NAME:[\s\S]*"activate_supervisor_general_report"/,
  );
  assert.match(modelTools, /Le modele ne peut donc ni[\s\S]*choisir un autre compte/);
  assert.match(modelTools, /source_chat_key: context\.source_chat_key/);
  assert.match(modelTools, /pub\(crate\) fn linked_agent_for_context/);
  assert.match(modelTools, /agent\.source_chat_key\.as_deref\(\) == Some\(source_chat_key\)/);
  assert.match(modelTools, /"minProperties": 1/);
  assert.doesNotMatch(
    modelTools.match(/pub\(crate\) struct UpdateAutonomousAgentToolArguments \{[\s\S]*?\n\}/)?.[0] ?? "",
    /agent_id|agentId/,
  );
  assert.match(server, /AutonomousAgentAction::Pause/);
  assert.match(server, /tool_update_success_response/);
  assert.match(server, /PAUSE_AUTONOMOUS_AGENT_TOOL_NAME =>/);
  assert.match(server, /tool_pause_success_response/);
  assert.match(server, /agents_for_policy_context/);
  assert.match(server, /apply_review_policy/);
  assert.match(modelTools, /requireVisualEvidence/);
  assert.match(chatBackend, /appelle `update_autonomous_agent`/);
  assert.match(chatBackend, /appelle `pause_autonomous_agent`/);
  assert.match(chatBackend, /appelle `activate_supervisor_general_report`/);
  assert.match(chatBackend, /appelle `apply_autonomous_agent_policy`/);
  assert.match(chatBackend, /capture ou maquette fidele avant autorisation/);
  assert.match(
    modelTools,
    /N'affirme jamais qu'une creation, une modification ou une mise en pause a reussi avant le succes de l'outil/,
  );
});

test("un chat web peut ouvrir un autre chat normal avec son contexte courant", () => {
  assert.match(modelTools, /CREATE_CHAT_TOOL_NAME: &str = "create_chat"/);
  assert.match(modelTools, /pub\(crate\) struct CreateChatToolArguments/);
  assert.match(modelTools, /pub fn claim_chat_creation/);
  assert.match(modelTools, /Un seul nouveau chat peut etre cree par tour/);
  assert.match(modelTools, /account_id: context\.account_id\.clone\(\)/);
  assert.match(modelTools, /project_dir: context\.project_dir\.clone\(\)/);
  assert.match(modelTools, /model: context\.model\.clone\(\)/);
  assert.match(modelTools, /reasoning_effort: context\.reasoning_effort\.clone\(\)/);
  assert.doesNotMatch(
    modelTools.match(/pub\(crate\) struct CreateChatToolArguments \{[\s\S]*?\n\}/)?.[0] ?? "",
    /account_id|accountId|project_dir|projectDir|model|reasoning/,
  );
  assert.match(server, /"\/chat\/open-requests\/claim"/);
  assert.match(server, /chat_open_requests\.enqueue\(request\)/);
  assert.match(platform, /case "claim_chat_open_requests"/);
  assert.match(main, /const claimChatOpenRequests = async/);
  assert.match(main, /const openChatFromModelRequest = async/);
  assert.match(main, /addExpertChatPane\(account\.id/);
  assert.match(main, /startNewChatWithPrompt\(pane, request\.prompt, submission\)/);
  assert.match(chatBackend, /appelle `create_chat`/);
});

test("un agent cree expose tous ses reglages dans un editeur persistant", () => {
  for (const field of [
    "name",
    "objective",
    "role",
    "accountId",
    "projectDir",
    "mode",
    "intervalMinutes",
    "triggerKind",
    "watchPaths",
    "debounceSeconds",
    "allowGitPublish",
    "model",
    "reasoningEffort",
    "requireUserReview",
    "testCommand",
    "testTimeoutSeconds",
    "activate",
  ]) {
    assert.match(main, new RegExp(`data-autonomous-edit-field="${field}"`));
  }
  assert.match(main, /data-autonomous-edit-connector=/);
  assert.match(main, /connectors: provider === "codex" \? draft\.connectors : \[\]/);
  assert.match(main, /activate: automaticallyPaused \|\| draft\.activate/);
  assert.match(main, /"update_autonomous_agent"/);
  assert.match(backend, /pub account_id: Option<String>/);
  assert.match(backend, /let account_changed = agent\.account_id != account_id/);
  assert.match(backend, /discussion_to_delete = agent[\s\S]*?\.session_id[\s\S]*?\.take\(\)/);
  assert.match(server, /api_update_autonomous_agent/);
});

test("la reaffectation d'un agent autonome protege le changement de compte", () => {
  assert.match(backend, /pub struct ReassignAutonomousAgentAccountRequest/);
  assert.match(backend, /pub fn reassign_account/);
  assert.match(backend, /account_has_auth_tokens\(target\)/);
  assert.match(backend, /agent\.current_start_id\.is_some\(\)/);
  assert.match(backend, /self\.inner\.chat\.stop\(turn_id\)/);
  assert.match(backend, /current\.account_id = target_account_id\.clone\(\)/);
  assert.match(backend, /remove_autonomous_discussion\(account_id, session_id\)/);
  assert.match(main, /invoke<AutonomousAgentSnapshot>\(\s*"reassign_autonomous_agent_account"/);
});

test("le lancement direct en orchestration differe tout cycle autonome", () => {
  assert.match(backend, /pub defer_first_run: bool/);
  assert.match(backend, /status: if defer_first_run \{\s*AutonomousAgentStatus::Paused/);
  assert.match(
    backend,
    /next_run_at: if defer_first_run[\s\S]*?\|\| trigger_kind == AutonomousTriggerKind::WorkspaceChange[\s\S]*?\{\s*None/,
  );
  assert.match(backend, /Agent préparé en pause pour son lancement en orchestration/);
  assert.match(main, /promote_autonomous_agent_to_orchestration/);
  assert.match(main, /workerAccountIds,/);
});

test("un quota autonome epuise bascule vers le meilleur autre compte", () => {
  assert.match(backend, /is_quota_exhaustion_message\(&error\)/);
  assert.match(backend, /settings::account_limit_views\(&app_settings\)/);
  assert.match(backend, /select_quota_failover_target/);
  assert.match(backend, /remaining_quota_percent/);
  assert.match(backend, /target\.source_account_id == agent\.account_id/);
  assert.match(backend, /agent\.account_id = target\.account_id\.clone\(\)/);
  assert.match(backend, /"quota_account_switched"/);
  assert.match(backend, /agent\.next_run_at = Some\(now\)/);
});

test("la promotion autonome est transactionnelle et conserve la session", () => {
  assert.match(backend, /prepare_orchestration_promotion/);
  assert.match(backend, /rollback_orchestration_promotion/);
  assert.match(backend, /finalize_orchestration_promotion/);
  assert.match(backend, /La discussion est conservée car elle appartient/);
  assert.match(orchestration, /create_internal\(create_request, true\)/);
  assert.match(orchestration, /autonomous\.finalize_orchestration_promotion\(id\)/);
  assert.match(orchestration, /self\.control\(&created\.id, OrchestrationAction::Resume\)/);
  assert.match(orchestration, /orchestrator_session_id: agent\.session_id\.clone\(\)/);
});

test("le smoke test web :8080 transmet les connecteurs Google au serveur", () => {
  const smoke = readFileSync(new URL("../scripts/smoke-site.mjs", import.meta.url), "utf8");
  assert.match(smoke, /data-autonomous-connector="gmail"/);
  assert.match(smoke, /data-autonomous-connector="google_calendar"/);
  assert.match(smoke, /autonomousPayload\.connectors/);
  assert.match(smoke, /\["gmail", "google_calendar"\]/);
  assert.match(smoke, /\/api\/autonomous-agents\/autonomous-smoke\/orchestration/);
  assert.match(smoke, /\/api\/autonomous-agents\/autonomous-smoke\/account/);
  assert.match(smoke, /autonomousOrchestrationForm/);
  assert.doesNotMatch(smoke, /data-autonomous-orchestration-worker/);
  assert.doesNotMatch(smoke, /data-autonomous-launch-worker/);
  assert.match(smoke, /JSON\.stringify\(\[accountId, accountId\]\)/);
  assert.match(smoke, /testerCount: Math\.max/);
  assert.match(smoke, /testers: \[\]/);
  assert.match(smoke, /if \(path === "\/api\/limits"\) \{[\s\S]*?hasTokens: true/);
  assert.match(smoke, /autonomousAccountMutation/);
  assert.match(smoke, /promotionMutation\.payload\.workerAccountIds/);
  assert.match(smoke, /workerAccountMutation\.payload\.role !== "worker"/);
  assert.match(smoke, /workerAccountMutation\.payload\.workerIndex !== 1/);
  assert.match(smoke, /directCreateMutation\.payload\.deferFirstRun !== true/);
  assert.match(smoke, /autonomous-direct-orchestrator-launch/);
});

test("le moteur persiste, reprend et se suspend apres les echecs", () => {
  assert.match(backend, /AutonomousAgentStore/);
  assert.match(backend, /pub enum AutonomousTriggerKind/);
  assert.match(backend, /WorkspaceChange/);
  assert.match(backend, /scan_workspace_events/);
  assert.match(backend, /publication_start_blocked/);
  assert.match(backend, /project_has_other_in_flight_work/);
  assert.match(backend, /workspace_fingerprint/);
  assert.match(backend, /apply_workspace_fingerprint/);
  assert.match(backend, /put_workspace_agent_to_sleep/);
  assert.match(backend, /AUTORISATION EXPLICITE GIT ET PUBLICATION/);
  assert.match(backend, /fs_util::atomic_write/);
  assert.match(backend, /normalize_loaded_store/);
  assert.match(backend, /Processus interrompu detecte/);
  assert.match(backend, /MAX_CONSECUTIVE_FAILURES: u32 = 3/);
  assert.match(backend, /MODEL_CAPACITY_RETRY_MAX_DELAY_SECONDS: u64 = 60/);
  assert.match(backend, /is_model_capacity_message/);
  assert.match(backend, /"model_capacity_retry"/);
  assert.match(backend, /"model_capacity_recovered"/);
  assert.match(backend, /if model_capacity_error \{/);
  assert.doesNotMatch(backend, /model_capacity_error\s*&&/);
  assert.match(backend, /AutonomousAgentStatus::NeedsAttention/);
  assert.match(backend, /pending_review/);
  assert.match(backend, /require_user_review/);
  assert.match(backend, /approved_review/);
  assert.match(backend, /effective_turn_mode/);
  assert.match(backend, /ChatTurnMode::Plan/);
  assert.match(backend, /start_review_planning/);
  assert.match(backend, /GARDE-FOU REVIEW UTILISATEUR ACTIF/);
  assert.match(backend, /capture PNG\/JPEG ou une maquette fidele/);
  assert.match(backend, /emets AUTONOMOUS_REVIEW_EVIDENCE/);
  assert.match(backend, /pub struct AutonomousReviewEvidence/);
  assert.match(backend, /MAX_REVIEW_EVIDENCE_BYTES/);
  assert.match(backend, /require_visual_review_evidence/);
  assert.match(backend, /Autorisation visuelle impossible/);
  assert.match(main, /renderAutonomousReviewEvidence/);
  assert.match(main, /read_autonomous_review_evidence/);
  assert.match(main, /candidate\.naturalWidth > 0 && candidate\.naturalHeight > 0/);
  assert.match(main, /const approvalBlocked = \(visualEvidenceRequired && !visualEvidenceReady\)/);
  assert.match(style, /\.autonomous-monitor-review-evidence/);
  assert.match(backend, /compare-la explicitement/);
  assert.match(backend, /les deux chemins de preuve/);
  assert.match(chatBackend, /ReviewProofArtifacts/);
  assert.match(chatBackend, /sandbox_mode=.*workspace-write/);
  assert.match(chatBackend, /approval_policy=.*never/);
  assert.match(chatBackend, /sandbox_workspace_write\.network_access=false/);
  assert.match(chatBackend, /AUTONOMOUS_REVIEW_EVIDENCE/);
  assert.match(gitignore, /^\.codex-proof\/$/m);
  assert.match(backend, /AUTONOMOUS_REVIEW:/);
  assert.match(backend, /AUTONOMOUS_REVIEW_EXTERNAL:/);
  assert.match(backend, /ChatAppConnector::Gmail/);
  assert.match(backend, /app_connectors: Some\(agent\.connectors\.clone\(\)\)/);
  assert.match(backend, /Les suppressions restent interdites/);
  assert.match(backend, /AutonomousAgentAction::ApproveReview/);
  assert.match(backend, /AutonomousAgentAction::AuthorizePayment/);
  assert.match(backend, /AutonomousAgentAction::ConfirmPayment/);
  assert.match(backend, /AutonomousAgentAction::RejectReview/);
  assert.match(backend, /pub struct AutonomousPaymentRequest/);
  assert.match(backend, /pub payment_id: Option<String>/);
  assert.match(backend, /AUTONOMOUS_PAYMENT: reference-stable/);
  assert.match(backend, /validate_payment_checkout_url/);
  assert.match(backend, /approved_review_allows_connector_write/);
  assert.match(backend, /let durable_directive = if pending_review\.payment\.is_some\(\)/);
  assert.doesNotMatch(backend, /pending_review\.payment\.is_some\(\) && !message_waiting/);
  assert.match(main, /data-autonomous-payment-open/);
  assert.match(main, /data-autonomous-payment-agent/);
  assert.match(main, /data-autonomous-payment-id/);
  assert.match(main, /controlAutonomousAgentFromMonitor\(agentId, "authorizePayment"\)/);
  assert.doesNotMatch(main, /data-autonomous-payment-confirmation|data-autonomous-payment-submit/);
  assert.match(main, /paymentId,/);
  assert.match(main, /<span>Payer<\/span>/);
  assert.match(main, /Google Pay s'il est proposé/);
  assert.match(backend, /PAIEMENTS AVEC HANDOFF MOBILE/);
  assert.match(backend, /PAYMENT_RECEIPT_CHECK_DELAY_SECONDS/);
  assert.match(backend, /AutonomousPaymentStatus::Authorized/);
  assert.match(backend, /CHECKOUT AUTORISE ET OUVERT/);
  assert.match(backend, /mobile_push::enqueue_payment_handoff/);
  assert.match(mobilePushBackend, /"fid": device\.firebase_installation_id/);
  assert.match(mobilePushBackend, /"type": "payment_handoff"/);
  assert.match(mobilePushBackend, /pub struct ConfigureMobilePushRequest/);
  assert.match(mobilePushBackend, /parse_google_services_json/);
  assert.match(mobilePushBackend, /parse_service_account_json/);
  assert.match(mobilePushBackend, /protect_private_file/);
  assert.match(mobilePushBackend, /test_mobile_push_configuration/);
  assert.match(mobilePushBackend, /"type": "configuration_test"/);
  assert.doesNotMatch(mobilePushBackend, /checkoutUrl|checkout_url/);
  const mobileDeviceView = mobilePushBackend.match(
    /pub struct MobilePushDeviceView \{([\s\S]*?)\n\}/,
  )?.[1] ?? "";
  assert.doesNotMatch(mobileDeviceView, /firebase_installation_id/);
  const mobileConfigurationView = mobilePushBackend.match(
    /pub struct MobilePushConfigurationView \{([\s\S]*?)\n\}/,
  )?.[1] ?? "";
  assert.doesNotMatch(mobileConfigurationView, /private_key|service_account_json/);
  assert.match(server, /\/notifications\/mobile-push\/devices/);
  assert.match(server, /\/notifications\/mobile-push\/config/);
  assert.match(server, /\/notifications\/mobile-push\/test/);
  assert.match(server, /api_register_mobile_push_device[\s\S]*?check_maintenance_header/);
  assert.match(platform, /installMobilePaymentHandoffListener/);
  assert.match(platform, /openExternalHttpsUrl/);
  assert.match(platform, /\{ action: args\.action, paymentId: args\.paymentId \}/);
  assert.match(platform, /tauriOpenUrl\(url\.toString\(\)\)/);
  assert.match(server, /request\.payment_id\.as_deref\(\)/);
  assert.match(desktop, /tauri_plugin_opener::init\(\)/);
  assert.match(desktopCapability, /"identifier": "opener:allow-open-url"/);
  assert.match(desktopCapability, /"url": "https:\/\/\*"/);
  assert.match(style, /\.autonomous-monitor-payment/);
  assert.match(backend, /AUTONOMOUS_STATUS: continue/);
  assert.match(backend, /AUTONOMOUS_REPORT: resultat essentiel du tour/);
  assert.match(backend, /lecteur humain non technique/);
  assert.match(backend, /N'y affiche jamais d'identifiant interne/);
  assert.match(backend, /AUTONOMOUS_PROPOSAL: titre court \| mission autonome precise/);
  assert.match(backend, /MAX_PUBLIC_REPORT_CHARS: usize = 600/);
  assert.match(backend, /create_goal/);
  assert.match(backend, /AUTONOMOUS_MEMORY:/);
  assert.match(backend, /AUTONOMOUS_MEMORY_STRATEGY:/);
  assert.match(backend, /AUTONOMOUS_TASK:/);
  assert.match(backend, /AUTONOMOUS_NEXT_TASK:/);
  assert.match(backend, /pub struct AutonomousAgentReport/);
  assert.match(backend, /pub struct AutonomousAgentProposal/);
  assert.match(backend, /fn proposals_from_snapshot/);
  assert.match(backend, /push_proposal\(agent, title, objective, now\)/);
  assert.match(backend, /fn normalize_loaded_proposals/);
  assert.match(backend, /proposals\.is_empty\(\) && agent_is_project_radar\(agent\)/);
  assert.match(backend, /pub source_proposal_id: Option<String>/);
  assert.match(backend, /pub source_report_id: Option<String>/);
  assert.match(backend, /pub source_report_idea_index: Option<u32>/);
  assert.match(backend, /Cette proposition a deja ete executee/);
  assert.match(backend, /Cette idee de compte rendu a deja ete implementee/);
  assert.match(backend, /doit conserver le compte et le projet de la proposition/);
  assert.match(backend, /const MAX_REPORTS: usize = 24/);
  assert.match(backend, /push_report\(agent, content, now\)/);
  assert.match(backend, /fn normalize_loaded_reports/);
  assert.match(backend, /public_parts\.join\("\\n"\)/);
  assert.match(backend, /reconcile_completion_with_work_plan/);
  assert.match(backend, /run_validation_command/);
  assert.match(backend, /MAX_CONSECUTIVE_TEST_FAILURES: u32 = 3/);
  assert.match(backend, /test_completion_pending/);
  assert.match(backend, /pub fn schedule\(/);
  assert.match(backend, /"rescheduled"/);
  assert.match(backend, /MAX_SCHEDULE_AHEAD_SECONDS/);
  assert.match(backend, /interval_seconds: Option<u64>/);
  assert.match(backend, /agent\.interval_seconds = interval_seconds/);
  assert.match(backend, /remove_autonomous_discussion/);
  assert.match(backend, /session_id: None,[\s\S]*memoire et du carnet persistants/);
  assert.match(backend, /CST_AUTONOMOUS_AGENT_SESSION: true/);
});

test("un superviseur systeme est cree et execute un controle horaire", () => {
  assert.match(backend, /SYSTEM_SUPERVISOR_ID: &str = "cst-autonomous-supervisor"/);
  assert.match(backend, /SYSTEM_SUPERVISOR_INTERVAL_SECONDS: u64 = 60 \* 60/);
  assert.match(backend, /fn reconcile_system_supervisor\(/);
  assert.match(backend, /agent_keeps_supervisor_enabled/);
  assert.match(backend, /supervisor_schedule_repaired/);
  assert.match(backend, /render_system_supervisor_context/);
  assert.match(backend, /render_system_supervisor_context_with_live/);
  assert.match(backend, /supervisor_general_report_batch_ids/);
  assert.match(backend, /COMPTES RENDUS NON LUS A COMPILER/);
  assert.match(backend, /AUTONOMOUS_REPORT_SOURCES:/);
  assert.match(backend, /fn general_report_source_ids_from_snapshot/);
  assert.match(backend, /general_report_published/);
  assert.match(backend, /report\.read_at = Some\(now\)/);
  assert.match(backend, /MAX_GENERAL_REPORT_CHARS: usize = 4_000/);
  assert.match(backend, /AUTONOMOUS_SUPERVISION:/);
  assert.match(backend, /apply_supervisor_guidance_to_store/);
  assert.match(backend, /AutonomousMemoryKind::Supervisor/);
  assert.match(backend, /SYSTEM_SUPERVISOR_REDIRECT_MIN_RUNTIME_SECONDS/);
  assert.match(backend, /STARTUP_RECOVERY_STAGGER_SECONDS: i64 = 10/);
  assert.match(backend, /MAX_CONCURRENT_AGENT_RUNS_PER_PROJECT: usize = 2/);
  assert.match(backend, /fn project_capacity_start_blocked\(/);
  assert.match(backend, /fn agent_start_blocked\(/);
  assert.match(backend, /stagger_due_agents_after_restart/);
  assert.match(backend, /startup_recovery_staggered/);
  assert.match(backend, /autonomous_prompt_with_context/);
  assert.match(backend, /Ne modifie jamais le fichier d'etat persistant/);
  assert.match(backend, /ensure_user_managed_agent/);
  assert.match(main, /agent\.systemManaged/);
  assert.match(main, /autonomousCollapsedAgentCards\.get\(agent\.id\) \?\? systemManaged/);
  assert.match(main, /cardCollapsed \? "Afficher" : "Masquer"/);
  assert.match(main, /Géré par le système · toutes les heures/);
  assert.match(main, /Cycle de supervision protégé/);
  assert.match(style, /\.autonomous-system-managed/);
  assert.match(style, /\.autonomous-memory-kind\.kind-supervisor/);
});

test("les cartes autonomes et leurs sous-sections sont masquables en noir et blanc", () => {
  assert.match(style, /\.autonomous-agent-collapse-button/);
  assert.match(style, /\.autonomous-agent-card\.is-collapsed/);
  assert.match(style, /\.autonomous-agent-subsection/);
  assert.match(style, /\.autonomous-agent-subsection-hide/);
  assert.match(style, /--autonomous-accent: #f5f5f5/);
  assert.match(theme, /Agents autonomes : variante claire strictement monochrome/);
  assert.match(theme, /--autonomous-accent: #171719/);
});

test("la consommation de tokens est persistée et détaillée dans chaque agent", () => {
  const accountUsage = readFileSync(
    new URL("../src-tauri/src/account_usage.rs", import.meta.url),
    "utf8",
  );
  assert.match(backend, /pub struct AutonomousTokenUsage/);
  assert.match(backend, /pub token_usage: AutonomousTokenUsage/);
  assert.match(backend, /agent\.token_usage\.add_session\(usage\)/);
  assert.match(accountUsage, /fn token_totals_for_account_session/);
  assert.match(main, /Tokens cumulés/);
  assert.match(main, /autonomous-agent-token-breakdown/);
  assert.match(main, /Tokens des agents/);
});
