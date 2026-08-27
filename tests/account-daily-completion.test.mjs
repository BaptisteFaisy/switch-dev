import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(
  new URL("../src/account-daily-completion.ts", import.meta.url),
  "utf8",
);
const compiled = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const completion = await import(
  `data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`
);

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const style = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");
const backend = readFileSync(
  new URL("../src-tauri/src/settings.rs", import.meta.url),
  "utf8",
);

test("le jour quotidien suit le calendrier local et bascule a minuit", () => {
  const beforeMidnight = new Date(2026, 7, 20, 23, 59, 59, 500);
  const afterMidnight = new Date(2026, 7, 21, 0, 0, 0, 0);

  assert.equal(completion.localCalendarDay(beforeMidnight), "2026-08-20");
  assert.equal(completion.millisecondsUntilNextLocalMidnight(beforeMidnight), 500);
  assert.equal(
    completion.accountCompletedToday({ completedOn: "2026-08-20" }, beforeMidnight),
    true,
  );
  assert.equal(
    completion.accountCompletedToday({ completedOn: "2026-08-20" }, afterMidnight),
    false,
  );
});

test("l'etoile pleine/vide est persistante, accessible et reinitialisee automatiquement", () => {
  assert.match(main, /completedOn\?: string \| null/);
  assert.match(main, /account\.completedOn = completed \? localCalendarDay\(\) : null/);
  assert.match(main, /invoke<AppSettings>\("save_settings", \{ settings \}\)/);
  assert.match(main, /scheduleAccountCompletionMidnightReset\(\)/);
  assert.match(main, /refreshAccountCompletions\(\)/);
  assert.match(main, /millisecondsUntilNextLocalMidnight\(\) \+ 100/);
  assert.match(main, /aria-pressed="\$\{completedToday\}"/);
  assert.match(style, /\.account-daily-completion-toggle\.active svg[\s\S]*?fill: currentColor/);
  assert.match(backend, /pub completed_on: Option<String>/);
  assert.match(backend, /skip_serializing_if = "Option::is_none"/);
});
