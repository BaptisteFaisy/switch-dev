import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const types = readFileSync(new URL("../src/types.ts", import.meta.url), "utf8");
const panel = readFileSync(new URL("../src/vps.ts", import.meta.url), "utf8");
const styles = readFileSync(new URL("../src/vps.css", import.meta.url), "utf8");
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
const server = readFileSync(new URL("../src-tauri/src/server.rs", import.meta.url), "utf8");
const manager = readFileSync(new URL("../src-tauri/src/vps_deploy.rs", import.meta.url), "utf8");

test("le site :8080 expose un onglet VPS sur bureau et mobile", () => {
  assert.match(types, /\| "vps"/);
  assert.match(main, /id="vpsToggle"/);
  assert.match(main, /data-view="vps"/);
  assert.match(main, /type VpsModule = typeof import\("\.\/vps"\)/);
  assert.match(main, /vpsModulePromise = import\("\.\/vps"\)/);
  assert.match(main, /if \(view === "vps" && !vpsModule\)/);
  assert.match(main, /case "vps":\s*return vpsModule\?\.renderVpsPanel\(\) \?\? ""/);
  assert.doesNotMatch(main, /from "\.\/vps";/);
  assert.match(main, /isRemoteMode\(\) \? `<button[^`]+vpsToggle/);
  assert.match(main, /vpsModule\?\.activateVpsPanel\(render\)/);
  assert.match(panel, /import "\.\/vps\.css";/);
});

test("l'onglet utilise des routes dediees et suit les deploiements", () => {
  assert.match(platform, /case "vps_deploy_capabilities":\s*return api<T>\("GET", "\/api\/vps\/capabilities"\)/);
  assert.match(platform, /case "vps_list_deployments":\s*return api<T>\("GET", "\/api\/vps\/deployments"\)/);
  assert.match(platform, /case "vps_start_deployment":\s*return api<T>\("POST", "\/api\/vps\/deployments", args\.request\)/);
  assert.match(panel, /window\.setTimeout\(\(\) => void refreshVpsPanel\(rerender, true\), 1_500\)/);
  assert.match(panel, /Déploiements récents/);
});

test("les operations VPS sont reservees au jeton de maintenance", () => {
  assert.match(server, /"\/vps\/capabilities", get\(api_vps_capabilities\)/);
  assert.match(server, /"\/vps\/deployments",\s*get\(api_vps_deployments\)\.post\(api_vps_start_deployment\)/);
  for (const handler of [
    "api_vps_capabilities",
    "api_vps_deployments",
    "api_vps_deployment",
    "api_vps_start_deployment",
  ]) {
    const start = server.indexOf(`async fn ${handler}`);
    assert.notEqual(start, -1, `${handler} absent`);
    const body = server.slice(start, start + 700);
    assert.match(body, /check_maintenance_header\(&state, &headers\)/);
  }
});

test("le panneau VPS reste strictement Azure et sans provisionnement Google", () => {
  assert.match(panel, /Déployer sur Azure/);
  assert.match(panel, /Connexion SSH Azure/);
  assert.doesNotMatch(panel, /vpsGoogle|Google Cloud|provisionneur Google/i);
  assert.doesNotMatch(server, /api_vps_google_status|api_vps_google_auth|api_vps_google_trial|api_vps_google_start_deployment|StartGoogleCloudDeployRequest/);
  assert.doesNotMatch(manager, /google-trial|Google Cloud|locate_google/i);
});

test("le backend valide les champs et lance PowerShell sans shell intermediaire", () => {
  assert.match(manager, /validate_ssh_target\(&ssh_target\)/);
  assert.match(manager, /value\.matches\('@'\)\.count\(\) != 1/);
  assert.match(manager, /let mut command = Command::new\(executable\)/);
  assert.match(manager, /\.arg\("-SshTarget"\)\s*\.arg\(&request\.ssh_target\)/);
  assert.doesNotMatch(manager, /cmd\.exe|\/bin\/sh|-Command/);
  assert.match(manager, /Un deploiement VPS est deja en cours/);
  assert.match(manager, /MAX_LOG_BYTES/);
  assert.match(manager, /redact_sensitive_output/);
});

test("aucune cle privee ne transite dans le navigateur", () => {
  assert.match(panel, /Chemin de la clé privée/);
  assert.match(panel, /La clé reste sur la machine/);
  assert.match(manager, /canonical_file\(&request\.identity_file/);
  assert.doesNotMatch(panel, /privateKey(Content|Base64)|readAsText\(/);
  assert.doesNotMatch(server, /private_key_content|privateKeyContent/);
});

test("un compte web peut deverrouiller la vue avec le jeton admin", () => {
  assert.match(panel, /id="vpsAdminUnlock"/);
  assert.match(panel, /hasRemoteAuth\(\)/);
  assert.match(panel, /saveRemoteConfig\(remoteBaseUrl\(\), token, remoteNodesText\(\)\)/);
  assert.match(panel, /type="password" autocomplete="current-password"/);
});

test("la vue VPS s'adapte aux ecrans mobiles", () => {
  assert.match(styles, /@media \(max-width: 680px\)/);
  assert.match(styles, /\.vps-layout \{ grid-template-columns: 1fr; \}/);
  assert.match(styles, /\.vps-panel \{ padding: 18px 14px 94px; \}/);
});

test("la vue VPS affiche l'essentiel et garde les details a la demande", () => {
  assert.match(panel, /let showAdvancedOptions = false/);
  assert.doesNotMatch(panel, /Prêt à déployer/);
  assert.match(panel, /id="vpsDetailsToggle"/);
  assert.match(panel, /aria-controls="vpsAdvancedSettings"/);
  assert.match(panel, /showAdvancedOptions = !showAdvancedOptions/);
  assert.match(panel, /id="vpsHistoryToggle"/);
  assert.match(styles, /\.vps-advanced-settings\[hidden\]\s*\{\s*display: none;/);
  assert.doesNotMatch(panel, /<details[^>]*open[^>]*>\s*<summary>Journal technique/);
});

test("la vue VPS utilise une palette strictement monochrome", () => {
  assert.match(styles, /--vps-bg: #050505/);
  assert.match(styles, /--vps-text: #f5f5f5/);
  assert.match(styles, /\.vps-submit\s*\{[^}]*background: var\(--vps-text\)/s);
  assert.doesNotMatch(styles, /#527bff|#6659e8|#1cb98b|#f0a33c|#e34d61|linear-gradient/);
});
