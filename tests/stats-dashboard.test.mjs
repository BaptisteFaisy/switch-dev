import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  STATS_RANGE_OPTIONS,
  accountTokenUsageForDate,
  aggregateApiModelUsage,
  aggregateAccountTokenDays,
  buildAccountTokenSeries,
  buildWorkTimeBuckets,
  deduplicateAccountTokenAccounts,
  recentDateKeys,
  sumTokenUsage,
} from "../src/stats.ts";

const usageDay = (date, totalTokens, inputTokens = totalTokens, outputTokens = 0) => ({
  date,
  inputTokens,
  cachedInputTokens: 0,
  outputTokens,
  totalTokens,
  costUsd: totalTokens / 1_000_000,
});

const modelUsage = (model, totalTokens, inputTokens, outputTokens, apiEquivalentUsd) => ({
  model,
  pricingModel: model,
  priced: true,
  inputTokens,
  cachedInputTokens: 0,
  outputTokens,
  reasoningOutputTokens: 0,
  totalTokens,
  apiEquivalentUsd,
  inputPricePerMillion: 1,
  cachedInputPricePerMillion: 0.1,
  outputPricePerMillion: 6,
  longContextRequests: 0,
});

test("les périodes proposées sont aujourd'hui, 7 jours et 30 jours", () => {
  assert.deepEqual(
    STATS_RANGE_OPTIONS.map(({ days, label }) => [days, label]),
    [
      [1, "Aujourd'hui"],
      [7, "7 jours"],
      [30, "30 jours"],
    ],
  );
});

test("les tokens de tous les comptes sont additionnés par jour", () => {
  const totals = aggregateAccountTokenDays({
    accounts: [
      {
        id: "first",
        label: "Premier",
        days: [usageDay("2026-07-12", 100, 80, 20), usageDay("2026-07-13", 250)],
      },
      {
        id: "second",
        label: "Second",
        days: [usageDay("2026-07-12", 400, 300, 100), usageDay("2026-07-13", 50)],
      },
    ],
  });

  assert.equal(totals.get("2026-07-12")?.totalTokens, 500);
  assert.equal(totals.get("2026-07-12")?.inputTokens, 380);
  assert.equal(totals.get("2026-07-12")?.outputTokens, 120);
  assert.equal(totals.get("2026-07-13")?.totalTokens, 300);
});

test("les jetons et l'equivalent API sont additionnes par modele", () => {
  const models = aggregateApiModelUsage([
    modelUsage("gpt-5.6-luna", 100, 90, 10, 0.00015),
    modelUsage("gpt-5.6-sol", 250, 225, 25, 0.001875),
    modelUsage("GPT-5.6-LUNA", 50, 45, 5, 0.000075),
  ]);

  assert.deepEqual(
    models.map(({ model, totalTokens }) => [model, totalTokens]),
    [
      ["gpt-5.6-sol", 250],
      ["gpt-5.6-luna", 150],
    ],
  );
  assert.equal(models[1].inputTokens, 135);
  assert.equal(models[1].outputTokens, 15);
  assert.equal(models[1].apiEquivalentUsd, 0.000225);
});

test("la page affiche les tarifs publics et la ventilation par modele", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const backend = readFileSync(new URL("../src-tauri/src/metrics.rs", import.meta.url), "utf8");

  assert.match(main, /Jetons et coût public par modèle/);
  assert.match(main, /Prix public \/ 1 M/);
  assert.match(main, /formatApiUsd\(model\.apiEquivalentUsd\)/);
  assert.match(main, /developers\.openai\.com\/api\/docs\/pricing/);
  assert.match(backend, /pub fn public_rates_for_model/);
  assert.match(backend, /long_context_threshold_tokens: Some\(272_000\)/);
  assert.match(backend, /tarif arbitraire/);
});

test("les profils qui partagent le meme CODEX_HOME ne sont comptes qu'une fois", () => {
  const sharedDay = usageDay("2026-07-13", 100);
  const accounts = deduplicateAccountTokenAccounts([
    {
      id: "copy-a",
      label: "Compte copie A",
      codexHome: "%CST_DATA_DIR%\\Codex-Homes/Shared",
      totalTokens: 100,
      days: [sharedDay],
    },
    {
      id: "copy-b",
      label: "Compte copie B",
      codexHome: "%cst_data_dir%/codex-homes/shared/",
      totalTokens: 100,
      days: [sharedDay],
    },
    {
      id: "other",
      label: "Autre compte",
      codexHome: "%CST_DATA_DIR%/codex-homes/other",
      totalTokens: 50,
      days: [usageDay("2026-07-13", 50)],
    },
  ]);

  assert.equal(accounts.length, 2);
  assert.equal(
    aggregateAccountTokenDays({ accounts: [...accounts, accounts[0]] }).get("2026-07-13")
      ?.totalTokens,
    150,
  );
});

