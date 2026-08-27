import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const watchdog = readFileSync(
  new URL("../deploy/cst-memory-watchdog.sh", import.meta.url),
  "utf8",
);
const compose = readFileSync(new URL("../compose.yaml", import.meta.url), "utf8");

test("le watchdog ne met pas le noeud en drain sur la charge CPU par defaut", () => {
  assert.match(
    compose,
    /CST_MEMORY_WATCHDOG_CPU_PERCENT:\s*\$\{CST_MEMORY_WATCHDOG_CPU_PERCENT:-0\}/,
  );
  assert.match(
    watchdog,
    /nonnegative_integer "\$\{CST_MEMORY_WATCHDOG_CPU_PERCENT:-\}" 0/,
  );

  const disabledCpuGuard = watchdog.indexOf(
    "if ((CPU_LOAD_PERCENT_PER_CORE == 0))",
  );
  const loadAverageRead = watchdog.indexOf("read -r load_one _ </proc/loadavg");
  assert.ok(disabledCpuGuard >= 0 && disabledCpuGuard < loadAverageRead);
});

test("le watchdog conserve les deux protections memoire", () => {
  assert.match(watchdog, /container_headroom_kib < REQUIRED_CONTAINER_HEADROOM_KIB/);
  assert.match(watchdog, /host_headroom_kib < REQUIRED_HOST_HEADROOM_KIB/);
});

test("le watchdog utilise le port CST_BIND du serveur", () => {
  assert.match(watchdog, /server_port="\$\{CST_BIND:-127\.0\.0\.1:8080\}"/);
  assert.match(
    watchdog,
    /http:\/\/127\.0\.0\.1:\$\{server_port\}\/api\/admin\/drain/,
  );
});
