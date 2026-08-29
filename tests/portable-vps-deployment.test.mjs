import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const dockerfile = readFileSync(new URL("../Dockerfile", import.meta.url), "utf8");
const compose = readFileSync(new URL("../compose.yaml", import.meta.url), "utf8");
const playbook = readFileSync(
  new URL("../deploy/ansible/playbook.yml", import.meta.url),
  "utf8",
);
const composeTemplate = readFileSync(
  new URL("../deploy/ansible/templates/compose.yaml.j2", import.meta.url),
  "utf8",
);
const duelloCompose = readFileSync(
  new URL("../deploy/duello/compose.yaml", import.meta.url),
  "utf8",
);
const envTemplate = readFileSync(
  new URL("../deploy/ansible/templates/cst.env.j2", import.meta.url),
  "utf8",
);
const entrypoint = readFileSync(
  new URL("../deploy/docker-entrypoint.sh", import.meta.url),
  "utf8",
);
const wrapper = readFileSync(
  new URL("../scripts/deploy-vps-ansible.ps1", import.meta.url),
  "utf8",
);
const githubUpdater = readFileSync(
  new URL("../scripts/update-vps-from-github.ps1", import.meta.url),
  "utf8",
);
const workflow = readFileSync(
  new URL("../.github/workflows/container.yml", import.meta.url),
  "utf8",
);
const platform = readFileSync(new URL("../src/platform.ts", import.meta.url), "utf8");
const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const packageJson = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));

test("l'image contient le frontend, le serveur et les outils de travail", () => {
  assert.match(dockerfile, /FROM node:22-bookworm-slim AS frontend-build/);
  assert.match(dockerfile, /FROM rust:1\.88\.0-bookworm AS server-build/);
  assert.match(dockerfile, /--profile server/);
  assert.match(
    dockerfile,
    /npm install --global --prefix \/home\/cst\/\.local @openai\/codex@0\.150\.1 @anthropic-ai\/claude-code(?:@[\d.]+)? opencode-ai freebuff/,
  );
  assert.match(dockerfile, /codex --version/);
  assert.match(dockerfile, /claude --version/);
  // Sans OpenCode, se connecter a un compte Z.ai / MiniMax / DeepSeek /
  // OpenRouter ouvrait un terminal bloque sur « command not found ».
  assert.match(dockerfile, /opencode --version/);
  assert.match(entrypoint, /exec \/usr\/sbin\/gosu cst "\$@"/);
  assert.match(entrypoint, /install -d -m 0700 -o cst -g cst \/srv\/cst/);
  assert.match(
    entrypoint,
    /find \/srv\/cst\/workspaces -mindepth 1 -maxdepth 1 -type d -exec chown cst:cst \{\} \+/,
  );
  assert.match(entrypoint, /chmod -R go-rwx \/srv\/cst/);
  assert.match(entrypoint, /chmod 0600 "\$ownership_marker"/);
  assert.match(dockerfile, /python3/);
  assert.match(dockerfile, /chromium/);
  assert.match(dockerfile, /ripgrep/);
  assert.match(dockerfile, /CST_WORKSPACES_ROOT=\/srv\/cst\/workspaces/);
});

// `opencode auth login` telecharge le catalogue models.dev puis installe ~60 Mo
// de node_modules avant d'afficher son invite : ~8 minutes de terminal muet sur
// le VPS. Le runtime est mutualise entre les comptes et pre-chauffe au build,
// sinon chaque nouveau compte repaye ce silence.
test("le runtime OpenCode est mutualise et pre-chauffe dans l'image", () => {
  assert.match(dockerfile, /ENV CST_OPENCODE_RUNTIME_DIR=\/home\/cst\/\.cst-opencode-runtime/);
  // `auth login` attend une cle sur un TTY et affiche son invite AVANT la fin de
  // l'installation du plugin : il ne peut pas servir de pre-chauffage. Les deux
  // etapes sont donc lancees explicitement, avec une fin exploitable.
  assert.match(dockerfile, /opencode models >\/dev\/null/);
  assert.match(dockerfile, /@opencode-ai\/plugin.*opencode --version/s);
  assert.match(dockerfile, /npm install --prefix "\$\{OPENCODE_CONFIG_DIR\}"/);
  // Le pre-chauffage doit faire echouer le build s'il n'a pas abouti, sinon
  // l'image partirait froide sans que personne ne le voie.
  assert.match(
    dockerfile,
    /test -s "\$\{CST_OPENCODE_RUNTIME_DIR\}\/cache\/opencode\/models\.json"/,
  );
  assert.match(
    dockerfile,
    /test -d "\$\{CST_OPENCODE_RUNTIME_DIR\}\/config\/opencode\/node_modules\/@opencode-ai\/plugin"/,
  );
  // Le home jetable du pre-chauffage ne doit laisser aucun credential.
  assert.match(dockerfile, /XDG_DATA_HOME=\/tmp\/opencode-warmup\/data/);
  assert.match(dockerfile, /rm -rf \/tmp\/opencode-warmup/);
});