test("la fenêtre de 30 jours est calendaire et traverse les changements de mois", () => {
  const keys = recentDateKeys("2026-03-02", 7);
  assert.deepEqual(keys, [
    "2026-02-24",
    "2026-02-25",
    "2026-02-26",
    "2026-02-27",
    "2026-02-28",
    "2026-03-01",
    "2026-03-02",
  ]);

  const series = buildAccountTokenSeries(
    { accounts: [{ days: [usageDay("2026-02-28", 120), usageDay("2026-03-02", 80)] }] },
    "2026-03-02",
    7,
  );
  assert.equal(series.length, 7);
  assert.equal(series[1].totalTokens, 0, "les jours sans activité restent visibles");
  assert.equal(sumTokenUsage(series).totalTokens, 200);
});

test("la page branche les boutons de période sur le re-rendu de la courbe", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  assert.match(main, /data-stats-range="\$\{range\.days\}"/);
  assert.match(main, /querySelectorAll<HTMLButtonElement>\("\[data-stats-range\]"\)/);
  assert.match(main, /statsRangeDays = range;\s*render\(\);/);
  assert.match(main, /data-stats-date=/);
  assert.match(main, /selectedStatsDate = date;\s*render\(\);/);
  assert.doesNotMatch(main, />14 jours</);
});

test("le temps de travail est regroupé par jour, semaine ISO et mois", () => {
  const days = [
    { date: "2026-06-30", activeSeconds: 3_600, turnCount: 1 },
    { date: "2026-07-01", activeSeconds: 1_800, turnCount: 2 },
    { date: "2026-07-06", activeSeconds: 7_200, turnCount: 3 },
    { date: "2026-07-14", activeSeconds: 900, turnCount: 1 },
  ];

  const daily = buildWorkTimeBuckets(days, "2026-07-14", "day");
  const weekly = buildWorkTimeBuckets(days, "2026-07-14", "week");
  const monthly = buildWorkTimeBuckets(days, "2026-07-14", "month");

  assert.equal(daily.length, 14);
  assert.equal(daily.at(-1).activeSeconds, 900);
  assert.equal(weekly.at(-1).startDate, "2026-07-13");
  assert.equal(weekly.at(-1).activeSeconds, 900);
  assert.equal(weekly.at(-2).activeSeconds, 7_200);
  assert.equal(weekly.at(-3).activeSeconds, 5_400);
  assert.equal(monthly.at(-1).activeSeconds, 9_900);
  assert.equal(monthly.at(-2).activeSeconds, 3_600);
});

test("l'onglet temps de travail charge la mesure locale et exclut explicitement l'autonome", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
  const backend = readFileSync(new URL("../src-tauri/src/work_time.rs", import.meta.url), "utf8");

  assert.match(main, /data-stats-tab="work-time"/);
  assert.match(main, /invoke<WorkTimeDashboard>\("work_time_dashboard"\)/);
  assert.match(main, /data-work-time-granularity=/);
  assert.match(platform, /case "work_time_dashboard":\s*return api<T>\("GET", "\/api\/work-time"\)/);
  // Import simple ou groupe : seule compte la reutilisation du helper partage.
  assert.match(backend, /discussions::(\{[^}]*)?is_autonomous_prompt/);
  assert.match(backend, /interval\.start_ms <= \*end/);
});

test("les tokens par compte sont relus pendant le poll temps reel", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  assert.match(
    main,
    /runWhenPageVisible\(\(\) => \{\s*void refreshUsageDashboard\(\);\s*void refreshWorkTimeDashboard\(\);/,
  );
  assert.match(
    main,
    /accountUsageLastRefreshAt >= ACCOUNT_USAGE_REFRESH_INTERVAL_MS[\s\S]*?void refreshAccountUsage\(\);/,
  );
  assert.match(main, /accountUsageChanged =\s*!accountUsageLoaded \|\| nextSignature !== accountUsageSignature/);
});

test("les stats gardent les rollouts locaux comme source canonique", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const backend = readFileSync(
    new URL("../src-tauri/src/account_usage.rs", import.meta.url),
    "utf8",
  );

  assert.doesNotMatch(backend, /account\/usage\/read/);
  assert.match(backend, /delta_above\(&high_water\)/);
  assert.match(main, /sans estimation distante/);
  assert.doesNotMatch(main, /Source prioritaire : usage du compte Codex/);
});

