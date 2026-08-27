// Compare sessionId entre tokscale et l'estimateur cumulatif Freebuff.
// Usage: node debug-comparison.mjs

import { execSync } from "node:child_process";
import { readFileSync, mkdirSync, existsSync } from "node:fs";
import { join, basename } from "node:path";

const HOMES = "/srv/cst/codex-homes";

// 1) Lance l'estimateur cumulatif
console.log("=== ESTIMATEUR ===");
const estOut = execSync(
  `node /srv/cst/freebuff-estimate.mjs ${HOMES}`,
  { encoding: "utf8" }
);
const estSessions = JSON.parse(estOut);
console.log(`  ${estSessions.length} sessions`);

// 2) Stage + tokscale pour un seul compte
const ACCOUNT = "freebuff-baptiste-faisy";
const homeDir = `${HOMES}/${ACCOUNT}`;
if (!existsSync(homeDir)) {
  console.log(`Compte introuvable: ${homeDir}`);
  process.exit(1);
}

const TMP = `/tmp/comp-${Date.now()}`;
mkdirSync(TMP, { recursive: true });
const STAGE = `${TMP}/stage`;
mkdirSync(STAGE, { recursive: true });

execSync(`node /srv/cst/freebuff-stage.mjs ${homeDir} ${STAGE}`, {
  stdio: "pipe",
});

console.log("\n=== TOKSCALE ===");
const tokOut = execSync(
  `FREEBUFF_DATA_DIR=${STAGE} npx --yes tokscale@latest models --json --client freebuff --group-by session,model --week`,
  { encoding: "utf8", maxBuffer: 50 * 1024 * 1024 }
);
const tokResult = JSON.parse(tokOut);
console.log(`  ${tokResult.entries?.length || 0} entries`);

// 3) Compare
console.log("\n=== COMPARAISON ===");
for (const tokEntry of (tokResult.entries || []).slice(0, 5)) {
  const tokSid = tokEntry.sessionId;
  const tokInput = tokEntry.input ?? tokEntry.inputTokens ?? 0;
  const tokOutput = tokEntry.output ?? tokEntry.outputTokens ?? 0;

  // cherche le match dans l'estimateur
  const estMatch = estSessions.find(
    (s) => s.account === ACCOUNT && s.sessionId === tokSid
  );

  console.log(
    `  tokscale: ${tokSid}  input=${tokInput}  output=${tokOutput}`
  );
  if (estMatch) {
    console.log(
      `  est:      ${estMatch.sessionId}  input=${estMatch.input}  output=${estMatch.output}`
    );
  }
}

// 4) Nettoyage
execSync(`rm -rf ${TMP}`);