import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { dirname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const scriptPath = fileURLToPath(import.meta.url);
const projectRoot = resolve(dirname(scriptPath), "..");
const defaultSiteUrl = "http://127.0.0.1:18080";

const boundedTimeout = (value) => {
  const parsed = Number.parseInt(String(value ?? ""), 10);
  return Number.isFinite(parsed) ? Math.min(60_000, Math.max(1_000, parsed)) : 10_000;
};

export const sha256 = (value) => createHash("sha256").update(value).digest("hex");

export const redactGitDiagnostic = (value) => String(value ?? "")
  .replace(/(https?:\/\/)[^\s/@:]+(?::[^\s/@]*)?@/gi, "$1[identifiants masques]@")
  .replace(/\b(?:gh[pousr]_|github_pat_)[a-z0-9_]+/gi, "[secret masque]")
  .replace(/((?:token|password|secret|api[_-]?key)\s*[=:]\s*)\S+/gi, "$1[secret masque]");

export const extractLocalAssetUrls = (html, indexUrl) => {
  const origin = new URL(indexUrl).origin;
  const assets = new Map();
  const references = html.matchAll(/\b(?:src|href)\s*=\s*["']([^"'#]+)["']/gi);
  for (const match of references) {
    const reference = match[1]?.trim();
    if (!reference || /^(?:data:|mailto:|tel:|javascript:)/i.test(reference)) continue;
    let url;
    try {
      url = new URL(reference, indexUrl);
    } catch {
      continue;
    }
    if (url.origin !== origin || !/^https?:$/.test(url.protocol)) continue;
    url.hash = "";
    assets.set(url.href, url);
  }
  return [...assets.values()];
};

const git = (...args) => {
  const result = spawnSync("git", ["-C", projectRoot, ...args], {
    cwd: projectRoot,
    encoding: "utf8",
    timeout: 20_000,
    windowsHide: true,
    env: {
      ...process.env,
      GIT_TERMINAL_PROMPT: "0",
      GCM_INTERACTIVE: "Never",
      GIT_OPTIONAL_LOCKS: "0",
    },
  });
  if (result.error) {
    throw new Error(`git ${args[0]} impossible: ${result.error.message}`);
  }
  if (result.status !== 0) {
    const detail = redactGitDiagnostic(result.stderr || result.stdout).trim().slice(0, 500);
    throw new Error(`git ${args[0]} a echoue${detail ? `: ${detail}` : ""}`);
  }
  return String(result.stdout || "").trim();
};

const fetchBytes = async (url, timeoutMs, label) => {
  const response = await fetch(url, {
    cache: "no-store",
    redirect: "error",
    signal: AbortSignal.timeout(timeoutMs),
    headers: { "cache-control": "no-cache" },
  });
  if (!response.ok) throw new Error(`${label}: HTTP ${response.status}`);
  return { response, bytes: Buffer.from(await response.arrayBuffer()) };
};

export const verifyPublishedBuild = async ({
  siteUrl = process.env.CST_PUBLISH_SITE_URL || defaultSiteUrl,
  timeoutMs = boundedTimeout(process.env.CST_PUBLISH_VERIFY_TIMEOUT_MS),
} = {}) => {
  const dirty = git("status", "--porcelain=v1", "--untracked-files=all");
  if (dirty) {
    const count = dirty.split(/\r?\n/).filter(Boolean).length;
    throw new Error(`Le depot contient encore ${count} changement(s) non publie(s).`);
  }

  const branch = git("symbolic-ref", "--quiet", "--short", "HEAD");
  if (!branch) throw new Error("La verification exige une branche Git active.");
  git("remote", "get-url", "origin");
  const localCommit = git("rev-parse", "HEAD");
  const remoteLine = git("ls-remote", "--exit-code", "origin", `refs/heads/${branch}`)
    .split(/\s+/)[0];
  if (remoteLine !== localCommit) {
    throw new Error(`Le commit local ${localCommit.slice(0, 12)} n'est pas actif sur origin/${branch}.`);
  }

  const baseUrl = new URL(siteUrl);
  if (!/^https?:$/.test(baseUrl.protocol)) {
    throw new Error("CST_PUBLISH_SITE_URL doit utiliser HTTP ou HTTPS.");
  }
  const healthUrl = new URL("/healthz", baseUrl);
  const healthResponse = await fetch(healthUrl, {
    cache: "no-store",
    redirect: "error",
    signal: AbortSignal.timeout(timeoutMs),
    headers: { accept: "application/json", "cache-control": "no-cache" },
  });
  if (!healthResponse.ok) throw new Error(`healthz: HTTP ${healthResponse.status}`);
  const health = await healthResponse.json();
  if (health?.ok !== true || health?.ready === false) {
    throw new Error("Le noeud web repond mais n'est pas pret a servir le frontend.");
  }

  const distDir = resolve(projectRoot, "dist");
  const localIndex = await readFile(resolve(distDir, "index.html"));
  const indexUrl = new URL("/", baseUrl);
  indexUrl.searchParams.set("cst-publish-verify", localCommit.slice(0, 12));
  const { bytes: servedIndex } = await fetchBytes(indexUrl, timeoutMs, "index.html actif");
  const localIndexHash = sha256(localIndex);
  const servedIndexHash = sha256(servedIndex);
  if (localIndexHash !== servedIndexHash) {
    throw new Error("Le site actif ne sert pas le dist/index.html du commit valide.");
  }

  const assetUrls = extractLocalAssetUrls(localIndex.toString("utf8"), indexUrl);
  for (const assetUrl of assetUrls) {
    let relativePath;
    try {
      relativePath = decodeURIComponent(assetUrl.pathname).replace(/^\/+/, "");
    } catch {
      throw new Error(`Chemin d'asset invalide: ${assetUrl.pathname}`);
    }
    const localAssetPath = resolve(distDir, relativePath);
    if (!localAssetPath.startsWith(`${distDir}${sep}`)) {
      throw new Error(`Asset hors du dist refuse: ${assetUrl.pathname}`);
    }
    const localAsset = await readFile(localAssetPath);
    assetUrl.searchParams.set("cst-publish-verify", localCommit.slice(0, 12));
    const { bytes: servedAsset } = await fetchBytes(
      assetUrl,
      timeoutMs,
      `asset ${assetUrl.pathname}`,
    );
    if (sha256(localAsset) !== sha256(servedAsset)) {
      throw new Error(`Le contenu de l'asset actif differe du build: ${assetUrl.pathname}`);
    }
  }

  return {
    ok: true,
    branch,
    commit: localCommit,
    site: baseUrl.origin,
    indexSha256: localIndexHash,
    assetsChecked: assetUrls.length,
  };
};

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  try {
    const result = await verifyPublishedBuild();
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } catch (error) {
    process.stderr.write(`Verification de publication echouee: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}