test("l'actualisation conserve le scroll interne de la page stats", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  assert.match(
    main,
    /const adminContentScrollTop =\s*document\.querySelector<HTMLElement>\("\.chat-admin-panel > :first-child"\)\?\.scrollTop \?\? 0;/,
  );
  assert.match(
    main,
    /restoredAdminContent\.scrollTop = adminContentScrollTop;/,
  );
});

test("les points restent atteignables au doigt et le jour actif est revele", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const style = readFileSync(new URL("../src/stats-view.css", import.meta.url), "utf8");
  assert.match(main, /const hitRadius = days\.length === 1 \? 65 : days\.length <= 7 \? 36 : 16/);
  assert.match(main, /const revealSelectedStatsPoint = \(\) =>/);
  assert.match(main, /pointRect\.left - chartRect\.left - chart\.clientWidth \/ 2/);
  assert.match(style, /\.stats-point-chart\.range-30 svg \{\s*min-width: 1440px;/);
  assert.match(style, /\.stats-range button \{\s*min-height: 44px;/);
});

test("les styles stats quittent le chemin initial et se chargent avec la vue", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const initialStyles = ["style.css", "theme.css"]
    .map((file) => readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8"))
    .join("\n");
  const view = readFileSync(new URL("../src/stats-view.ts", import.meta.url), "utf8");
  const viewStyle = readFileSync(new URL("../src/stats-view.css", import.meta.url), "utf8");

  assert.match(main, /type StatsViewModule = typeof import\("\.\/stats-view"\)/);
  assert.match(main, /statsViewModulePromise = import\("\.\/stats-view"\)/);
  assert.match(main, /if \(view === "dashboard" && !statsViewModule\)/);
  assert.match(view, /import "\.\/stats-view\.css";/);
  assert.doesNotMatch(initialStyles, /\.(?:stats-|work-time-)/);
  assert.match(viewStyle, /\.stats-dashboard\s*\{/);
  assert.match(viewStyle, /:root\[data-theme="light"\] \.stats-dashboard/);
});

test("le detail d'un jour conserve tous les comptes et calcule leur part", () => {
  const rows = accountTokenUsageForDate(
    {
      accounts: [
        {
          id: "alpha",
          label: "Alpha",
          profileLabels: ["Alpha", "Ancien profil Alpha"],
          usageSource: "codex-account",
          days: [usageDay("2026-07-13", 750)],
        },
        {
          id: "beta",
          label: "Beta",
          usageSource: "codex-account",
          days: [usageDay("2026-07-13", 250)],
        },
        {
          id: "gamma",
          label: "Gamma",
          usageSource: "local-sessions",
          days: [],
        },
      ],
    },
    "2026-07-13",
  );

  assert.deepEqual(
    rows.map(({ accountId, totalTokens, share }) => [accountId, totalTokens, share]),
    [
      ["alpha", 750, 0.75],
      ["beta", 250, 0.25],
      ["gamma", 0, 0],
    ],
  );
  assert.equal(rows[0].profileLabels.length, 2);
});

test("la page reste compatible avec les reponses d'usage de l'ancienne stable", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  assert.match(main, /const normalizeAccountUsageDashboard =/);
  assert.match(main, /profileLabels: profileLabels\.length \? profileLabels : \[account\.label\]/);
  assert.match(main, /: "local-sessions";/);
  assert.match(main, /const nextAccountUsage = normalizeAccountUsageDashboard\(/);
});

test("les stats détaillent et totalisent les tokens des agents autonomes", () => {
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
  const style = readFileSync(new URL("../src/stats-view.css", import.meta.url), "utf8");
  assert.match(main, /const renderStatsAutonomousTokenUsage =/);
  assert.match(main, /Consommation détaillée par agent/);
  assert.match(main, /Total des agents/);
  assert.match(main, /reasoningOutputTokens/);
  assert.match(main, /renderStatsAutonomousTokenUsage\(\)/);
  assert.match(style, /\.stats-autonomous-token-card \.stats-api-table tfoot th/);
});