test("Compose garde le runtime prive et les donnees hors de l'image", () => {
  // La pile de production Azure peut utiliser un port d'écoute différent de la pile locale.

  assert.match(compose, /127\.0\.0\.1:\$\{CST_HOST_PORT:-\d+\}:\d+/);
  assert.match(compose, /:\/srv\/cst/);
  assert.match(compose, /no-new-privileges:true/);
  assert.match(composeTemplate, /127\.0\.0\.1:\{\{ cst_remote_port \}\}:\d+/);
  assert.match(
    composeTemplate,
    /volumes:[\s\S]*?- type: bind[\s\S]*?source: \/srv\/cst[\s\S]*?target: \/srv\/cst[\s\S]*?propagation: rslave/,
  );
  assert.match(envTemplate, /CST_BIND="0\.0\.0\.0:8080"/);
});

test("Ansible installe Docker officiellement puis valide la sante", () => {
  assert.match(playbook, /download\.docker\.com\/linux\/\{\{ ansible_distribution \| lower \}\}/);
  assert.match(playbook, /docker-compose-plugin/);
  assert.match(playbook, /docker\s*\n\s*- compose/);
  assert.match(playbook, /--wait-timeout/);
  assert.match(playbook, /http:\/\/127\.0\.0\.1:\{\{ cst_remote_port \}\}\/healthz/);
  assert.match(playbook, /not cst_existing_settings\.stat\.exists/);
  assert.match(playbook, /no_log: true/);
  assert.match(
    playbook,
    /path: "\/srv\/cst\/workspaces\/\{\{ cst_workspace_name \}\}"[\s\S]*?owner: "10001"[\s\S]*?group: "10001"[\s\S]*?mode: "0700"/,
  );
});

test("les rebuilds CST laissent les services externes comme Duello intacts", () => {
  assert.match(
    playbook,
    /label=com\.codex-switch-terminal\.rebuild-policy=exclude/,
  );
  assert.match(playbook, /cst_protected_containers_before/);
  assert.match(playbook, /cst_protected_containers_after/);
  assert.match(playbook, /com\.docker\.compose\.project=codex-switch-terminal/);
  assert.doesNotMatch(playbook, /--remove-orphans/);

  assert.match(duelloCompose, /^name: duello$/m);
  assert.match(duelloCompose, /container_name: duello-server/);
  assert.match(
    duelloCompose,
    /com\.codex-switch-terminal\.rebuild-policy: exclude/,
  );
  assert.match(duelloCompose, /restart: unless-stopped/);
  assert.match(duelloCompose, /OPENAI_ACCOUNTS_DIR: \/srv\/cst\/codex-homes/);
  assert.match(duelloCompose, /http:\/\/127\.0\.0\.1:8787\/health\)" = 401/);
  assert.doesNotMatch(duelloCompose, /^\s+ports:/m);
});

