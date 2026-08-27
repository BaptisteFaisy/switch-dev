import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { promisify } from "node:util";
import test from "node:test";
import {
  analyzeProject,
  prepareBuildFiles,
  renderRunScript,
  validateImageReference,
} from "../public/skills/dockerize-git/scripts/docker-project.mjs";
import {
  parseArguments,
  validateRepository,
} from "../public/skills/dockerize-git/scripts/dockerize-git.mjs";

const execFileAsync = promisify(execFile);
const cli = resolve("public/skills/dockerize-git/scripts/dockerize-git.mjs");

const fixture = async (files, callback) => {
  const root = await mkdtemp(join(tmpdir(), "cst-dockerize-test-"));
  try {
    for (const [name, content] of Object.entries(files)) {
      const path = join(root, name);
      await mkdir(dirname(path), { recursive: true });
      await writeFile(path, content, "utf8");
    }
    return await callback(root);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
};

test("parse la commande à lien unique et valide les options de déploiement", () => {
  const parsed = parseArguments([
    "https://github.com/acme/app.git",
    "--context",
    "apps/web",
    "--deploy",
    "ubuntu@203.0.113.10",
    "--container-port=3000",
    "--host-port",
    "80",
    "--no-export",
  ]);
  assert.equal(parsed.repository, "https://github.com/acme/app.git");
  assert.equal(parsed.context, "apps/web");
  assert.equal(parsed.deploy, "ubuntu@203.0.113.10");
  assert.equal(parsed.containerPort, 3000);
  assert.equal(parsed.hostPort, 80);
  assert.equal(parsed.exportImage, false);
});

test("refuse les secrets Git en ligne et les chemins qui sortent du dépôt", () => {
  assert.throws(
    () => validateRepository("https://token@github.com/acme/private.git"),
    /Do not put credentials/,
  );
  assert.throws(
    () => validateRepository("https://github.com/acme/private.git?token=secret"),
    /query strings/,
  );
  assert.throws(
    () => parseArguments(["https://github.com/acme/app.git", "--context", "../secret"]),
    /inside the repository/,
  );
  assert.throws(
    () => validateImageReference("UPPER/project:latest"),
    /lowercase repository/,
  );
});

test("détecte Vite et génère un conteneur statique sans secrets", async () => {
  await fixture({
    "package.json": JSON.stringify({
      scripts: { build: "vite build" },
      devDependencies: { vite: "^6.0.0" },
    }),
    "package-lock.json": "{}",
    "src/main.js": "document.body.textContent = 'ok';",
    ".env": "SECRET=never-copy",
    ".env.example": "PUBLIC=value\nAPI_BASE_URL=\n",
  }, async (root) => {
    const analysis = await analyzeProject(root);
    assert.equal(analysis.kind, "node-static");
    assert.equal(analysis.framework, "Vite");
    assert.equal(analysis.port, 80);
    assert.deepEqual(analysis.environmentVariables, ["API_BASE_URL", "PUBLIC"]);

    const files = await prepareBuildFiles(root, analysis);
    const dockerfile = await readFile(files.dockerfilePath, "utf8");
    const ignore = await readFile(files.ignorePath, "utf8");
    const nginx = await readFile(files.nginxPath, "utf8");
    assert.match(dockerfile, /FROM node:22-bookworm-slim AS build/);
    assert.match(dockerfile, /COPY --from=build \/app\/dist/);
    assert.match(ignore, /^\.env$/m);
    assert.match(ignore, /^!\.env\.example$/m);
    assert.match(ignore, /^!\.cst-docker$/m);
    assert.match(nginx, /listen 80;/);
  });
});

test("installe les dépendances Node de build avant d'activer NODE_ENV production", async () => {
  await fixture({
    "package.json": JSON.stringify({
      scripts: { build: "next build", start: "next start" },
      dependencies: { next: "latest" },
      devDependencies: { typescript: "latest" },
      engines: { node: ">=24" },
      packageManager: "npm@12.x",
    }),
    "package-lock.json": "{}",
  }, async (root) => {
    const analysis = await analyzeProject(root);
    const files = await prepareBuildFiles(root, analysis);
    const dockerfile = await readFile(files.dockerfilePath, "utf8");
    const install = dockerfile.indexOf("npm ci");
    const production = dockerfile.indexOf("ENV NODE_ENV=production");
    assert.match(dockerfile, /FROM node:24-bookworm-slim/);
    assert.match(dockerfile, /npm install --global \\"npm@12\.x\\"/);
    assert.ok(install >= 0 && production > install);
  });
});

test("propage un port statique explicite jusqu'à Nginx et au healthcheck", async () => {
  await fixture({ "index.html": "<!doctype html><title>portable</title>" }, async (root) => {
    const analysis = await analyzeProject(root, { containerPort: 8088 });
    const files = await prepareBuildFiles(root, analysis);
    const dockerfile = await readFile(files.dockerfilePath, "utf8");
    const nginx = await readFile(files.nginxPath, "utf8");
    assert.match(nginx, /listen 8088;/);
    assert.match(dockerfile, /127\.0\.0\.1:8088\/healthz/);
    assert.match(dockerfile, /EXPOSE 8088/);
    assert.match(dockerfile, /RUN rm -rf \.cst-docker/);
    assert.doesNotMatch(dockerfile, /COPY \. \/usr\/share\/nginx\/html/);
  });
});

test("réutilise exactement le Dockerfile du dépôt et détecte EXPOSE", async () => {
  const upstream = "FROM node:22-alpine\nEXPOSE 4321\nCMD [\"node\", \"server.js\"]\n";
  await fixture({
    Dockerfile: upstream,
    "server.js": "console.log('ok')",
    ".dockerignore": "coverage\n",
  }, async (root) => {
    const analysis = await analyzeProject(root);
    assert.equal(analysis.generated, false);
    assert.equal(analysis.port, 4321);
    const files = await prepareBuildFiles(root, analysis);
    assert.equal(await readFile(files.dockerfilePath, "utf8"), upstream);
    assert.match(await readFile(files.ignorePath, "utf8"), /^coverage$/m);
  });
});

test("détecte FastAPI et crée une commande liée à toutes les interfaces", async () => {
  await fixture({
    "requirements.txt": "fastapi\n",
    "api/main.py": "from fastapi import FastAPI\napp = FastAPI()\n",
  }, async (root) => {
    const analysis = await analyzeProject(root);
    assert.equal(analysis.framework, "FastAPI");
    assert.match(analysis.startCommand, /api\.main:app --host 0\.0\.0\.0/);
    const files = await prepareBuildFiles(root, analysis);
    assert.match(await readFile(files.dockerfilePath, "utf8"), /python -m uvicorn/);
  });
});

test("génère les images compilées Go, Rust et Java avec un runtime séparé", async (context) => {
  await context.test("Go", async () => fixture({
    "go.mod": "module example.invalid/api\n\ngo 1.23\n",
    "main.go": "package main\nimport \"net/http\"\nfunc main(){ http.ListenAndServe(\":9090\", nil) }\n",
  }, async (root) => {
    const analysis = await analyzeProject(root);
    const files = await prepareBuildFiles(root, analysis);
    const dockerfile = await readFile(files.dockerfilePath, "utf8");
    assert.equal(analysis.port, 9090);
    assert.match(dockerfile, /FROM golang:1\.23-bookworm AS build/);
    assert.match(dockerfile, /CGO_ENABLED=0 go build/);
  }));

  await context.test("Rust", async () => fixture({
    "Cargo.toml": "[package]\nname = \"portable-api\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    "Cargo.lock": "# fixture\n",
    "src/main.rs": "fn main() {}\n",
  }, async (root) => {
    const analysis = await analyzeProject(root);
    const files = await prepareBuildFiles(root, analysis);
    const dockerfile = await readFile(files.dockerfilePath, "utf8");
    assert.equal(analysis.binary, "portable-api");
    assert.match(dockerfile, /cargo build --release --locked --bin portable-api/);
    assert.match(dockerfile, /FROM debian:bookworm-slim AS runtime/);
  }));

  await context.test("Maven", async () => fixture({
    "pom.xml": "<project><modelVersion>4.0.0</modelVersion></project>\n",
    "src/main/java/App.java": "class App { public static void main(String[] args) {} }\n",
  }, async (root) => {
    const analysis = await analyzeProject(root);
    const files = await prepareBuildFiles(root, analysis);
    const dockerfile = await readFile(files.dockerfilePath, "utf8");
    assert.equal(analysis.framework, "Java / Maven");
    assert.match(dockerfile, /mvn -B -DskipTests package/);
    assert.match(dockerfile, /ENTRYPOINT \["java", "-jar", "\/app\.jar"\]/);
  }));
});

test("arrête l'analyse d'un monorepo ambigu au lieu de choisir au hasard", async () => {
  await fixture({
    "package.json": JSON.stringify({ scripts: { start: "node index.js" } }),
    "requirements.txt": "flask\n",
    "index.js": "console.log('node')",
    "app.py": "from flask import Flask\napp = Flask(__name__)\n",
  }, async (root) => {
    await assert.rejects(() => analyzeProject(root), /Several runnable stacks/);
  });
});

test("le script portable développe HOST_PORT au moment du lancement", () => {
  const script = renderRunScript({
    image: "cst/demo:abc",
    containerName: "demo",
    containerPort: 3000,
    hostPort: 8080,
  });
  assert.match(script, /-p "\$\{HOST_PORT:-8080\}:3000"/);
  assert.doesNotMatch(script, /-p '\$\{HOST_PORT/);
});

test("le CLI réalise un dry run complet sur un dépôt Git local sans Docker", async () => {
  const root = await mkdtemp(join(tmpdir(), "cst-dockerize-cli-test-"));
  try {
    const repository = join(root, "sample-api");
    const output = join(root, "bundle");
    await mkdir(repository);
    await writeFile(join(repository, "package.json"), JSON.stringify({
      scripts: { start: "node server.js" },
    }), "utf8");
    await writeFile(
      join(repository, "server.js"),
      "require('http').createServer((_req, res) => res.end('ok')).listen(process.env.PORT || 3456);\n",
      "utf8",
    );
    await execFileAsync("git", ["init", "-b", "main"], { cwd: repository });
    await execFileAsync("git", ["config", "user.email", "test@example.invalid"], { cwd: repository });
    await execFileAsync("git", ["config", "user.name", "CST Test"], { cwd: repository });
    await execFileAsync("git", ["add", "."], { cwd: repository });
    await execFileAsync("git", ["commit", "-m", "fixture"], { cwd: repository });
    await mkdir(output);
    await writeFile(join(output, "image.tar.gz"), "stale", "utf8");
    await writeFile(join(output, "run.sh"), "stale", "utf8");

    const result = await execFileAsync(process.execPath, [cli, repository, "--dry-run", "--output", output]);
    assert.match(result.stdout, /Node\.js/);
    const manifest = JSON.parse(await readFile(join(output, "manifest.json"), "utf8"));
    assert.equal(manifest.build.mode, "dry-run");
    assert.equal(manifest.analysis.port, 3456);
    assert.equal(manifest.image.archive, null);
    assert.equal(manifest.run.archive, null);
    assert.equal(manifest.build.dockerfile, "Dockerfile");
    assert.equal(manifest.image.status, "planned");
    assert.equal((await stat(join(output, "Dockerfile"))).isFile(), true);
    await assert.rejects(() => stat(join(output, "image.tar.gz")), /ENOENT/);
    await assert.rejects(() => stat(join(output, "run.sh")), /ENOENT/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("le flux de build/export produit un bundle avec un moteur Docker simulé", async () => {
  const root = await mkdtemp(join(tmpdir(), "cst-dockerize-build-test-"));
  try {
    const repository = join(root, "static-site");
    const output = join(root, "bundle");
    const bin = join(root, "bin");
    await mkdir(repository);
    await mkdir(bin);
    await writeFile(join(repository, "index.html"), "<!doctype html><h1>ok</h1>", "utf8");
    await execFileAsync("git", ["init", "-b", "main"], { cwd: repository });
    await execFileAsync("git", ["config", "user.email", "test@example.invalid"], { cwd: repository });
    await execFileAsync("git", ["config", "user.name", "CST Test"], { cwd: repository });
    await execFileAsync("git", ["add", "."], { cwd: repository });
    await execFileAsync("git", ["commit", "-m", "fixture"], { cwd: repository });

    const fakeDocker = join(bin, "fake-docker.mjs");
    await writeFile(
      fakeDocker,
      "const args = process.argv.slice(2);\nif (args[0] === 'image' && args[1] === 'save') process.stdout.write('fake-docker-image');\n",
      "utf8",
    );
    if (process.platform !== "win32") await chmod(fakeDocker, 0o755);

    const env = { ...process.env };
    env.CST_DOCKER_COMMAND = process.execPath;
    env.CST_DOCKER_COMMAND_ARGS = JSON.stringify([fakeDocker]);
    await execFileAsync(process.execPath, [cli, repository, "--output", output], { env });
    const manifest = JSON.parse(await readFile(join(output, "manifest.json"), "utf8"));
    assert.equal(manifest.build.mode, "local");
    assert.equal(manifest.image.status, "built");
    assert.equal(typeof manifest.completedAt, "string");
    assert.ok(manifest.image.archiveBytes > 0);
    assert.match(manifest.image.archiveSha256, /^[a-f0-9]{64}$/);
    assert.ok((await stat(join(output, "image.tar.gz"))).size > 0);
    assert.equal((await stat(join(output, "run.sh"))).isFile(), true);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("le paquet skill standard et le manifeste intégré restent cohérents", async () => {
  const skill = (await readFile(resolve("public/skills/dockerize-git/SKILL.md"), "utf8"))
    .replace(/\r\n/g, "\n");
  const metadata = await readFile(resolve("public/skills/dockerize-git/agents/openai.yaml"), "utf8");
  const index = JSON.parse(await readFile(resolve("public/skills/index.json"), "utf8"));
  const tauri = JSON.parse(await readFile(resolve("src-tauri/tauri.conf.json"), "utf8"));
  const desktop = await readFile(resolve("src-tauri/src/lib.rs"), "utf8");
  assert.match(skill, /^---\nname: dockerize-git\ndescription: .+\n---\n/);
  assert.match(metadata, /default_prompt: "Use \$dockerize-git /);
  const entry = index.skills.find((candidate) => candidate.id === "dockerize-git");
  assert.equal(entry.file, "dockerize-git/SKILL.md");
  assert.equal(entry.buttonLabel, "Docker Git");
  assert.equal(
    tauri.bundle.resources["../public/skills/dockerize-git"],
    "skills/dockerize-git",
  );
  assert.match(desktop, /CST_DOCKERIZE_GIT_SCRIPT/);
});
