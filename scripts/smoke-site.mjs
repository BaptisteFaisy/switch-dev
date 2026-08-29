import { existsSync, mkdirSync } from "node:fs";
import { chromium } from "playwright-core";

const site = process.env.CST_SMOKE_URL || "http://127.0.0.1:18080";
const trace = (step) => {
  if (process.env.CST_SMOKE_TRACE) process.stderr.write(`[smoke] ${step}\n`);
};
const chromeCandidates = [
  process.env.CST_CHROME_PATH,
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
  `${process.env.LOCALAPPDATA || ""}/Google/Chrome/Application/chrome.exe`,
].filter(Boolean);
const executablePath = chromeCandidates.find(existsSync);

if (!executablePath) {
  throw new Error("Chrome ou Chromium est introuvable.");
}

const workspace = "C:\\Smoke\\Projet";
const accountId = "smoke-account";
const automaticOrchestrationPrompt = "Construire une fonctionnalité complexe avec plusieurs chantiers parallèles";
const automaticOrchestrationVerdict = 'CST_AUTO_ORCHESTRATION: {"decision":"orchestrate","workerCount":3,"reason":"Trois chantiers indépendants à intégrer"}';
const settings = {
  accounts: [{
    id: accountId,
    label: "Compte smoke test",
    provider: "codex",
    codexHome: "C:\\Smoke\\.codex",
    projectDir: workspace,
    bypass: false,
    model: "gpt-5-codex",
    reasoningEffort: "high",
  }, {
    id: "smoke-account-2",
    label: "Compte reprise smoke",
    provider: "codex",
    codexHome: "C:\\Smoke\\.codex-2",
    projectDir: workspace,
    bypass: false,
    model: "gpt-5-codex",
    reasoningEffort: "high",
  }],
  proxies: [],
  defaultAccountId: accountId,
  shell: "powershell",
  codexCommand: "codex",
  autoRunCodex: false,
  proxyControlsEnabled: false,
  pool: {
    port: 8787,
    apiKey: "",
    defaultModel: "gpt-5-codex",
    reasoningEffort: "high",
    upstream: "",
    requestTimeoutSecs: 120,
    cooldownSecs429: 60,
    concurrency: 1,
    clientIdOverride: "",
  },
  agents: [{
    id: "codex",
    label: "Codex",
    command: "codex",
    provider: "codex",
    kind: "cli",
    builtin: true,
  }],
  activeAgentId: "codex",
  kombai: {
    codeServerCommand: "code-server",
    port: 3000,
    extensionId: "",
    autoInstallExtension: false,
  },
  codexBypass: false,
  autoDiscoverAccounts: false,
  workspaces: [{ id: "smoke/projet", label: "Projet smoke", path: workspace, memory: "" }],
  closedWorkspaceIds: [],
};

const discussions = {
  generatedAt: Date.now(),
  totalDiscussions: 0,
  accounts: [{
    accountId,
    label: "Compte smoke test",
    provider: "codex",
    codexHome: "C:\\Smoke\\.codex",
    hasTokens: true,
    discussionCount: 0,
    discussions: [],
  }],
};

const today = new Date().toISOString().slice(0, 10);
const emptyUsageDay = {
  date: today,
  agentRunSeconds: 0,
  agentRuns: 0,
  apiRequests: 0,
  apiErrors: 0,
  apiSeconds: 0,
  inputTokens: 0,
  cachedInputTokens: 0,
  outputTokens: 0,
  totalTokens: 0,
  estimatedRequests: 0,
  costUsd: 0,
};

let autonomousAgentsMock = [];
let orchestrationsMock = [];
const mutationRequests = [];

const autonomousSnapshotFixture = (request) => {
  const timestamp = Math.floor(Date.now() / 1000);
  return {
    id: "autonomous-smoke",
    name: request.name || "Agent smoke",
    objective: request.objective,
    role: request.role,
    accountId: request.accountId,
    projectDir: request.projectDir,
    sessionId: null,
    mode: request.mode,
    model: request.model,
    reasoningEffort: request.reasoningEffort,
    connectors: request.connectors || [],
    intervalSeconds: request.intervalSeconds,
    status: request.deferFirstRun ? "paused" : "active",
    createdAt: timestamp,
    updatedAt: timestamp,
    nextRunAt: request.deferFirstRun ? null : timestamp + request.intervalSeconds,
    lastRunStartedAt: null,
    lastRunFinishedAt: null,
    currentTurnId: null,
    currentStartId: null,
    attemptCount: 0,
    runCount: 0,
    consecutiveFailures: 0,
    lastError: null,
    lastSummary: null,
    requireUserReview: request.requireUserReview,
    pendingReview: null,
    approvedReview: null,
    memory: request.initialMemory
      ? [{ id: "memory-smoke", kind: "user", content: request.initialMemory, createdAt: timestamp }]
      : [],
    testCommand: request.testCommand,
    testTimeoutSeconds: request.testTimeoutSeconds,
    testStatus: request.testCommand ? "idle" : "not_configured",
    currentTestId: null,
    testCompletionPending: false,
    consecutiveTestFailures: 0,
    lastTestStartedAt: null,
    lastTestFinishedAt: null,
    lastTestExitCode: null,
    lastTestDurationMs: null,
    lastTestOutput: null,
    events: [{ timestamp, kind: "created", message: "Agent smoke créé" }],
  };
};

const orchestrationSnapshotFixture = (request) => {
  const timestamp = Math.floor(Date.now() / 1000);
  return {
    id: "orchestration-smoke",
    name: request.name || "Orchestration smoke",
    objective: request.objective,
    workerCount: request.workerCount,
    testerCount: Math.max(1, Math.ceil(request.workerCount / 5)),
    accountId: request.accountId,
    orchestratorAccountId: request.orchestratorAccountId || request.accountId,
    workerAccountIds: request.workerAccountIds || Array.from({ length: request.workerCount }, () => request.accountId),
    orchestratorHandoffPending: false,
    orchestratorHandoffCount: 0,
    projectDir: request.projectDir,
    model: request.model,
    reasoningEffort: request.reasoningEffort,
    testCommand: request.testCommand,
    testTimeoutSeconds: request.testTimeoutSeconds,
    status: "active",
    phase: "planning",
    createdAt: timestamp,
    updatedAt: timestamp,
    baseCommit: "0000000000000000000000000000000000000000",
    integratedCommit: "0000000000000000000000000000000000000000",
    sandboxRoot: "C:\\Smoke\\sandboxes\\orchestration-smoke",
    orchestratorDir: "C:\\Smoke\\sandboxes\\orchestration-smoke\\orchestrator",
    orchestratorSessionId: request.orchestratorSessionId || null,
    currentTurnId: null,
    currentTurnKind: null,
    currentTaskId: null,
    currentTesterId: null,
    currentStartId: null,
    currentValidationId: null,
    currentValidationKind: null,
    nextActionAt: timestamp + 5,
    planSummary: null,
    tasks: [],
    testers: [],
    finalSummary: null,
    lastError: null,
    consecutiveStartFailures: 0,
    protocolFailures: 0,
    publishApplied: false,
    events: [{ timestamp, kind: "created", message: "Orchestration smoke créée" }],
  };
};

const jsonFor = (path) => {
  if (path === "/api/settings") return settings;
  if (path === "/api/discussions") return discussions;
  if (path === "/api/forum/topics") return [];
  if (
    path === "/api/private-messages/users"
    || path === "/api/private-messages/conversations"
  ) return [];
  if (path === "/api/fs/list") {
    return {
      root: "C:\\Smoke",
      path: workspace,
      parent: "C:\\Smoke",
      entries: [{ name: "Sous-dossier", path: `${workspace}\\Sous-dossier`, isDir: true }],
    };
  }
  if (path === "/api/limits") {
    return settings.accounts.map((account, index) => ({
      id: account.id,
      label: account.label,
      provider: account.provider,
      codexHome: account.codexHome,
      hasTokens: true,
      sessionUsedPercent: index === 0 ? 0 : 95,
      weeklyUsedPercent: index === 0 ? 0 : 95,
      buckets: [],
      source: "smoke",
      error: null,
    }));
  }
  if (path === "/api/chat/models") return [];
  if (path === "/api/video/capabilities") {
    return {
      configured: false,
      service: "smoke",
      configurationHint: "Configuration vidéo simulée",
      maxImageBytes: 10 * 1024 * 1024,
      models: [],
    };
  }
  if (path === "/api/usage") {
    return {
      generatedAt: Date.now(),
      totalAgentSeconds: 0,
      totalAgentRuns: 0,
      activeAgentCount: 0,
      totalApiRequests: 0,
      totalApiErrors: 0,
      totalTokens: 0,
      totalCostUsd: 0,
      today: emptyUsageDay,
      days: [emptyUsageDay],
    };
  }
  if (path === "/api/autonomous-agents") return autonomousAgentsMock;
  if (path === "/api/orchestrations") return orchestrationsMock;
  if (path === "/api/workspaces/access") return [];
  if (path.includes("/events") || path.includes("/history")) return [];
  if (path.includes("account") && path.includes("usage")) {
    return { generatedAt: Date.now(), profileCount: 1, totalTokens: 0, totalCostUsd: 0, totalSessions: 0, accounts: [] };
  }
  if (path.includes("metrics") || path.includes("stats")) {
    return { generatedAt: Date.now(), totals: {}, daily: [], accounts: [] };
  }
  if (path.includes("pool") && path.includes("status")) return { running: false, accounts: [] };
  if (path.includes("kombai") && path.includes("status")) {
    return {
      running: false,
      started: false,
      port: 3000,
      command: "code-server",
      binaryAvailable: false,
      extensionId: "kombai.kombai",
    };
  }
  if (path.includes("skills")) return [];
  if (path.includes("doctolib")) return { state: "demo", proposals: [] };
  if (path.includes("voice") || path.includes("gpu")) {
    return {
      mode: "remote",
      state: "ready",
      stage: "idle",
      transcriptionModel: "Systran/faster-whisper-small",
      summaryModel: "llama",
      transcriptionTarget: "remote",
      summaryTarget: "unknown",
      transcriptionAccelerator: "gpu",
      transcriptionReady: true,
      whisperReady: true,
      ollamaReachable: false,
      summaryModelLoaded: false,
      summaryModelOnGpu: false,
    };
  }
  return {};
};

const browser = await chromium.launch({ executablePath, headless: true });
const failures = [];
const accessibilityResponsiveChecks = [];
const keyboardFocusChecks = [];
let ignoredWebSocketFailures = 0;
let ignoredRequestAborts = 0;
let expectedApiFailures = 0;
let currentView = "startup";
const contextTasksOnly = process.env.CST_SMOKE_CONTEXT_TASKS_ONLY === "1";
const progress = (stage) => process.stderr.write(`[smoke] ${stage}\n`);