test("le wrapper protege les secrets et cree un profil reutilisable", () => {
  assert.match(wrapper, /ConvertFrom-SecureString -SecureString \$secure/);
  assert.match(wrapper, /tokenProtected = \(Protect-Secret \$adminToken\)/);
  assert.match(wrapper, /deploymentMode = "ansible-compose"/);
  assert.match(wrapper, /run-ansible-deploy\.sh/);
  assert.match(wrapper, /workspace-seed\.tar\.gz/);
  assert.match(wrapper, /autonomous-agents\.json/);
  assert.match(wrapper, /Convert-AutonomousAgentStoreForVps/);
  assert.match(wrapper, /Convert-SettingsForVps/);
  assert.match(wrapper, /\$settings\.shell = "\/bin\/bash"/);
  assert.match(wrapper, /\$settings\.workspaces = @\(\$portableWorkspace\)/);
  assert.match(wrapper, /\$settings\.closedWorkspaceIds = @\(\)/);
  assert.match(wrapper, /\/srv\/cst\/workspaces\/\$WorkspaceName/);
  assert.match(wrapper, /--exclude=codex-homes\/\*\/logs_\*\.sqlite\*/);
  assert.match(wrapper, /--exclude=codex-homes\/\*\/\.sandbox-secrets/);
  assert.match(wrapper, /agent\(s\) autonome\(s\) utilisent un projet hors du workspace transfere/);
  assert.match(wrapper, /capacity = \$Capacity/);
  assert.match(wrapper, /cst_public_base_url = \$PublicBaseUrl/);
  assert.match(wrapper, /publicBaseUrl = \$PublicBaseUrl/);
  assert.match(wrapper, /existingProfile\.publicBaseUrl/);
  assert.match(envTemplate, /CST_PUBLIC_BASE_URL=\{\{ cst_public_base_url \| to_json \}\}/);
  assert.match(envTemplate, /CST_ALLOWED_ORIGINS=\{\{ cst_public_base_url \| to_json \}\}/);
  assert.doesNotMatch(wrapper, /Write-Host[^\n]*\$adminToken/);
  assert.match(packageJson.scripts["deploy:vps:portable"], /deploy-vps-ansible\.ps1/);
  assert.match(packageJson.scripts["update:vps:github"], /update-vps-from-github\.ps1/);
});

test("un profil VPS peut etre mis a jour depuis une revision GitHub exacte", () => {
  assert.match(githubUpdater, /git[\s\S]*ls-remote/);
  assert.match(githubUpdater, /checkout[\s\S]*--detach[\s\S]*\$targetRevision/);
  assert.match(githubUpdater, /blocked_unpublished_changes/);
  assert.match(githubUpdater, /ReplaceDirtyDeployment/);
  assert.match(githubUpdater, /SkipAccountSeed = \$true/);
  assert.match(githubUpdater, /NoWorkspaceSeed = \$true/);
  assert.match(githubUpdater, /Invoke-SshJson[\s\S]*healthz/);
  assert.match(githubUpdater, /Assert-HealthyNode -Health \$afterHealth/);
  assert.match(githubUpdater, /type = "github"/);
  assert.match(githubUpdater, /commit = \$targetRevision/);
});

test("la CI publie une image unique pour AMD64 et ARM64", () => {
  assert.match(workflow, /packages: write/);
  assert.match(workflow, /docker\/build-push-action@v7/);
  assert.match(workflow, /platforms: linux\/amd64,linux\/arm64/);
  assert.match(workflow, /ghcr\.io\/\$\{\{ github\.repository \}\}/);
});

test("un nouveau chat peut imposer Oracle sans casser la session sticky", () => {
  assert.match(platform, /remoteChatExecutionTargets/);
  assert.match(platform, /targetNodeId/);
  assert.match(platform, /candidates = candidates\.filter/);
  assert.match(platform, /if \(sessionId\)[\s\S]*candidates = \[sticky\]/);
  assert.match(main, /id="newChatExecutionTarget"/);
  assert.match(main, /Machine d.ex.cution/);
  assert.match(main, /targetNodeId: resumeSessionId \? null : pane\.executionTargetId/);
  assert.match(main, /executionTargetId: pane\.executionTargetId/);
});

test("chaque environnement peut fixer son VPS pour les chats et terminaux", () => {
  assert.match(main, /data-environment-execution-target-id/);
  assert.match(main, /setWorkspaceExecutionTarget/);
  assert.match(main, /workspaceExecutionTargetIdForPath\(environmentPath\)/);
  assert.match(main, /targetNodeId: isRemoteMode\(\) \? workspaceExecutionTargetIdForPath\(folder\)/);
  assert.match(platform, /terminalNodeCandidates\(targetNodeId\)/);
  assert.match(platform, /remoteRouteKey\(route\) === targetNodeId/);
});
