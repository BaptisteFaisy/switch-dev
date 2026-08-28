import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const start = readFileSync(new URL("../scripts/start-switch-development-runtime.ps1", import.meta.url), "utf8");
const watch = readFileSync(new URL("../scripts/watch-switch-development-runtime.ps1", import.meta.url), "utf8");
const restart = readFileSync(new URL("../scripts/restart-switch-development.ps1", import.meta.url), "utf8");
const agents = readFileSync(new URL("../AGENTS.md", import.meta.url), "utf8");

test("le serveur de developpement tourne depuis le disque interne", () => {
  assert.match(start, /LOCALAPPDATA[\s\S]*?SwitchDevelopmentRuntime/);
  assert.match(start, /Get-FileHash -Algorithm SHA256/);
  assert.match(start, /CST_STATIC_DIR = \$runtimeStaticPath/);
  assert.match(start, /Start-Process -FilePath \$runtimeServerPath/);
  assert.match(start, /RedirectStandardError \$stderrPath/);
  assert.doesNotMatch(start, /Start-Process -FilePath \$sourceServerPath/);
});

test("le launcher rend la main au watchdog sans quitter son processus", () => {
  assert.doesNotMatch(start, /\bexit\s+0\b/i);
  assert.match(start, /Switch developpement est deja disponible[\s\S]*?return/);
  assert.match(watch, /while \(\$true\)/);
  assert.match(watch, /Wait-Process -Id \$serverPid/);
  assert.match(watch, /catch \{[\s\S]*?demarrage impossible[\s\S]*?continue/);
});

test("un redemarrage attend l'inactivite puis draine avant l'arret", () => {
  const activeCheck = restart.indexOf("Get-ActiveWorkloadCount -Health $health");
  const drain = restart.indexOf("Set-DevelopmentDrain -Draining $true");
  const stop = restart.indexOf("Stop-Process -Id");
  assert.ok(activeCheck >= 0 && drain > activeCheck && stop > drain);
  assert.match(restart, /activeTerminals[\s\S]*?activeChatTurns/);
  assert.match(restart, /Redemarrage annule sans interruption/);
  assert.match(restart, /Assert-ExpectedServerProcess/);
  assert.match(restart, /SwitchDevelopmentRuntime/);
});

test("les consignes locales imposent le redemarrage protege", () => {
  assert.match(agents, /restart-switch-development\.ps1/);
  assert.match(agents, /ne jamais arr[eê]ter directement `cst-server\.exe`/i);
});