try {
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    locale: "fr-FR",
    serviceWorkers: "block",
  });
  await context.addInitScript(({ workspacePath, contextTasksEnabled }) => {
    localStorage.setItem("codex-switch-terminal.remote.token", "smoke-token");
    localStorage.setItem("codex-switch-terminal.workspace.path", workspacePath);
    localStorage.setItem("codex-switch-terminal.workspaces.v1", JSON.stringify([workspacePath]));
    if (contextTasksEnabled) {
      localStorage.removeItem("codex-switch-terminal.chat-context-tasks-visible.v1");
      localStorage.setItem("codex-switch-terminal.tasks.v1", JSON.stringify([
        { id: "smoke-active-high", title: "Corriger le menu droit", completed: false, createdAt: 2, completedAt: null, priority: "high", dueDate: null, environmentPath: null },
        { id: "smoke-active-normal", title: "Vérifier les limites", completed: false, createdAt: 1, completedAt: null, priority: "normal", dueDate: null, environmentPath: null },
        { id: "smoke-completed", title: "Tâche déjà terminée", completed: true, createdAt: 0, completedAt: 1, priority: "normal", dueDate: null, environmentPath: null },
      ]));
    }
  }, { workspacePath: workspace, contextTasksEnabled: contextTasksOnly });

  const page = await context.newPage();
  const capturedChatTurns = [];
  let chatTurn = null;
  page.setDefaultTimeout(10_000);
  page.on("pageerror", (error) => failures.push(`pageerror[${currentView}]: ${error.stack || error}`));
  page.on("console", (message) => {
    if (message.type() !== "error") {
      if (process.env.CST_SMOKE_TRACE) trace(`console-${message.type()}-${message.text()}`);
      return;
    }
    if (
      message.text().includes("WebSocket connection to")
      && (message.text().includes("/ws/discussions") || message.text().includes("/ws/runtime"))
    ) {
      ignoredWebSocketFailures += 1;
    } else {
      const location = message.location();
      const source = location.url
        ? ` (${location.url}:${location.lineNumber ?? 0}:${location.columnNumber ?? 0})`
        : "";
      failures.push(`console[${currentView}]: ${message.text()}${source}`);
    }
  });
  page.on("requestfailed", (request) => {
    const errorText = request.failure()?.errorText || "inconnue";
    if (request.method() === "GET" && errorText.includes("net::ERR_ABORTED")) {
      ignoredRequestAborts += 1;
      return;
    }
    failures.push(`requestfailed: ${request.method()} ${request.url()} (${errorText})`);
  });
  page.on("request", (request) => {
    if (process.env.CST_SMOKE_TRACE && request.method() !== "GET") {
      trace(`request-${request.method().toLowerCase()}-${new URL(request.url()).pathname}`);
    }
  });

  const auditAccessibilityAndOverflow = async (label) => {
    const diagnostics = await page.evaluate(() => {
      const visible = (element) => {
        const style = getComputedStyle(element);
        const rect = element.getBoundingClientRect();
        return style.display !== "none"
          && style.visibility !== "hidden"
          && Number(style.opacity) > 0
          && rect.width > 0
          && rect.height > 0;
      };
      const accessibleName = (element) => {
        const labelledBy = element.getAttribute("aria-labelledby");
        if (labelledBy) {
          const text = labelledBy
            .split(/\s+/)
            .map((id) => document.getElementById(id)?.textContent?.trim() || "")
            .filter(Boolean)
            .join(" ");
          if (text) return text;
        }
        const ariaLabel = element.getAttribute("aria-label")?.trim();
        if (ariaLabel) return ariaLabel;
        if (element instanceof HTMLInputElement && element.labels?.length) {
          const text = [...element.labels].map((label) => label.textContent?.trim() || "").join(" ").trim();
          if (text) return text;
        }
        if ((element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement)
          && element.placeholder?.trim()) return element.placeholder.trim();
        if (element instanceof HTMLImageElement && element.alt.trim()) return element.alt.trim();
        return element.textContent?.trim() || element.getAttribute("title")?.trim() || "";
      };
      const parseColor = (value) => {
        const srgb = value.match(
          /^color\(srgb\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+))?\)$/i,
        );
        if (srgb) {
          return [
            Number(srgb[1]) * 255,
            Number(srgb[2]) * 255,
            Number(srgb[3]) * 255,
            srgb[4] === undefined ? 1 : Number(srgb[4]),
          ];
        }
        const parts = value.match(/[\d.]+/g)?.map(Number) || [];
        return parts.length >= 3 ? [parts[0], parts[1], parts[2], parts[3] ?? 1] : null;
      };
      const composite = (foreground, background) => {
        const alpha = foreground[3] + background[3] * (1 - foreground[3]);
        if (!alpha) return [0, 0, 0, 0];
        return [
          (foreground[0] * foreground[3] + background[0] * background[3] * (1 - foreground[3])) / alpha,
          (foreground[1] * foreground[3] + background[1] * background[3] * (1 - foreground[3])) / alpha,
          (foreground[2] * foreground[3] + background[2] * background[3] * (1 - foreground[3])) / alpha,
          alpha,
        ];
      };
      const backgroundColor = (element) => {
        const ancestors = [];
        for (let current = element; current; current = current.parentElement) ancestors.unshift(current);
        let color = [255, 255, 255, 1];
        for (const ancestor of ancestors) {
          const style = getComputedStyle(ancestor);
          if (style.backgroundImage !== "none") return null;
          const layer = parseColor(style.backgroundColor);
          if (layer) color = composite(layer, color);
        }
        return color;
      };
      const luminance = (color) => {
        const channels = color.slice(0, 3).map((channel) => {
          const value = channel / 255;
          return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
        });
        return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
      };
      const contrast = (first, second) => {
        const light = Math.max(luminance(first), luminance(second));
        const dark = Math.min(luminance(first), luminance(second));
        return (light + 0.05) / (dark + 0.05);
      };
      const selector = [
        "button", "a[href]", "input:not([type='hidden'])", "select", "textarea",
        "[role='button']", "[role='link']", "[role='menuitem']", "[role='tab']",
      ].join(",");
      const unnamed = [...document.querySelectorAll(selector)]
        .filter((element) => visible(element) && !element.closest("[inert]") && !accessibleName(element))
        .map((element) => ({
          tag: element.tagName.toLowerCase(),
          id: element.id,
          className: typeof element.className === "string" ? element.className : "",
        }));
      const htmlOverflow = Math.max(
        document.documentElement.scrollWidth,
        document.body.scrollWidth,
      ) - window.innerWidth;
      const clippedControls = [...document.querySelectorAll(selector)]
        .filter((element) => visible(element) && !element.closest("[inert]"))
        .filter((element) => {
          const rect = element.getBoundingClientRect();
          if (rect.right <= window.innerWidth + 1 && rect.left >= -1) return false;
          let parent = element.parentElement;
          while (parent) {
            const style = getComputedStyle(parent);
            if ((style.overflowX === "auto" || style.overflowX === "scroll")
              && parent.scrollWidth > parent.clientWidth) return false;
            parent = parent.parentElement;
          }
          return true;
        })
        .map((element) => accessibleName(element).slice(0, 80) || element.id || element.tagName);
      const lowContrast = [...document.querySelectorAll("p, span, strong, small, label, button, a, h1, h2, h3, h4, td, th, summary")]
        .filter((element) => visible(element) && !element.closest("[inert]")
          && [...element.childNodes].some((node) => node.nodeType === Node.TEXT_NODE && node.textContent?.trim()))
        .flatMap((element) => {
          const style = getComputedStyle(element);
          const foreground = parseColor(style.color);
          const background = backgroundColor(element);
          if (!foreground || !background || Number(style.opacity) < 1) return [];
          const renderedForeground = composite(foreground, background);
          const ratio = contrast(renderedForeground, background);
          const size = Number.parseFloat(style.fontSize);
          const weight = Number.parseInt(style.fontWeight, 10) || 400;
          const minimum = size >= 24 || (size >= 18.66 && weight >= 700) ? 3 : 4.5;
          return ratio + 0.05 < minimum ? [{
            text: element.textContent.trim().replace(/\s+/g, " ").slice(0, 80),
            tag: element.tagName.toLowerCase(),
            id: element.id,
            className: typeof element.className === "string" ? element.className : "",
            color: style.color,
            background: background.map((channel) => Math.round(channel * 100) / 100),
            ratio: Number(ratio.toFixed(2)),
            minimum,
          }] : [];
        });
      return { unnamed, htmlOverflow, clippedControls, lowContrast };
    });
    if (diagnostics.unnamed.length) {
      failures.push(`accessibility-${label}: controles sans nom ${JSON.stringify(diagnostics.unnamed)}`);
    }
    if (diagnostics.htmlOverflow > 1) {
      failures.push(`responsive-${label}: debordement horizontal de ${diagnostics.htmlOverflow}px`);
    }
    if (diagnostics.clippedControls.length) {
      failures.push(`responsive-${label}: controles hors viewport ${JSON.stringify(diagnostics.clippedControls)}`);
    }
    if (diagnostics.lowContrast.length) {
      failures.push(`contrast-${label}: textes sous le seuil ${JSON.stringify(diagnostics.lowContrast)}`);
    }
    accessibilityResponsiveChecks.push({ label, ...diagnostics });
  };

  const auditKeyboardFocus = async (steps) => {
    await page.evaluate(() => {
      if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
      window.scrollTo(0, 0);
    });
    for (let index = 0; index < steps; index += 1) {
      await page.keyboard.press("Tab");
      const diagnostic = await page.evaluate(() => {
        const element = document.activeElement;
        if (!(element instanceof HTMLElement) || element === document.body) return null;
        const focusTargets = [element, element.parentElement, element.parentElement?.parentElement].filter(Boolean);
        const snapshot = () => focusTargets.map((target) => {
          const style = getComputedStyle(target);
          return {
            backgroundColor: style.backgroundColor,
            borderColor: style.borderColor,
            boxShadow: style.boxShadow,
            color: style.color,
            outlineStyle: style.outlineStyle,
            outlineWidth: style.outlineWidth,
          };
        });
        const focused = snapshot();
        element.blur();
        const idle = snapshot();
        element.focus();
        const rect = element.getBoundingClientRect();
        const changed = focused.some((focusedStyle, targetIndex) =>
          ["backgroundColor", "borderColor", "boxShadow", "color"]
            .some((property) => focusedStyle[property] !== idle[targetIndex][property]));
        const outlined = focused.some((style) =>
          style.outlineStyle !== "none" && Number.parseFloat(style.outlineWidth) > 0);
        return {
          name: element.getAttribute("aria-label") || element.textContent?.trim().replace(/\s+/g, " ").slice(0, 60) || element.id || element.tagName,
          inViewport: rect.top >= -1 && rect.left >= -1 && rect.bottom <= window.innerHeight + 1 && rect.right <= window.innerWidth + 1,
          indicator: outlined || changed,
        };
      });
      if (!diagnostic) {
        break;
      }
      if (!diagnostic.inViewport) failures.push(`keyboard-${diagnostic.name}: focus hors viewport`);
      if (!diagnostic.indicator) failures.push(`keyboard-${diagnostic.name}: focus sans indicateur visible`);
      keyboardFocusChecks.push(diagnostic);
    }
  };
  // Le smoke s'execute aussi contre `vite preview`, qui ne possede pas la
  // sonde du serveur Rust. Sans ce mock, le poll de mise a jour web produit un
  // 404 console sans rapport avec l'interface testee.
  await page.route("**/healthz", (route) => route.fulfill({
    json: { ok: true, ready: true, version: "0.1.0", commit: "local" },
  }));
  await page.route("**/api/**", (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === "/api/autonomous-agents") {
      if (request.method() === "POST") {
        const payload = request.postDataJSON();
        mutationRequests.push({ path, payload });
        const snapshot = autonomousSnapshotFixture(payload);
        autonomousAgentsMock = [snapshot];
        return route.fulfill({ json: snapshot });
      }
      return route.fulfill({ json: autonomousAgentsMock });
    }
    if (path === "/api/orchestrations") {
      if (request.method() === "POST") {
        const payload = request.postDataJSON();
        mutationRequests.push({ path, payload });
        const snapshot = orchestrationSnapshotFixture(payload);
        orchestrationsMock = [snapshot];
        return route.fulfill({ json: snapshot });
      }
      return route.fulfill({ json: orchestrationsMock });
    }
    const autonomousControl = /^\/api\/autonomous-agents\/([^/]+)\/control$/.exec(path);
    if (autonomousControl && request.method() === "POST") {
      const payload = request.postDataJSON();
      mutationRequests.push({ path, payload });
      const current = autonomousAgentsMock.find((agent) => agent.id === autonomousControl[1]);
      const updated = {
        ...current,
        status: payload.action === "pause" ? "paused" : "active",
        updatedAt: Math.floor(Date.now() / 1000),
      };
      autonomousAgentsMock = [updated];
      return route.fulfill({ json: updated });
    }
    const autonomousSchedule = /^\/api\/autonomous-agents\/([^/]+)\/schedule$/.exec(path);
    if (autonomousSchedule && request.method() === "POST") {
      const payload = request.postDataJSON();
      mutationRequests.push({ path, payload });
      const current = autonomousAgentsMock.find((agent) => agent.id === autonomousSchedule[1]);
      const updated = {
        ...current,
        nextRunAt: payload.nextRunAt,
        intervalSeconds: payload.intervalSeconds,
        updatedAt: Math.floor(Date.now() / 1000),
      };
      autonomousAgentsMock = [updated];
      return route.fulfill({ json: updated });
    }
    const autonomousAccount = /^\/api\/autonomous-agents\/([^/]+)\/account$/.exec(path);
    if (autonomousAccount && request.method() === "POST") {
      const payload = request.postDataJSON();
      mutationRequests.push({ path, payload });
      const current = autonomousAgentsMock.find((agent) => agent.id === autonomousAccount[1]);
      const target = settings.accounts.find((account) => account.id === payload.accountId);
      const updated = {
        ...current,
        accountId: payload.accountId,
        model: target?.model ?? current.model,
        reasoningEffort: target?.reasoningEffort ?? current.reasoningEffort,
        sessionId: null,
        updatedAt: Math.floor(Date.now() / 1000),
      };
      autonomousAgentsMock = [updated];
      return route.fulfill({ json: updated });
    }
    const autonomousPromotion = /^\/api\/autonomous-agents\/([^/]+)\/orchestration$/.exec(path);
    if (autonomousPromotion && request.method() === "POST") {
      const payload = request.postDataJSON();
      mutationRequests.push({ path, payload });
      const current = autonomousAgentsMock.find((agent) => agent.id === autonomousPromotion[1]);
      const snapshot = orchestrationSnapshotFixture({
        ...payload,
        accountId: current.accountId,
        orchestratorAccountId: current.accountId,
        orchestratorSessionId: current.sessionId,
      });
      autonomousAgentsMock = autonomousAgentsMock.filter((agent) => agent.id !== autonomousPromotion[1]);
      orchestrationsMock = [snapshot];
      return route.fulfill({ json: snapshot });
    }
    const orchestrationControl = /^\/api\/orchestrations\/([^/]+)\/control$/.exec(path);
    if (orchestrationControl && request.method() === "POST") {
      const payload = request.postDataJSON();
      mutationRequests.push({ path, payload });
      const current = orchestrationsMock.find((run) => run.id === orchestrationControl[1]);
      const updated = {
        ...current,
        status: payload.action === "pause" ? "paused" : "active",
        updatedAt: Math.floor(Date.now() / 1000),
      };
      orchestrationsMock = [updated];
      return route.fulfill({ json: updated });
    }
    const orchestrationAccount = /^\/api\/orchestrations\/([^/]+)\/account$/.exec(path);
    if (orchestrationAccount && request.method() === "POST") {
      const payload = request.postDataJSON();
      mutationRequests.push({ path, payload });
      const current = orchestrationsMock.find((run) => run.id === orchestrationAccount[1]);
      const updated = payload.role === "orchestrator"
        ? {
            ...current,
            orchestratorAccountId: payload.accountId,
            orchestratorHandoffCount: (current.orchestratorHandoffCount || 0) + 1,
          }
        : {
            ...current,
            workerAccountIds: current.workerAccountIds.map((id, index) =>
              index === payload.workerIndex - 1 ? payload.accountId : id),
          };
      orchestrationsMock = [updated];
      return route.fulfill({ json: updated });
    }
    const autonomousDelete = /^\/api\/autonomous-agents\/([^/]+)$/.exec(path);
    if (autonomousDelete && request.method() === "DELETE") {
      mutationRequests.push({ path, payload: null });
      autonomousAgentsMock = autonomousAgentsMock.filter((agent) => agent.id !== autonomousDelete[1]);
      return route.fulfill({ json: {} });
    }
    const orchestrationDelete = /^\/api\/orchestrations\/([^/]+)$/.exec(path);
    if (orchestrationDelete && request.method() === "DELETE") {
      mutationRequests.push({ path, payload: null });
      orchestrationsMock = orchestrationsMock.filter((run) => run.id !== orchestrationDelete[1]);
      return route.fulfill({ json: {} });
    }
    if (path === "/api/chat/turns/active") return route.fulfill({ json: [] });
    if (path === "/api/chat/turns" && request.method() === "POST") {
      const payload = request.postDataJSON();
      capturedChatTurns.push(payload);
      const timestamp = Math.floor(Date.now() / 1000);
      const answer = payload.prompt === automaticOrchestrationPrompt
        ? automaticOrchestrationVerdict
        : "Réponse smoke reçue";
      chatTurn = {
        id: 42,
        accountId: payload.accountId,
        sessionId: "smoke-session",
        status: "completed",
        startedAt: timestamp,
        finishedAt: timestamp,
        error: null,
        activities: [],
        thoughts: [],
        parts: [{ id: "answer", kind: "text", status: "completed", text: answer }],
      };
      discussions.totalDiscussions = 1;
      discussions.accounts[0].discussionCount = 1;
      discussions.accounts[0].discussions = [{
        sessionId: "smoke-session",
        rolloutId: "smoke-session",
        forkCount: 1,
        provider: "codex",
        accountId,
        accountLabel: "Compte smoke test",
        codexHome: "C:\\Smoke\\.codex",
        filePath: "C:\\Smoke\\rollout-smoke-session.jsonl",
        folderPath: workspace,
        cwd: workspace,
        startedAt: timestamp,
        lastActivity: timestamp,
        title: "Message smoke",
        preview: payload.prompt,
        messageCount: 2,
        totalTokens: 10,
        cliVersion: "smoke",
      }];
      return route.fulfill({ json: chatTurn });
    }
    if (/^\/api\/chat\/turns\/[^/]+$/.test(path) && chatTurn) {
      return route.fulfill({ json: chatTurn });
    }
    if (path === "/api/discussions/transcript") {
      const timestamp = Math.floor(Date.now() / 1000);
      return route.fulfill({ json: {
        sessionId: "smoke-session",
        truncated: false,
        messages: [
          { role: "user", text: capturedChatTurns.at(-1)?.prompt || "Message smoke", timestamp },
          { role: "assistant", text: "Réponse smoke reçue", timestamp },
        ],
      } });
    }
    return route.fulfill({ json: jsonFor(path) });
  });

  // L'application maintient volontairement des polls 24/7 (agents, limites,
  // tours actifs). `networkidle` ne peut donc plus servir de signal de boot.
  const response = await page.goto(`${site}/?smoke=1`, { waitUntil: "domcontentloaded" });
  if (!response?.ok()) failures.push(`navigation: HTTP ${response?.status() ?? "sans réponse"}`);
  await page.locator("#chatAppSidebar").waitFor({ state: "visible" });
  trace("startup-ready");
  const navigationTarget = (process.env.CST_SMOKE_NAVIGATION_TARGET || "").trim();
  const navigationOnly = process.env.CST_SMOKE_NAVIGATION_ONLY === "1" || !!navigationTarget;
  const automaticOrchestrationOnly = process.env.CST_SMOKE_AUTO_ORCHESTRATION_ONLY === "1";
  const staleChunkRecoveryOnly = process.env.CST_SMOKE_STALE_CHUNK_RECOVERY_ONLY === "1";
  const rightMenuOnly = process.env.CST_SMOKE_RIGHT_MENU_ONLY === "1";

  const verifyAutonomousMonitorDrag = async () => {
    await page.waitForFunction(() => {
      const launcher = document.querySelector("#autonomousMonitorLauncher");
      return launcher instanceof HTMLButtonElement
        && !launcher.getAttribute("aria-label")?.includes("Chargement des agents");
    });
    await page.locator("#autonomousMonitorLauncher").click();
    const monitor = page.locator("#autonomousMonitorWindow");
    await monitor.waitFor({ state: "visible" });
    const dragHandle = page.locator("[data-autonomous-monitor-drag-handle]");
    await dragHandle.waitFor({ state: "visible" });
    if (await page.locator("[data-autonomous-open-chat], [data-autonomous-monitor-chat]").count()) {
      failures.push("autonomous-success: un agent autonome expose encore un lien vers Discussions");
    }
    const geometry = await page.waitForFunction(() => {
      const monitorElement = document.querySelector("#autonomousMonitorWindow");
      const titleElement = document.querySelector(".autonomous-monitor-head > div");
      if (!(monitorElement instanceof HTMLElement) || !(titleElement instanceof HTMLElement)) return null;
      const monitorBounds = monitorElement.getBoundingClientRect();
      const titleBounds = titleElement.getBoundingClientRect();
      if (!monitorBounds.width || !monitorBounds.height || !titleBounds.width || !titleBounds.height) return null;
      return {
        before: { x: monitorBounds.x, y: monitorBounds.y },
        dragHandleBounds: {
          x: titleBounds.x,
          y: titleBounds.y,
          width: titleBounds.width,
          height: titleBounds.height,
        },
      };
    }).then((handle) => handle.jsonValue());
    const before = geometry?.before;
    const dragHandleBounds = geometry?.dragHandleBounds;
    if (!before || !dragHandleBounds) {
      failures.push("autonomous-monitor-drag: fenetre ou poignee introuvable");
    } else {
      const pointerStart = {
        x: dragHandleBounds.x + Math.min(250, dragHandleBounds.width * .55),
        y: dragHandleBounds.y + dragHandleBounds.height / 2,
      };
      const expected = { x: 80, y: 8 };
      await page.mouse.move(pointerStart.x, pointerStart.y);
      await page.mouse.down();
      await page.mouse.move(
        pointerStart.x + expected.x - before.x,
        pointerStart.y + expected.y - before.y,
        { steps: 8 },
      );
      await page.mouse.up();
      await page.waitForFunction(() => {
        const stored = JSON.parse(
          localStorage.getItem("codex-switch-terminal.autonomous-monitor-position.v1") || "null",
        );
        return stored && Math.abs(stored.x - 80) <= 2 && Math.abs(stored.y - 8) <= 2;
      });
      const after = await monitor.boundingBox();
      const stored = await page.evaluate(() => JSON.parse(
        localStorage.getItem("codex-switch-terminal.autonomous-monitor-position.v1") || "null",
      ));
      if (
        !after
        || Math.abs(after.x - expected.x) > 2
        || Math.abs(after.y - expected.y) > 2
        || Math.abs(stored?.x - expected.x) > 2
        || Math.abs(stored?.y - expected.y) > 2
      ) {
        failures.push(
          `autonomous-monitor-drag: position inattendue ${JSON.stringify({ after, stored })}`,
        );
      }
    }
    await monitor.locator("[data-autonomous-monitor-close]").click();
    await page.locator("#autonomousMonitorLauncher").click();
    await monitor.waitFor({ state: "visible" });
    const reopened = await monitor.boundingBox();
    if (!reopened || Math.abs(reopened.x - 80) > 2 || Math.abs(reopened.y - 8) > 2) {
      failures.push(`autonomous-monitor-drag: position non restauree ${JSON.stringify(reopened)}`);
    }
    await monitor.locator("[data-autonomous-monitor-close]").click();
  };

  if (process.env.CST_SMOKE_AUTONOMOUS_MONITOR_ONLY === "1") {
    await verifyAutonomousMonitorDrag();
    const relevantFailures = failures.filter(
      (failure) => !failure.startsWith("requestfailed:") || !failure.includes("net::ERR_ABORTED"),
    );
    process.stdout.write(`${JSON.stringify({
      site,
      check: "autonomous-monitor-drag-position",
      ignoredRequestAborts: failures.length - relevantFailures.length,
      failures: relevantFailures,
    }, null, 2)}\n`);
    await context.close();
    await browser.close();
    process.exit(relevantFailures.length ? 1 : 0);
  }

  const activateSidebarDestination = async (targetPage, selector) => {
    const button = targetPage.locator(selector);
    if (await button.count() !== 1) return false;
    if (!(await button.isVisible())) {
      const more = targetPage.locator("#chatSideMoreToggle");
      if (await more.count() !== 1 || !(await more.isVisible())) return false;
      if ((await more.getAttribute("aria-expanded")) !== "true") await more.click();
      await button.waitFor({ state: "visible" });
    }
    // Plusieurs vues sont chargees en import dynamique et peuvent provoquer un
    // rerendu entre la resolution du locator et l'action pointeur de Playwright.
    // Le clic DOM est atomique une fois la visibilite verifiee ci-dessus.
    await button.evaluate((element) => element.click());
    return true;
  };

  if (!automaticOrchestrationOnly && !navigationTarget && !staleChunkRecoveryOnly && !contextTasksOnly) {
  currentView = "desktop-more-keyboard";
  const expectedRestingNavigationCount = await page.locator(
    ".chat-side-tools > button, .chat-side-tools > .chat-side-more > #chatSideMoreToggle",
  ).count();
  const restingNavigationCount = await page.locator(".chat-side-tools button:visible").count();
  if (restingNavigationCount !== expectedRestingNavigationCount) {
    failures.push(`navigation desktop: ${restingNavigationCount} controles visibles au repos au lieu de ${expectedRestingNavigationCount}`);
  }
  const desktopMore = page.locator("#chatSideMoreToggle");
  const desktopMoreMenu = page.locator("#chatSideMoreMenu");
  const openDesktopMoreMenu = async (mode) => {
    // Le polling peut remplacer la barre entre sa creation et `bindUi()`. Un
    // geste tombe alors sur l'ancien bouton, sans listener, meme si le produit
    // est correctement cable. Reprendre sur le noeud courant borne ce faux
    // negatif sans masquer une absence persistante de liaison.
    for (let attempt = 0; attempt < 5; attempt += 1) {
      if (await desktopMoreMenu.isVisible()) return;
      const trigger = page.locator("#chatSideMoreToggle");
      if (mode === "keyboard") {
        await trigger.evaluate((button) => {
          button.focus();
          button.dispatchEvent(new KeyboardEvent("keydown", {
            key: "ArrowDown",
            bubbles: true,
            cancelable: true,
          }));
        });
      } else {
        await trigger.evaluate((button) => button.click());
      }
      try {
        await desktopMoreMenu.waitFor({ state: "visible", timeout: 500 });
        return;
      } catch {
        // Le prochain essai vise le bouton du dernier rendu.
      }
    }
    await desktopMoreMenu.waitFor({ state: "visible" });
  };
  const closeDesktopMoreMenu = async (mode) => {
    for (let attempt = 0; attempt < 5; attempt += 1) {
      if (!(await desktopMoreMenu.isVisible())) return;
      const target = mode === "menu"
        ? page.locator("#chatSideMoreMenu [role='menuitem']").first()
        : page.locator("#chatSideMoreToggle");
      await target.evaluate((element) => {
        element.focus();
        element.dispatchEvent(new KeyboardEvent("keydown", {
          key: "Escape",
          bubbles: true,
          cancelable: true,
        }));
      });
      try {
        await desktopMoreMenu.waitFor({ state: "hidden", timeout: 500 });
        return;
      } catch {
        // Le prochain essai vise les controles du dernier rendu.
      }
    }
    await desktopMoreMenu.waitFor({ state: "hidden" });
  };
  await openDesktopMoreMenu("keyboard");
  if ((await desktopMore.getAttribute("aria-expanded")) !== "true") {
    failures.push("navigation desktop: Plus n'annonce pas son ouverture");
  }
  const focusedMoreItem = await page.evaluate(() => document.activeElement?.id || "");
  if (focusedMoreItem !== "bugReportToggle") {
    failures.push(`navigation desktop: le premier item de Plus ne recoit pas le focus (${focusedMoreItem || "aucun"})`);
  }
  await auditAccessibilityAndOverflow("desktop-more");
  await closeDesktopMoreMenu("menu");
  if ((await desktopMore.getAttribute("aria-expanded")) !== "false" || !(await desktopMore.evaluate((button) => button === document.activeElement))) {
    failures.push("navigation desktop: Echap ne ferme pas Plus avec retour du focus");
  }
  for (const viewport of [
    { name: "compact", width: 1024, height: 600 },
    { name: "low", width: 861, height: 390 },
  ]) {
    await page.setViewportSize({ width: viewport.width, height: viewport.height });
    await page.waitForTimeout(80);
    if (await page.locator(".chat-side-tools button:visible").count() !== expectedRestingNavigationCount) {
      failures.push(`navigation desktop ${viewport.name}: le repos n'affiche pas ${expectedRestingNavigationCount} controles`);
    }
    await openDesktopMoreMenu("click");
    await page.evaluate(() => {
      document.querySelector("#chatSideMoreMenu [role='menuitem']:last-child")
        ?.scrollIntoView({ block: "nearest" });
    });
    const menuLayout = await page.evaluate(() => {
      const menu = document.querySelector("#chatSideMoreMenu");
      const last = menu?.querySelector("[role='menuitem']:last-child");
      if (!(menu instanceof HTMLElement) || !(last instanceof HTMLElement)) return null;
      const menuRect = menu.getBoundingClientRect();
      const lastRect = last.getBoundingClientRect();
      return {
        insideViewport: menuRect.top >= 0 && menuRect.right <= document.documentElement.clientWidth && menuRect.bottom <= document.documentElement.clientHeight && menuRect.left >= 0,
        lastReachable: lastRect.top >= menuRect.top - 1 && lastRect.bottom <= menuRect.bottom + 1,
        menuRect: { top: menuRect.top, right: menuRect.right, bottom: menuRect.bottom, left: menuRect.left },
        overflowY: getComputedStyle(menu).overflowY,
      };
    });
    if (!menuLayout?.insideViewport || !menuLayout.lastReachable || menuLayout.overflowY !== "auto") {
      failures.push(`navigation desktop ${viewport.name}: menu Plus hors limites ${JSON.stringify(menuLayout)}`);
    }
    await closeDesktopMoreMenu("trigger");
  }
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.waitForTimeout(80);
  currentView = "startup";
  }

  if (rightMenuOnly) {
    process.stdout.write(`${JSON.stringify({
      site,
      check: "right-menu-navigation",
      accessibilityResponsiveChecks,
      failures,
    }, null, 2)}\n`);
    await context.close();
    await browser.close();
    process.exit(failures.length ? 1 : 0);
  }

  if (contextTasksOnly) {
    currentView = "context-tasks";
    const panel = page.locator("#chatContextTasks");
    await panel.waitFor({ state: "visible" });
    const initialPreference = await page.evaluate(() => localStorage.getItem("codex-switch-terminal.chat-context-tasks-visible.v1"));
    if (initialPreference !== null) {
      failures.push(`context-tasks: la préférence initiale devrait être absente (${initialPreference})`);
    }
    await page.locator("#settingsToggle").click();
    await page.locator('[data-chat-context-tasks="hide"]').waitFor({ state: "visible" });
    await page.locator('[data-chat-context-tasks="hide"]').click();
    await panel.waitFor({ state: "detached" });
    await page.waitForFunction(() => localStorage.getItem("codex-switch-terminal.chat-context-tasks-visible.v1") === "false");
    await page.locator('[data-chat-context-tasks="show"]').waitFor({ state: "visible" });
    await page.locator('[data-chat-context-tasks="show"]').click();
    await page.waitForFunction(() => localStorage.getItem("codex-switch-terminal.chat-context-tasks-visible.v1") === "true");
    await panel.waitFor({ state: "visible" });
    const screenshotDir = process.env.CST_SMOKE_SCREENSHOT_DIR?.trim();
    await page.locator("#chatContextSidebarCollapse").click();
    await page.waitForFunction(() =>
      document.querySelector(".chat-app-layout")?.classList.contains("is-context-sidebar-collapsed"),
    );
    const collapsedSidebar = await page.evaluate(() => {
      const layout = document.querySelector(".chat-app-layout");
      const sidebar = document.querySelector("#chatContextSidebar");
      const workspace = document.querySelector("#chatMainWorkspace");
      const expand = document.querySelector("#chatContextSidebarExpand");
      if (!(layout instanceof HTMLElement)
        || !(sidebar instanceof HTMLElement)
        || !(workspace instanceof HTMLElement)
        || !(expand instanceof HTMLElement)) return null;
      const workspaceRect = workspace.getBoundingClientRect();
      return {
        storedWidth: localStorage.getItem("codex-switch-terminal.chat-context-sidebar-width.v1"),
        sidebarVisibility: getComputedStyle(sidebar).visibility,
        expandDisplay: getComputedStyle(expand).display,
        workspaceRight: Math.round(workspaceRect.right),
        viewportWidth: document.documentElement.clientWidth,
      };
    });
    if (
      collapsedSidebar?.storedWidth !== "0"
      || collapsedSidebar.sidebarVisibility !== "hidden"
      || !["flex", "inline-flex"].includes(collapsedSidebar.expandDisplay)
      || collapsedSidebar.workspaceRight !== collapsedSidebar.viewportWidth
    ) {
      failures.push(`context-sidebar: repli incomplet ${JSON.stringify(collapsedSidebar)}`);
    }
    if (screenshotDir) {
      mkdirSync(screenshotDir, { recursive: true });
      await page.screenshot({ path: `${screenshotDir}/context-sidebar-collapsed-1440.png` });
    }
    await page.locator("#chatContextSidebarExpand").click();
    await page.waitForFunction(() =>
      !document.querySelector(".chat-app-layout")?.classList.contains("is-context-sidebar-collapsed"),
    );
    const restoredSidebar = await page.evaluate(() => {
      const sidebar = document.querySelector("#chatContextSidebar");
      if (!(sidebar instanceof HTMLElement)) return null;
      return {
        storedWidth: Number(localStorage.getItem("codex-switch-terminal.chat-context-sidebar-width.v1")),
        visibility: getComputedStyle(sidebar).visibility,
        width: Math.round(sidebar.getBoundingClientRect().width),
      };
    });
    if (
      !restoredSidebar
      || restoredSidebar.storedWidth <= 0
      || restoredSidebar.visibility !== "visible"
      || restoredSidebar.width <= 0
    ) {
      failures.push(`context-sidebar: restauration impossible ${JSON.stringify(restoredSidebar)}`);
    }
    if (screenshotDir) {
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.waitForTimeout(180);
      await auditAccessibilityAndOverflow("context-tasks-1440");
      await page.screenshot({ path: `${screenshotDir}/context-tasks-1440.png` });
      await page.setViewportSize({ width: 1024, height: 700 });
      await page.waitForTimeout(180);
      await auditAccessibilityAndOverflow("context-tasks-1024");
      await page.screenshot({ path: `${screenshotDir}/context-tasks-1024.png` });
      await page.setViewportSize({ width: 1440, height: 900 });
    }
    const panelText = await panel.innerText();
    if (!panelText.includes("Corriger le menu droit") || !panelText.includes("Vérifier les limites")) {
      failures.push(`context-tasks: tâches actives absentes (${panelText})`);
    }
    if (panelText.includes("Tâche déjà terminée")) {
      failures.push("context-tasks: une tâche terminée est affichée");
    }
    await page.locator('[data-context-task-toggle="smoke-active-high"]').evaluate((input) => input.click());
    await page.waitForFunction(() => {
      const tasks = JSON.parse(localStorage.getItem("codex-switch-terminal.tasks.v1") || "[]");
      return tasks.find((task) => task.id === "smoke-active-high")?.completed === true;
    });
    if (await page.locator('[data-context-task-toggle="smoke-active-high"]').count()) {
      failures.push("context-tasks: la tâche terminée reste dans le panneau");
    }
    await page.locator("#chatContextTasksOpenAll").click();
    await page.locator("#tasksPanel").waitFor({ state: "visible" });
    process.stdout.write(`${JSON.stringify({
      site,
      check: "context-tasks-setting",
      defaultActive: true,
      activeTasksVisible: 2,
      completionPersisted: true,
      fullTasksViewOpened: true,
      failures,
    }, null, 2)}\n`);
    await context.close();
    await browser.close();
    process.exit(failures.length ? 1 : 0);
  }

  if (staleChunkRecoveryOnly) {
    currentView = "stale-chunk-recovery";
    let rejectedPromptChunkCount = 0;
    await page.route(/\/assets\/prompts-view-[^/]+\.js(?:\?.*)?$/, async (route) => {
      if (rejectedPromptChunkCount === 0) {
        rejectedPromptChunkCount += 1;
        await route.fulfill({
          status: 404,
          contentType: "text/javascript",
          body: "",
        });
        return;
      }
      await route.continue();
    });
    const expectedFailureStart = failures.length;
    await page.locator("#promptsToggle").click();
    let recovered = false;
    try {
      await page.waitForURL((url) => url.searchParams.has("cst-chunk-build"));
      await page.locator("#promptLibraryPanel").waitFor({ state: "visible" });
      recovered = true;
    } catch (error) {
      failures.push(`stale-chunk: récupération absente (${String(error)}) · URL ${page.url()}`);
    }
    const expectedResourceFailures = failures.splice(expectedFailureStart).filter(
      (failure) => !failure.includes("Failed to load resource")
        && !failure.includes("prompts-view-")
        && !failure.includes("net::ERR_ABORTED"),
    );
    failures.push(...expectedResourceFailures);
    if (rejectedPromptChunkCount !== 1) {
      failures.push(`stale-chunk: ${rejectedPromptChunkCount} ancien(s) chunk(s) rejeté(s) au lieu de 1`);
    }
    const recoveryUrl = new URL(page.url());
    if (recoveryUrl.searchParams.has("cst-chunk-view")) {
      failures.push("stale-chunk: la destination de récupération reste dans l'URL");
    }
    process.stdout.write(`${JSON.stringify({
      site,
      check: "stale-chunk-recovery",
      rejectedPromptChunkCount,
      recovered,
      recoveredView: await page.locator(".chat-admin-head strong").textContent({ timeout: 500 }).catch(() => null),
      accessibilityResponsiveChecks,
      failures,
    }, null, 2)}\n`);
    await context.close();
    await browser.close();
    process.exit(failures.length ? 1 : 0);
  }

  const views = [
    ["#autonomousToggle", "autonomous"],
    ["#orchestrationToggle", "orchestration"],
    ["#sideDiscussions", "discussions"],
    ["#dashboardToggle", "dashboard"],
    ["#limitsToggle", "limits"],
    ["#promptsToggle", "prompts"],
    ["#transcriptionToggle", "transcription"],
    ["#skillsToggle", "skills"],
    ["#settingsToggle", "settings"],
    ["#designToggle", "design"],
    ["#auditToggle", "audit"],
  ];
  const navigationViews = automaticOrchestrationOnly
    ? []
    : navigationTarget
      ? views.filter(([, name]) => name === navigationTarget)
      : navigationOnly
      ? [
        ["#tasksToggle", "tasks"],
        ["#scheduledChatToggle", "scheduled-chat"],
        ["#promptsToggle", "prompts"],
        ["#transcriptionToggle", "transcription"],
        ["#autonomousToggle", "autonomous"],
        ["#designToggle", "design"],
        ["#videoToggle", "video"],
        ["#bugReportToggle", "bug-report"],
        ["#orchestrationToggle", "orchestration"],
        ["#forumToggle", "forum"],
        ["#sideDiscussions", "discussions"],
        ["#dashboardToggle", "dashboard"],
        ["#limitsToggle", "limits"],
        ["#skillsToggle", "skills"],
        ["#settingsToggle", "settings"],
        ]
      : views;
  if (navigationTarget && navigationViews.length === 0) {
    failures.push(`navigation: destination inconnue ${navigationTarget}`);
  }
  const visited = [];
  const mobileVisited = [];
  progress("navigation desktop");

  for (const [selector, name] of navigationViews) {
    currentView = name;
    if (!(await activateSidebarDestination(page, selector))) {
      failures.push(`${name}: bouton ${selector} absent`);
      continue;
    }
    await page.waitForTimeout(150);
    const panel = page.locator(".chat-admin-panel");
    if (await panel.count() !== 1 || !(await panel.isVisible())) {
      failures.push(`${name}: panneau principal absent ou masqué`);
      continue;
    }
    const text = (await panel.innerText()).trim();
    if (!text) failures.push(`${name}: panneau vide`);
    visited.push({ name, characters: text.length });
    if (!navigationOnly) await auditAccessibilityAndOverflow(`desktop-${name}`);
    trace(`view-${name}`);
  }

  if (navigationOnly) {
    const screenshotDir = process.env.CST_SMOKE_SCREENSHOT_DIR?.trim();
    if (screenshotDir && navigationTarget) {
      mkdirSync(screenshotDir, { recursive: true });
      await page.screenshot({ path: `${screenshotDir}/${navigationTarget}-1440.png` });
      await page.setViewportSize({ width: 390, height: 844 });
      await page.waitForTimeout(200);
      await page.screenshot({ path: `${screenshotDir}/${navigationTarget}-390.png` });
    }
    const duplicateIds = await page.evaluate(() => {
      const counts = new Map();
      for (const element of document.querySelectorAll("[id]")) {
        counts.set(element.id, (counts.get(element.id) || 0) + 1);
      }
      return [...counts].filter(([, count]) => count > 1);
    });
    if (duplicateIds.length) failures.push(`DOM: identifiants dupliqués ${JSON.stringify(duplicateIds)}`);
    process.stdout.write(`${JSON.stringify({ site, visited, accessibilityResponsiveChecks, duplicateIds, failures }, null, 2)}\n`);
    await context.close();
    await browser.close();
    process.exit(failures.length ? 1 : 0);
  }

  const interactionChecks = [];
  const successfulMutationChecks = [];
  if (!automaticOrchestrationOnly) {
  await page.locator("#chatHome").click();
  await auditKeyboardFocus(30);

  const openAndCloseDialog = async ({ name, trigger, dialog, close }) => {
    currentView = `interaction-${name}`;
    await page.waitForFunction((selector) => {
      const button = document.querySelector(selector);
      if (!(button instanceof HTMLButtonElement) || button.disabled || !button.getClientRects().length) return false;
      button.focus();
      button.click();
      return true;
    }, trigger);
    const modal = page.locator(dialog);
    await modal.waitFor({ state: "visible" });
    if ((await modal.getAttribute("aria-modal")) !== "true") {
      failures.push(`${name}: la modale n'est pas annoncée comme modale`);
    }
    await page.locator(close).click();
    if (await modal.count()) failures.push(`${name}: la modale ne se ferme pas`);
    if (!(await page.locator(trigger).evaluate((element) => element === document.activeElement))) {
      failures.push(`${name}: le focus ne revient pas au déclencheur`);
    }
    interactionChecks.push(name);
    trace(`dialog-${name}`);
  };

  await page.locator("#chatHome").click();
  progress("modales desktop");
  await openAndCloseDialog({
    name: "new-chat",
    trigger: "#addExpertChat",
    dialog: "#newChatBackdrop [role='dialog']",
    close: "#closeNewChatModal",
  });
  await openAndCloseDialog({
    name: "environment-menu",
    trigger: "#wsOpenFolder",
    dialog: "#terminalEnvironmentMenuBackdrop [role='dialog']",
    close: "#closeTerminalEnvironmentMenu",
  });
  currentView = "interaction-workspace-browser";
  await page.locator("#wsOpenFolder").click();
  await page.locator("#createEnvironmentFromMenu").click();
  const workspaceDialog = page.locator("#workspaceBackdrop [role='dialog']");
  await workspaceDialog.waitFor({ state: "visible" });
  if ((await page.locator(".ws-folder-entry").count()) !== 1) {
    failures.push("workspace-browser: les dossiers de l'API ne sont pas rendus");
  }
  await page.locator("#closeWorkspaceModal").click();
  if (await workspaceDialog.count()) failures.push("workspace-browser: la modale ne se ferme pas");
  interactionChecks.push("workspace-browser");
  trace("dialog-workspace-browser");
  await page.locator("#settingsToggle").click();
  await openAndCloseDialog({
    name: "agents-manager",
    trigger: "#settingsAgents",
    dialog: "#agentsBackdrop [role='dialog']",
    close: "#closeAgentsModal",
  });

  currentView = "interaction-autonomous-validation";
  progress("validation autonome");
  await page.locator("#autonomousToggle").click();
  await page.waitForFunction(() => {
    const button = document.querySelector("#autonomousNewAgent");
    if (!(button instanceof HTMLButtonElement)) return false;
    button.click();
    return true;
  });
  await page.locator("#autonomousCreateForm").waitFor({ state: "visible" });
  if (!(await page.locator("#autonomousName").isHidden())) {
    failures.push("autonomous-simple: les options avancées sont visibles au premier affichage");
  }
  await page.locator("#autonomousCreateOptions > summary").click();
  await page.locator("#autonomousEnvironmentPreset").selectOption("__custom__");
  const customProject = page.locator("#autonomousProjectDir");
  if (await customProject.isHidden()) failures.push("autonomous: le chemin personnalisé reste masqué");
  await page.locator("#autonomousObjective").fill("Objectif de validation smoke");
  await page.locator("details.autonomous-advanced:has(#autonomousTestCommand) > summary").click();
  await page.locator("#autonomousTestCommand").fill("npm test");
  await page.locator("#autonomousCreateForm button[type='submit']").click();
  await page.waitForTimeout(50);
  const projectValidation = await page.locator("#autonomousProjectDir").evaluate(
    (input) => ({ invalid: !input.checkValidity(), message: input.validationMessage }),
  );
  if (!projectValidation.invalid || !projectValidation.message) {
    failures.push("autonomous: une commande de test sans environnement n'est pas bloquée");
  }
  interactionChecks.push("autonomous-validation");
  trace("autonomous-validation");

  currentView = "interaction-autonomous-success";
  await page.locator("#autonomousName").fill("Agent smoke réussi");
  await page.locator("#autonomousRole").fill("Vérificateur prudent");
  await page.locator("#autonomousProjectDir").fill(workspace);
  await page.locator("#autonomousInterval").selectOption("300");
  await page.locator("#autonomousMode").selectOption("plan");
  await page.locator("details.autonomous-advanced:has(#autonomousConnectorTitle) > summary").click();
  await page.locator("#autonomousRequireUserReview").check();
  await page.locator('[data-autonomous-connector="gmail"]').check();
  await page.locator('[data-autonomous-connector="google_calendar"]').check();
  await page.locator("#autonomousTestTimeout").fill("45");
  await page.locator("#autonomousCreateForm button[type='submit']").click();
  await page.locator(".autonomous-agent-list").getByText("Agent smoke réussi", { exact: true }).waitFor();
  const autonomousPayload = mutationRequests.findLast((entry) => entry.path === "/api/autonomous-agents")?.payload;
  if (
    !autonomousPayload
    || autonomousPayload.objective !== "Objectif de validation smoke"
    || autonomousPayload.projectDir !== workspace
    || autonomousPayload.intervalSeconds !== 300
    || autonomousPayload.mode !== "plan"
    || autonomousPayload.requireUserReview !== true
    || JSON.stringify(autonomousPayload.connectors) !== JSON.stringify(["gmail", "google_calendar"])
    || autonomousPayload.testCommand !== "npm test"
    || autonomousPayload.testTimeoutSeconds !== 45
  ) {
    failures.push(`autonomous-success: payload inattendu ${JSON.stringify(autonomousPayload)}`);
  }
  successfulMutationChecks.push("autonomous-create");
  const autonomousAccountSelect = page.locator('[data-autonomous-account="autonomous-smoke"]');
  if (!(await autonomousAccountSelect.isHidden())) {
    failures.push("autonomous-simple: les réglages détaillés sont visibles par défaut");
  }
  await page.locator('[data-autonomous-agent-more="autonomous-smoke"] > summary').click();
  await page.locator('[data-autonomous-agent-section="autonomous-smoke:configuration"] > summary').click();
  await autonomousAccountSelect.waitFor({ state: "visible" });
  await autonomousAccountSelect.selectOption("smoke-account-2");
  await page.waitForFunction(() => {
    const select = document.querySelector('[data-autonomous-account="autonomous-smoke"]');
    return select instanceof HTMLSelectElement && select.value === "smoke-account-2" && !select.disabled;
  });
  const autonomousAccountMutation = mutationRequests.findLast(
    (entry) => entry.path === "/api/autonomous-agents/autonomous-smoke/account",
  );
  if (!autonomousAccountMutation || autonomousAccountMutation.payload.accountId !== "smoke-account-2") {
    failures.push(`autonomous-account: payload inattendu ${JSON.stringify(autonomousAccountMutation?.payload)}`);
  }
  successfulMutationChecks.push("autonomous-account-reassign");
  await verifyAutonomousMonitorDrag();
  interactionChecks.push("autonomous-monitor-drag-position");
  interactionChecks.push("autonomous-isolated-from-discussions");
  trace("autonomous-success");

  currentView = "interaction-autonomous-schedule";
  await page.locator('[data-autonomous-schedule-open="autonomous-smoke"]').click();
  const scheduleInput = page.locator('[data-autonomous-schedule-input="autonomous-smoke"]');
  const frequencyInput = page.locator('[data-autonomous-frequency-input="autonomous-smoke"]');
  await scheduleInput.waitFor({ state: "visible" });
  await frequencyInput.selectOption("3600");
  const scheduleValue = await page.evaluate(() => {
    const date = new Date(Date.now() + 2 * 60 * 60 * 1000);
    const pad = (value) => String(value).padStart(2, "0");
    return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
  });
  await scheduleInput.fill(scheduleValue);
  await page.locator('[data-autonomous-schedule-save="autonomous-smoke"]').click();
  await page.locator('[data-autonomous-schedule-open="autonomous-smoke"]').waitFor();
  const scheduleMutation = mutationRequests.findLast((entry) => entry.path === "/api/autonomous-agents/autonomous-smoke/schedule");
  const expectedSchedule = Math.floor(new Date(scheduleValue).getTime() / 1000);
  if (
    !scheduleMutation
    || scheduleMutation.payload.nextRunAt !== expectedSchedule
    || scheduleMutation.payload.intervalSeconds !== 3_600
  ) {
    failures.push(`autonomous-schedule: payload inattendu ${JSON.stringify(scheduleMutation?.payload)}`);
  }
  successfulMutationChecks.push("autonomous-schedule");
  trace("autonomous-schedule");

  currentView = "interaction-autonomous-lifecycle";
  await page.locator('[data-autonomous-action="pause"][data-autonomous-id="autonomous-smoke"]').click();
  await page.locator('[data-autonomous-action="resume"][data-autonomous-id="autonomous-smoke"]').waitFor();
  await page.locator('[data-autonomous-action="resume"][data-autonomous-id="autonomous-smoke"]').click();
  await page.locator('[data-autonomous-action="pause"][data-autonomous-id="autonomous-smoke"]').waitFor();
  await page.locator('.autonomous-agent-card [data-autonomous-orchestrate="autonomous-smoke"]').click();
  const autonomousPromotionForm = page.locator("#autonomousOrchestrationForm");
  await autonomousPromotionForm.waitFor({ state: "visible" });
  if (!(await autonomousPromotionForm.getByText("Connecteurs non transférés", { exact: true }).isVisible())) {
    failures.push("autonomous-promotion: l'avertissement sur les connecteurs est absent");
  }
  const autonomousOrchestratorAccount = page.locator("#autonomousOrchestrationAccount");
  if ((await autonomousOrchestratorAccount.inputValue()) !== "smoke-account-2") {
    failures.push("autonomous-promotion: le compte courant n'est pas repris par l'orchestrateur");
  }
  await autonomousOrchestratorAccount.selectOption(accountId);
  await page.locator("#autonomousOrchestrationName").fill("Agent promu smoke");
  await page.locator("#autonomousOrchestrationObjective").fill("Orchestrer l'objectif autonome smoke");
  await page.locator("#autonomousOrchestrationProject").fill(workspace);
  await page.locator("#autonomousOrchestrationTestCommand").fill("npm run verify:quick");
  await page.locator("#autonomousOrchestrationWorkerCount").fill("2");
  await autonomousPromotionForm.locator("button[type='submit']").click();
  await page.locator(".orchestration-runs h3").getByText("Agent promu smoke", { exact: true }).waitFor();
  await page.locator(".autonomous-agent-list").getByText("Agent smoke réussi", { exact: true }).waitFor({ state: "detached" });
  const promotionMutation = mutationRequests.findLast(
    (entry) => entry.path === "/api/autonomous-agents/autonomous-smoke/orchestration",
  );
  if (
    !promotionMutation
    || promotionMutation.payload.objective !== "Orchestrer l'objectif autonome smoke"
    || promotionMutation.payload.projectDir !== workspace
    || promotionMutation.payload.testCommand !== "npm run verify:quick"
    || promotionMutation.payload.workerCount !== 2
    || JSON.stringify(promotionMutation.payload.workerAccountIds) !== JSON.stringify([accountId, accountId])
  ) {
    failures.push(`autonomous-promotion: payload inattendu ${JSON.stringify(promotionMutation?.payload)}`);
  }
  const promotionAccountMutation = mutationRequests.findLast(
    (entry) => entry.path === "/api/autonomous-agents/autonomous-smoke/account",
  );
  if (!promotionAccountMutation || promotionAccountMutation.payload.accountId !== accountId) {
    failures.push(`autonomous-promotion-account: payload inattendu ${JSON.stringify(promotionAccountMutation?.payload)}`);
  }
  successfulMutationChecks.push("autonomous-pause-resume-promotion-with-accounts");
  trace("autonomous-lifecycle");

  currentView = "interaction-orchestration-validation";
  progress("validation orchestration");
  if (!(await page.locator(".orchestration-panel").isVisible())) {
    await activateSidebarDestination(page, "#orchestrationToggle");
  }
  await page.locator("#orchestrationNew").click();
  await page.locator("#orchestrationCreateForm button[type='submit']").click();
  const objectiveValidation = await page.locator("#orchestrationObjective").evaluate(
    (input) => ({ invalid: !input.checkValidity(), message: input.validationMessage }),
  );
  if (!objectiveValidation.invalid || !objectiveValidation.message) {
    failures.push("orchestration: l'objectif vide n'est pas bloqué");
  }
  interactionChecks.push("orchestration-validation");
  trace("orchestration-validation");

  currentView = "interaction-orchestration-success";
  await page.locator("#orchestrationCreateAdvanced > summary").click();
  await page.locator("#orchestrationName").fill("Orchestration smoke réussie");
  await page.locator("#orchestrationObjective").fill("Construire la feature smoke");
  await page.locator("#orchestrationProjectDir").fill(workspace);
  await page.locator("#orchestrationTestCommand").fill("npm run verify:quick");
  await page.locator("#orchestrationTestTimeout").fill("90");
  await page.locator("#orchestrationCreateForm button[type='submit']").click();
  await page.locator(".orchestration-runs h3").getByText("Orchestration smoke réussie", { exact: true }).waitFor();
  const orchestrationPayload = mutationRequests.findLast((entry) => entry.path === "/api/orchestrations")?.payload;
  if (
    !orchestrationPayload
    || orchestrationPayload.objective !== "Construire la feature smoke"
    || orchestrationPayload.projectDir !== workspace
    || orchestrationPayload.testCommand !== "npm run verify:quick"
    || orchestrationPayload.testTimeoutSeconds !== 90
  ) {
    failures.push(`orchestration-success: payload inattendu ${JSON.stringify(orchestrationPayload)}`);
  }
  successfulMutationChecks.push("orchestration-create");
  trace("orchestration-success");

  currentView = "interaction-orchestration-account-handoff";
  await page.locator(".orchestration-run-details > summary").click();
  const orchestratorAccountSelect = page.locator(
    '[data-orchestration-account-role="orchestrator"][data-orchestration-run-id="orchestration-smoke"]',
  );
  await orchestratorAccountSelect.selectOption("smoke-account-2");
  await page.locator(
    '[data-orchestration-account-role="orchestrator"][data-orchestration-run-id="orchestration-smoke"]',
  ).waitFor({ state: "attached" });
  const accountMutation = mutationRequests.findLast(
    (entry) => entry.path === "/api/orchestrations/orchestration-smoke/account",
  );
  if (
    !accountMutation
    || accountMutation.payload.role !== "orchestrator"
    || accountMutation.payload.accountId !== "smoke-account-2"
  ) {
    failures.push(`orchestration-account: payload inattendu ${JSON.stringify(accountMutation?.payload)}`);
  }
  await page.locator(".orchestration-run-details > summary").click();
  const workerAccountSelect = page.locator(
    '[data-orchestration-account-role="worker"][data-orchestration-worker-index="1"][data-orchestration-run-id="orchestration-smoke"]',
  );
  await workerAccountSelect.selectOption("smoke-account-2");
  await page.waitForFunction(() => {
    const select = document.querySelector(
      '[data-orchestration-account-role="worker"][data-orchestration-worker-index="1"][data-orchestration-run-id="orchestration-smoke"]',
    );
    return select instanceof HTMLSelectElement && select.value === "smoke-account-2" && !select.disabled;
  });
  const workerAccountMutation = mutationRequests.findLast(
    (entry) => entry.path === "/api/orchestrations/orchestration-smoke/account",
  );
  if (
    !workerAccountMutation
    || workerAccountMutation.payload.role !== "worker"
    || workerAccountMutation.payload.workerIndex !== 1
    || workerAccountMutation.payload.accountId !== "smoke-account-2"
  ) {
    failures.push(`orchestration-worker-account: payload inattendu ${JSON.stringify(workerAccountMutation?.payload)}`);
  }
  successfulMutationChecks.push("orchestration-orchestrator-and-worker-account-handoffs");
  trace("orchestration-account-handoff");

  currentView = "interaction-orchestration-lifecycle";
  await page.locator('[data-orchestration-action="pause"][data-orchestration-id="orchestration-smoke"]').click();
  await page.locator('[data-orchestration-action="resume"][data-orchestration-id="orchestration-smoke"]').waitFor();
  await page.locator('[data-orchestration-action="resume"][data-orchestration-id="orchestration-smoke"]').click();
  await page.locator('[data-orchestration-action="pause"][data-orchestration-id="orchestration-smoke"]').waitFor();
  await page.locator(".orchestration-run-details > summary").click();
  await page.locator('[data-orchestration-delete="orchestration-smoke"]').click();
  await page.locator('[data-orchestration-delete-confirm="orchestration-smoke"]').click();
  await page.locator(".orchestration-runs").getByText("Orchestration smoke réussie", { exact: true }).waitFor({ state: "detached" });
  successfulMutationChecks.push("orchestration-pause-resume-delete");
  trace("orchestration-lifecycle");

  currentView = "interaction-autonomous-direct-orchestrator";
  await page.locator("#autonomousToggle").click();
  await page.locator("#autonomousNewAgent").click();
  await page.locator("#autonomousCreateOptions > summary").click();
  await page.locator("#autonomousName").fill("Orchestrateur direct smoke");
  await page.locator("#autonomousRole").fill("Orchestrateur de validation smoke");
  await page.locator("#autonomousObjective").fill("Lancer directement une équipe orchestrée smoke");
  await page.locator("#autonomousLaunchMode").selectOption("orchestrator");
  await page.locator("#autonomousLaunchWorkerCount").waitFor({ state: "visible" });
  await page.locator("#autonomousEnvironmentPreset").selectOption("__custom__");
  await page.locator("#autonomousProjectDir").fill(workspace);
  await page.locator("#autonomousLaunchWorkerCount").fill("2");
  const directAdvanced = page.locator("details.autonomous-advanced:has(#autonomousTestCommand)");
  if (!(await directAdvanced.evaluate((details) => details.open))) {
    await directAdvanced.locator("summary").click();
  }
  await page.locator("#autonomousTestCommand").fill("npm run verify:quick");
  await page.locator("#autonomousCreateForm button[type='submit']").click();
  await page.locator(".orchestration-runs h3").getByText("Orchestrateur direct smoke", { exact: true }).waitFor();
  const directCreateMutation = mutationRequests.findLast((entry) => entry.path === "/api/autonomous-agents");
  const directPromotionMutation = mutationRequests.findLast(
    (entry) => entry.path === "/api/autonomous-agents/autonomous-smoke/orchestration",
  );
  if (
    !directCreateMutation
    || directCreateMutation.payload.deferFirstRun !== true
    || directCreateMutation.payload.objective !== "Lancer directement une équipe orchestrée smoke"
  ) {
    failures.push(`autonomous-direct-orchestrator-create: payload inattendu ${JSON.stringify(directCreateMutation?.payload)}`);
  }
  if (
    !directPromotionMutation
    || directPromotionMutation.payload.workerCount !== 2
    || directPromotionMutation.payload.testCommand !== "npm run verify:quick"
    || JSON.stringify(directPromotionMutation.payload.workerAccountIds) !== JSON.stringify([accountId, accountId])
  ) {
    failures.push(`autonomous-direct-orchestrator-promotion: payload inattendu ${JSON.stringify(directPromotionMutation?.payload)}`);
  }
  if (autonomousAgentsMock.some((agent) => agent.name === "Orchestrateur direct smoke")) {
    failures.push("autonomous-direct-orchestrator: l'agent différé existe encore après sa promotion");
  }
  successfulMutationChecks.push("autonomous-direct-orchestrator-launch");
  trace("autonomous-direct-orchestrator");
  }

  currentView = "interaction-chat-success";
  await page.locator("#chatHome").click();
  await page.locator("#addExpertChat").click();
  const existingChatPaneKeys = await page.locator(".chat-panel--expert").evaluateAll(
    (panels) => panels.map((panel) => panel.getAttribute("data-chat-panel")).filter(Boolean),
  );
  await page.locator("#confirmNewChat").click();
  // La confirmation automatique choisit le quota de façon asynchrone. De plus,
  // supprimer une orchestration conserve son ancien orchestrateur comme chat
  // normal. Attendre une nouvelle identité évite de saisir dans cet ancien chat.
  const newChatPaneKey = await page.waitForFunction((knownKeys) => {
    const known = new Set(knownKeys);
    return [...document.querySelectorAll(".chat-panel--expert")]
      .map((panel) => panel.getAttribute("data-chat-panel"))
      .find((key) => key && !known.has(key)) || null;
  }, existingChatPaneKeys).then((handle) => handle.jsonValue());
  const chatPane = page.locator(`[data-chat-panel="${newChatPaneKey}"]`);
  await chatPane.waitFor({ state: "visible" });
  await page.locator("#newChatBackdrop").waitFor({ state: "detached" });
  // L'orchestration automatique est désormais le défaut global. Valider ce
  // défaut, puis repasser explicitement en mode direct pour couvrir d'abord le
  // parcours de chat classique avant de réactiver l'orchestrateur plus bas.
  const directModeToggle = chatPane.locator("[data-chat-action='toggle-automatic-orchestration']");
  await directModeToggle.waitFor({ state: "visible" });
  if ((await directModeToggle.getAttribute("aria-pressed")) !== "true") {
    failures.push("chat-automatic-orchestration: le mode orchestrateur n'est pas actif par défaut");
  }
  await directModeToggle.evaluate((button) => button.click());
  await page.waitForFunction((paneKey) =>
    document.querySelector(`.chat-panel--expert[data-chat-panel="${CSS.escape(paneKey)}"] [data-chat-action='toggle-automatic-orchestration']`)
      ?.getAttribute("aria-pressed") === "false", newChatPaneKey);
  await page.waitForFunction((paneKey) => {
    const panel = document.querySelector(`.chat-panel--expert[data-chat-panel="${CSS.escape(paneKey)}"]`);
    const send = panel?.querySelector("[data-chat-action='send']");
    return send instanceof HTMLButtonElement && !send.disabled;
  }, newChatPaneKey);
  const chatRequest = page.waitForRequest((request) =>
    new URL(request.url()).pathname === "/api/chat/turns" && request.method() === "POST",
  );
  await chatPane.locator("[data-chat-control='prompt']").evaluate((input, message) => {
    input.value = message;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.closest("form")?.requestSubmit();
  }, "Message smoke de bout en bout");
  if (process.env.CST_SMOKE_TRACE) {
    await page.waitForTimeout(150);
    const diagnostics = await chatPane.evaluate((panel) => ({
      prompt: panel.querySelector("[data-chat-control='prompt']")?.value,
      sendDisabled: panel.querySelector("[data-chat-action='send']")?.disabled,
      text: panel.textContent?.replace(/\s+/g, " ").trim().slice(-300),
    }));
    trace(`chat-send-${JSON.stringify(diagnostics)}`);
  }
  await chatRequest;
  await chatPane.getByText("Réponse smoke reçue", { exact: true }).waitFor();
  const chatPayload = capturedChatTurns.at(-1);
  if (
    !chatPayload
    || chatPayload.accountId !== accountId
    || chatPayload.prompt !== "Message smoke de bout en bout"
    || chatPayload.projectDir !== workspace
    || chatPayload.mode !== "build"
    || chatPayload.sessionId != null
  ) {
    failures.push(`chat-success: payload inattendu ${JSON.stringify(chatPayload)}`);
  }
  successfulMutationChecks.push("chat-create-and-send");
  trace("chat-success");

  currentView = "interaction-chat-fullscreen-fluidity";
  const fullscreenStart = await chatPane.evaluate((panel) => {
    globalThis.__cstDesktopFullscreenPanelIdentity = panel;
    globalThis.__cstDesktopFullscreenFeedIdentity = panel.querySelector(".chat-feed");
    panel.querySelector("[data-chat-action='fullscreen']")?.click();
    return {
      domPreserved: panel.isConnected,
      transitionStarted: panel.classList.contains("is-fullscreen-transitioning"),
    };
  });
  await page.locator(".chat-panel--expert.is-fullscreen").waitFor({ state: "visible" });
  await page.waitForFunction(() =>
    !document.querySelector(".chat-panel--expert.is-fullscreen-transitioning"));
  const desktopFullscreenDiagnostics = await chatPane.evaluate((panel) => {
    const bounds = panel.getBoundingClientRect();
    return {
      bottom: bounds.bottom,
      domPreserved: panel === globalThis.__cstDesktopFullscreenPanelIdentity
        && panel.querySelector(".chat-feed") === globalThis.__cstDesktopFullscreenFeedIdentity,
      height: bounds.height,
      left: bounds.left,
      position: getComputedStyle(panel).position,
      right: bounds.right,
      top: bounds.top,
      width: bounds.width,
    };
  });
  if (
    !fullscreenStart.domPreserved
    || !fullscreenStart.transitionStarted
    || !desktopFullscreenDiagnostics.domPreserved
    || desktopFullscreenDiagnostics.position !== "fixed"
    || Math.abs(desktopFullscreenDiagnostics.left) > 1
    || Math.abs(desktopFullscreenDiagnostics.top) > 1
    || Math.abs(desktopFullscreenDiagnostics.right - await page.evaluate(() => innerWidth)) > 1
    || Math.abs(desktopFullscreenDiagnostics.bottom - await page.evaluate(() => innerHeight)) > 1
  ) {
    failures.push(`chat-fullscreen-fluidity: bascule non ciblée ${JSON.stringify({
      fullscreenStart,
      desktopFullscreenDiagnostics,
    })}`);
  }
  await chatPane.locator("[data-chat-action='fullscreen']").click();
  await page.locator(".chat-panel--expert.is-fullscreen").waitFor({ state: "detached" });
  await page.waitForFunction(() =>
    !document.querySelector(".chat-panel--expert.is-fullscreen-transitioning"));
  interactionChecks.push("chat-fullscreen-fluidity");
  trace("chat-fullscreen-fluidity");

  currentView = "interaction-chat-automatic-orchestration";
  const orchestrationMutationCount = mutationRequests.filter(
    (entry) => entry.path === "/api/orchestrations",
  ).length;
  if (await chatPane.locator(".chat-panel-header [data-chat-action='orchestrate']").count()) {
    failures.push("chat-automatic-orchestration: l'ancien bouton Orchestrer est encore présent dans le bandeau");
  }
  const automaticToggle = chatPane.locator("[data-chat-action='toggle-automatic-orchestration']");
  await automaticToggle.waitFor({ state: "visible" });
  if ((await automaticToggle.getAttribute("aria-pressed")) !== "false") {
    failures.push("chat-automatic-orchestration: le mode direct n'a pas été conservé avant sa réactivation");
  }
  await automaticToggle.evaluate((button) => button.click());
  await page.waitForFunction((paneKey) =>
    document.querySelector(`.chat-panel--expert[data-chat-panel="${CSS.escape(paneKey)}"] [data-chat-action='toggle-automatic-orchestration']`)
      ?.getAttribute("aria-pressed") === "true", newChatPaneKey);
  const persistedAutomaticMode = await page.evaluate((paneKey) => {
    const value = localStorage.getItem("codex-switch-terminal.expert-open-chats.v1");
    if (!value) return false;
    return JSON.parse(value).panes?.find((pane) => pane.key === paneKey)?.automaticOrchestrationEnabled === true;
  }, newChatPaneKey);
  if (!persistedAutomaticMode) {
    failures.push("chat-automatic-orchestration: l'activation n'est pas persistée pour le chat");
  }

  await chatPane.locator("[data-chat-control='prompt']").fill(automaticOrchestrationPrompt);
  const chatTurnCountBeforeAutomaticOrchestration = capturedChatTurns.length;
  const automaticOrchestrationRequest = page.waitForRequest((request) =>
    new URL(request.url()).pathname === "/api/orchestrations" && request.method() === "POST",
  );
  await chatPane.locator("[data-chat-action='send']").evaluate((button) => button.click());
  await automaticOrchestrationRequest;
  await chatPane.locator("[data-chat-control='orchestration-managed']").waitFor({ state: "visible" });
  if (capturedChatTurns.length !== chatTurnCountBeforeAutomaticOrchestration) {
    failures.push("chat-automatic-orchestration-routing: un tour direct a été lancé avant l'équipe");
  }
  const automaticMutations = mutationRequests.filter(
    (entry) => entry.path === "/api/orchestrations",
  );
  const automaticPayload = automaticMutations[orchestrationMutationCount]?.payload;
  if (
    !automaticPayload
    || automaticPayload.orchestratorSessionId !== "smoke-session"
    || automaticPayload.orchestratorAccountId !== accountId
    || automaticPayload.workerCount !== 1
    || automaticPayload.adaptiveFanout !== true
    || automaticPayload.maxTaskCount !== 5
    || automaticPayload.maxConcurrency !== 5
    || JSON.stringify(automaticPayload.workerAccountIds) !== JSON.stringify([accountId])
    || automaticPayload.objective !== automaticOrchestrationPrompt
    || automaticPayload.projectDir !== workspace
  ) {
    failures.push(`chat-automatic-orchestration-launch: payload inattendu ${JSON.stringify(automaticPayload)}`);
  }
  if (await page.locator("#orchestrationConvertForm").count()) {
    failures.push("chat-automatic-orchestration: la modale manuelle s'est ouverte");
  }
  successfulMutationChecks.push("chat-automatic-orchestration");
  trace("chat-automatic-orchestration");

  if (automaticOrchestrationOnly) {
    const duplicateIds = await page.evaluate(() => {
      const counts = new Map();
      for (const element of document.querySelectorAll("[id]")) {
        counts.set(element.id, (counts.get(element.id) || 0) + 1);
      }
      return [...counts].filter(([, count]) => count > 1);
    });
    if (duplicateIds.length) failures.push(`DOM: identifiants dupliqués ${JSON.stringify(duplicateIds)}`);
    process.stdout.write(`${JSON.stringify({ site, interactionChecks, successfulMutationChecks, duplicateIds, failures }, null, 2)}\n`);
    await context.close();
    await browser.close();
    process.exit(failures.length ? 1 : 0);
  }

  currentView = "mobile-startup";
  progress("navigation mobile");
  await page.setViewportSize({ width: 390, height: 844 });
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.locator("#chatAppSidebar").waitFor({ state: "attached" });
  const mobileChrome = page.locator(".m-chrome");
  const mobileDiagnostics = await page.evaluate(() => ({
    innerWidth: window.innerWidth,
    mediaMatches: window.matchMedia("(max-width: 860px)").matches,
    display: getComputedStyle(document.querySelector(".m-chrome")).display,
    remoteLogin: Boolean(document.querySelector(".remote-login")),
    boot: Boolean(document.querySelector(".boot, .boot-splash")),
  }));
  if (!mobileDiagnostics.mediaMatches || mobileDiagnostics.display === "none") {
    throw new Error(`La coque mobile reste masquée : ${JSON.stringify(mobileDiagnostics)}`);
  }

  const drawerButton = page.locator('[data-m="drawer"]');
  await drawerButton.waitFor({ state: "visible" });
  await drawerButton.click();
  if (!(await page.locator("body").evaluate((body) => body.classList.contains("chat-sidebar-open")))) {
    failures.push("mobile: le bouton du tiroir ne l'ouvre pas");
  }
  await drawerButton.click();

  const mobileChatTab = page.locator('.m-bottomnav [data-view="chat"]');
  await mobileChatTab.click();
  await page.locator('[data-m="new"]').click();
  await page.locator("#confirmNewChat").click();
  const mobileChatPane = page.locator(".chat-panel--expert.active").first();
  await mobileChatPane.waitFor({ state: "visible" });
  await mobileChatPane.evaluate((panel) => {
    globalThis.__cstFullscreenPanelIdentity = panel;
    globalThis.__cstFullscreenFeedIdentity = panel.querySelector(".chat-feed");
  });

  const mobileChatFullscreenViewports = [
    { name: "portrait", width: 390, height: 844 },
    { name: "compact", width: 390, height: 568 },
    { name: "landscape", width: 844, height: 390 },
  ];
  for (const viewport of mobileChatFullscreenViewports) {
    currentView = `mobile-chat-fullscreen-${viewport.name}`;
    await page.setViewportSize({ width: viewport.width, height: viewport.height });
    await page.waitForTimeout(160);

    const fullscreenButton = mobileChatPane.locator('[data-chat-action="fullscreen"]');
    await fullscreenButton.click();
    const fullscreenPane = page.locator(".chat-panel--expert.is-fullscreen").first();
    await fullscreenPane.waitFor({ state: "visible" });
    const fullscreenDiagnostics = await fullscreenPane.evaluate((panel) => {
      const rect = (element) => {
        const bounds = element?.getBoundingClientRect();
        return bounds ? {
          bottom: bounds.bottom,
          height: bounds.height,
          left: bounds.left,
          right: bounds.right,
          top: bounds.top,
          width: bounds.width,
        } : null;
      };
      const visible = (element) => {
        const style = getComputedStyle(element);
        const bounds = element.getBoundingClientRect();
        return style.display !== "none"
          && style.visibility !== "hidden"
          && Number(style.opacity) > 0
          && bounds.width > 0
          && bounds.height > 0;
      };
      const panelRect = panel.getBoundingClientRect();
      const workspace = panel.closest(".chat-main-workspace");
      const topbar = document.querySelector(".m-topbar");
      const bottomnav = document.querySelector(".m-bottomnav");
      const conversation = panel.querySelector(".chat-conversation-body");
      const feed = panel.querySelector(".chat-feed");
      const composer = panel.querySelector(".chat-composer");
      const composerBox = panel.querySelector(".chat-composer-box");
      const toolbar = panel.querySelector(".chat-composer-toolbar");
      const agentTools = panel.querySelector(".chat-agent-tools");
      const clippedControls = [...panel.querySelectorAll("button, input, select, textarea")]
        .filter(visible)
        .filter((element) => {
          const bounds = element.getBoundingClientRect();
          const horizontalOverflow = bounds.left < panelRect.left - 1 || bounds.right > panelRect.right + 1;
          const verticalOverflow = bounds.top < panelRect.top - 1 || bounds.bottom > panelRect.bottom + 1;
          if (!horizontalOverflow && !verticalOverflow) return false;
          let horizontallyReachable = !horizontalOverflow;
          let verticallyReachable = !verticalOverflow;
          for (let parent = element.parentElement; parent && parent !== panel; parent = parent.parentElement) {
            const style = getComputedStyle(parent);
            if (
              (style.overflowX === "auto" || style.overflowX === "scroll")
              && parent.scrollWidth > parent.clientWidth
            ) horizontallyReachable = true;
            if (
              (style.overflowY === "auto" || style.overflowY === "scroll")
              && parent.scrollHeight > parent.clientHeight
            ) verticallyReachable = true;
          }
          return !horizontallyReachable || !verticallyReachable;
        })
        .map((element) => ({
          bounds: rect(element),
          name: element.getAttribute("aria-label") || element.getAttribute("title") || element.tagName,
        }));

      let transcriptEndReachable = false;
      if (feed) {
        const previousScrollTop = feed.scrollTop;
        feed.scrollTop = feed.scrollHeight;
        const feedRect = feed.getBoundingClientRect();
        const lastMessageRect = feed.lastElementChild?.getBoundingClientRect();
        transcriptEndReachable = !lastMessageRect || lastMessageRect.bottom <= feedRect.bottom + 1;
        feed.scrollTop = previousScrollTop;
      }

      return {
        agentToolsDisplay: agentTools ? getComputedStyle(agentTools).display : null,
        bottomnav: rect(bottomnav),
        clippedControls,
        composer: rect(composer),
        composerHorizontalOverflow: composerBox ? composerBox.scrollWidth - composerBox.clientWidth : null,
        conversation: rect(conversation),
        domPreserved: panel === globalThis.__cstFullscreenPanelIdentity
          && feed === globalThis.__cstFullscreenFeedIdentity,
        feed: rect(feed),
        feedOverflowY: feed ? getComputedStyle(feed).overflowY : null,
        panel: rect(panel),
        panelPosition: getComputedStyle(panel).position,
        toolbarHorizontalOverflow: toolbar ? toolbar.scrollWidth - toolbar.clientWidth : null,
        topbar: rect(topbar),
        transcriptEndReachable,
        visibleAgentToolLabels: agentTools
          ? [...agentTools.querySelectorAll("span")].filter(visible).map((label) => label.textContent?.trim())
          : [],
        workspace: rect(workspace),
      };
    });

    const panel = fullscreenDiagnostics.panel;
    const workspace = fullscreenDiagnostics.workspace;
    const topbar = fullscreenDiagnostics.topbar;
    const bottomnav = fullscreenDiagnostics.bottomnav;
    const conversation = fullscreenDiagnostics.conversation;
    const feed = fullscreenDiagnostics.feed;
    const composer = fullscreenDiagnostics.composer;
    if (
      !panel || !workspace || !topbar || !bottomnav || !fullscreenDiagnostics.domPreserved
      || fullscreenDiagnostics.panelPosition !== "absolute"
      || Math.abs(panel.top - workspace.top) > 1
      || Math.abs(panel.bottom - workspace.bottom) > 1
      || panel.top < topbar.bottom - 1
      || panel.bottom > bottomnav.top + 1
      || panel.left < -1
      || panel.right > viewport.width + 1
    ) {
      failures.push(`mobile-chat-${viewport.name}: plein ecran hors coque ${JSON.stringify(fullscreenDiagnostics)}`);
    }
    if (
      !conversation || conversation.height <= 0
      || !feed || feed.height <= 0
      || fullscreenDiagnostics.feedOverflowY !== "auto"
      || !fullscreenDiagnostics.transcriptEndReachable
      || !composer || composer.height <= 0
      || composer.bottom > panel.bottom + 1
    ) {
      failures.push(`mobile-chat-${viewport.name}: transcript ou compositeur inaccessible ${JSON.stringify(fullscreenDiagnostics)}`);
    }
    if (
      fullscreenDiagnostics.agentToolsDisplay !== "grid"
      || fullscreenDiagnostics.visibleAgentToolLabels.length
      || (fullscreenDiagnostics.composerHorizontalOverflow ?? 1) > 1
      || (fullscreenDiagnostics.toolbarHorizontalOverflow ?? 1) > 1
      || fullscreenDiagnostics.clippedControls.length
    ) {
      failures.push(`mobile-chat-${viewport.name}: outils non replies ou tronques ${JSON.stringify(fullscreenDiagnostics)}`);
    }
    mobileVisited.push(`chat-fullscreen/${viewport.name}`);
    await fullscreenPane.locator('[data-chat-action="fullscreen"]').click();
    await page.locator(".chat-panel--expert.is-fullscreen").waitFor({ state: "detached" });
  }

  for (const name of ["chat", "terminal", "messaging", "forum"]) {
    currentView = `mobile-${name}`;
    const tab = page.locator(`.m-bottomnav [data-view="${name}"]`);
    if (await tab.count() !== 1) {
      failures.push(`mobile-${name}: onglet absent`);
      continue;
    }
    await tab.click();
    await page.waitForTimeout(100);
    if ((await tab.getAttribute("aria-current")) !== "page") {
      failures.push(`mobile-${name}: l'onglet actif n'est pas annoncé`);
    }
    await auditAccessibilityAndOverflow(`mobile-${name}`);
    mobileVisited.push(name);
  }

  currentView = "mobile-more";
  const moreButton = page.locator('[data-m="menu"]');
  const morePanel = page.locator(".m-sheet-panel");
  let expectedMobileMoreItemCount = null;
  const mobileSheetViewports = [
    { name: "portrait", width: 390, height: 844, mustScroll: false },
    { name: "compact", width: 390, height: 568, mustScroll: true },
    { name: "landscape", width: 844, height: 390, mustScroll: true },
  ];
  for (const viewport of mobileSheetViewports) {
    currentView = `mobile-more-${viewport.name}`;
    await page.setViewportSize({ width: viewport.width, height: viewport.height });
    await page.waitForTimeout(160);
    await moreButton.click();
    await page.waitForFunction(() => {
      const sheet = document.querySelector(".m-sheet");
      return !!sheet && Math.abs(sheet.getBoundingClientRect().bottom - window.innerHeight) <= 1;
    });
    if (!(await morePanel.isVisible())) {
      failures.push(`mobile-${viewport.name}: le menu Plus ne s'ouvre pas`);
      if ((await moreButton.getAttribute("aria-expanded")) === "true") await moreButton.click();
      continue;
    }
    const sheetDiagnostics = await morePanel.evaluate((panel) => {
      const panelRect = panel.getBoundingClientRect();
      const topbarRect = document.querySelector(".m-topbar")?.getBoundingClientRect();
      const bottomnavRect = document.querySelector(".m-bottomnav")?.getBoundingClientRect();
      const panelStyle = getComputedStyle(panel);
      const items = [...panel.querySelectorAll("[role='menuitem']")];
      const safeTop = Math.max(panelRect.top, topbarRect?.bottom ?? 0);
      const safeBottom = Math.min(panelRect.bottom, bottomnavRect?.top ?? window.innerHeight);
      const maxScrollTop = panel.scrollHeight - panel.clientHeight;
      const unreachable = [];

      for (const item of items) {
        panel.scrollTop = 0;
        const initialRect = item.getBoundingClientRect();
        const centeredTop = safeTop + (safeBottom - safeTop - initialRect.height) / 2;
        panel.scrollTop = Math.max(0, Math.min(maxScrollTop, initialRect.top - centeredTop));
        const itemRect = item.getBoundingClientRect();
        if (itemRect.top < safeTop - 1 || itemRect.bottom > safeBottom + 1) {
          unreachable.push(item.textContent?.trim() || "sans libelle");
        }
      }
      panel.scrollTop = 0;

      return {
        bottom: panelRect.bottom,
        clientHeight: panel.clientHeight,
        itemCount: items.length,
        overflowY: panelStyle.overflowY,
        overscrollBehaviorY: panelStyle.overscrollBehaviorY,
        scrollHeight: panel.scrollHeight,
        top: panelRect.top,
        topbarBottom: topbarRect?.bottom ?? 0,
        unreachable,
        viewportHeight: window.innerHeight,
      };
    });
    if (
      sheetDiagnostics.top < sheetDiagnostics.topbarBottom - 1
      || sheetDiagnostics.bottom > sheetDiagnostics.viewportHeight + 1
    ) {
      failures.push(`mobile-${viewport.name}: feuille hors viewport ${JSON.stringify(sheetDiagnostics)}`);
    }
    if (sheetDiagnostics.overflowY !== "auto" || sheetDiagnostics.overscrollBehaviorY !== "contain") {
      failures.push(`mobile-${viewport.name}: defilement non confine ${JSON.stringify(sheetDiagnostics)}`);
    }
    if (viewport.mustScroll && sheetDiagnostics.scrollHeight <= sheetDiagnostics.clientHeight) {
      failures.push(`mobile-${viewport.name}: la faible hauteur ne declenche pas le defilement`);
    }
    expectedMobileMoreItemCount ??= sheetDiagnostics.itemCount;
    if (
      expectedMobileMoreItemCount < 21
      || sheetDiagnostics.itemCount !== expectedMobileMoreItemCount
      || sheetDiagnostics.unreachable.length
    ) {
      failures.push(`mobile-${viewport.name}: actions inaccessibles ${JSON.stringify(sheetDiagnostics)}`);
    }
    await auditAccessibilityAndOverflow(`mobile-more-${viewport.name}`);
    mobileVisited.push(`more/${viewport.name}`);
    await page.keyboard.press("Escape");
  }

  await page.setViewportSize({ width: 390, height: 844 });
  await page.waitForTimeout(160);
  await moreButton.click();
  const autonomousEntry = morePanel.locator('[data-view="autonomous"]');
  await autonomousEntry.click();
  if (!await page.locator(".autonomous-panel").waitFor({ state: "visible" }).then(
    () => true,
    () => false,
  )) {
    failures.push("mobile: la navigation Plus vers Agents autonomes échoue");
  }
  mobileVisited.push("more/autonomous");
  trace("mobile-complete");

  const duplicateIds = await page.evaluate(() => {
    const counts = new Map();
    for (const element of document.querySelectorAll("[id]")) {
      counts.set(element.id, (counts.get(element.id) || 0) + 1);
    }
    return [...counts].filter(([, count]) => count > 1);
  });
  if (duplicateIds.length) failures.push(`DOM: identifiants dupliqués ${JSON.stringify(duplicateIds)}`);
  await context.close();

  progress("états API dégradés");
  const errorStateChecks = [];
  const errorContext = await browser.newContext({
    viewport: { width: 1280, height: 800 },
    locale: "fr-FR",
    serviceWorkers: "block",
  });
  await errorContext.addInitScript(({ workspacePath }) => {
    localStorage.setItem("codex-switch-terminal.remote.token", "smoke-token");
    localStorage.setItem("codex-switch-terminal.workspace.path", workspacePath);
  }, { workspacePath: workspace });
  const errorPage = await errorContext.newPage();
  errorPage.setDefaultTimeout(10_000);
  let errorCurrentView = "startup";
  errorPage.on("pageerror", (error) => {
    failures.push(`error-state pageerror[${errorCurrentView}]: ${error.stack || error}`);
  });
  errorPage.on("console", (message) => {
    if (message.type() !== "error") return;
    if (
      message.text().includes("WebSocket connection to")
      && (message.text().includes("/ws/discussions") || message.text().includes("/ws/runtime"))
    ) {
      ignoredWebSocketFailures += 1;
    } else if (message.text().includes("Failed to load resource") && message.text().includes("503")) {
      expectedApiFailures += 1;
    } else {
      failures.push(`error-state console[${errorCurrentView}]: ${message.text()}`);
    }
  });
  await errorPage.route("**/healthz", (route) => route.fulfill({
    json: { ok: true, ready: true, version: "0.1.0", commit: "local" },
  }));
  await errorPage.route("**/api/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/settings") return route.fulfill({ json: settings });
    // L'authentification fait partie du bootstrap, pas des vues fonctionnelles
    // testees ci-dessous. Si son endpoint tombe aussi en 503, l'application
    // s'arrete avant le premier rendu et aucun etat degrade ne peut etre audite.
    if (path === "/api/auth/config") {
      return route.fulfill({
        json: {
          enabled: false,
          registrationEnabled: false,
          googleEnabled: false,
        },
      });
    }
    return route.fulfill({
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({ error: "Indisponibilité simulée par le smoke test" }),
    });
  });
  await errorPage.goto(`${site}/?smoke=errors`, { waitUntil: "domcontentloaded" });
  await errorPage.locator("#chatAppSidebar").waitFor({ state: "visible" });
  for (const [selector, name] of [
    ["#autonomousToggle", "autonomous"],
    ["#orchestrationToggle", "orchestration"],
    ["#sideDiscussions", "discussions"],
    ["#dashboardToggle", "dashboard"],
    ["#limitsToggle", "limits"],
    ["#settingsToggle", "settings"],
  ]) {
    errorCurrentView = name;
    if (!(await activateSidebarDestination(errorPage, selector))) {
      failures.push(`error-state ${name}: bouton ${selector} absent`);
      continue;
    }
    await errorPage.waitForTimeout(100);
    const panel = errorPage.locator(".chat-admin-panel");
    if (!(await panel.isVisible()) || !(await panel.innerText()).trim()) {
      failures.push(`error-state ${name}: la vue disparaît lorsque l'API échoue`);
    }
    errorStateChecks.push(name);
    trace(`error-view-${name}`);
  }
  await errorContext.close();

  progress("résultat");
  process.stdout.write(`${JSON.stringify({ site, visited, interactionChecks, successfulMutationChecks, mobileVisited, errorStateChecks, accessibilityResponsiveChecks, keyboardFocusChecks, duplicateIds, ignoredWebSocketFailures, ignoredRequestAborts, expectedApiFailures, failures }, null, 2)}\n`);
  if (failures.length) process.exitCode = 1;
} finally {
  await browser.close();
}
