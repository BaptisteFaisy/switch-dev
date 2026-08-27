import { createHash } from "node:crypto";
import { existsSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import { createConnection } from "node:net";
import { chromium } from "playwright-core";

const ACCOUNT_BROWSER_PIPE = "\\\\.\\pipe\\CodexSwitchAccountBrowser";
const activeBrowserContexts = new Set();

const chromeCandidates = [
  process.env.CST_CHROME_PATH,
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
  `${process.env.LOCALAPPDATA || ""}/Google/Chrome/Application/chrome.exe`,
].filter(Boolean);

const executablePath = chromeCandidates.find(existsSync);

const decodeUrlPart = (value) => {
  try {
    return decodeURIComponent(value);
  } catch {
    throw new Error("Les identifiants du proxy sont mal encodes.");
  }
};

export const proxyOptionsFromUrl = (rawProxyUrl) => {
  const proxyUrl = new URL(String(rawProxyUrl || ""));
  if (!["http:", "https:", "socks5:"].includes(proxyUrl.protocol)) {
    throw new Error("Le proxy doit utiliser HTTP, HTTPS ou SOCKS5.");
  }
  if (!proxyUrl.hostname || !proxyUrl.port || proxyUrl.pathname !== "/" || proxyUrl.search || proxyUrl.hash) {
    throw new Error("L'URL du proxy doit contenir uniquement les identifiants, l'hote et le port.");
  }
  return {
    server: `${proxyUrl.protocol}//${proxyUrl.host}`,
    ...(proxyUrl.username ? { username: decodeUrlPart(proxyUrl.username) } : {}),
    ...(proxyUrl.password ? { password: decodeUrlPart(proxyUrl.password) } : {}),
  };
};

export const parseAccountBrowserRequest = (value) => {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Demande d'ouverture du navigateur invalide.");
  }
  const accountId = String(value.accountId || "").trim();
  const target = new URL(String(value.url || ""));
  if (
    !accountId ||
    target.protocol !== "https:" ||
    target.hostname !== "freebuff.com" ||
    target.pathname !== "/login" ||
    target.username ||
    target.password ||
    !/^[A-Za-z0-9_-]{8,512}$/.test(target.searchParams.get("auth_code") || "")
  ) {
    throw new Error("Seul un lien officiel de connexion Freebuff peut etre ouvert.");
  }
  const proxy = value.proxyUrl ? proxyOptionsFromUrl(value.proxyUrl) : null;
  return { accountId, url: target.toString(), proxy };
};

const readStdin = async () => {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
};

const writeResult = (value) => {
  process.stdout.write(`${JSON.stringify(value)}\n`);
};

export const openDefaultBrowser = (url) => {
  const child = spawn("rundll32.exe", ["url.dll,FileProtocolHandler", url], {
    detached: true,
    stdio: "ignore",
    windowsHide: true,
  });
  child.unref();
};

const proxyProfileDir = (accountId, proxy) => {
  const root = join(
    process.env.LOCALAPPDATA || join(homedir(), "AppData", "Local"),
    "CodexSwitchTerminal",
    "proxy-browser-profiles",
  );
  const identity = createHash("sha256")
    .update(accountId)
    .update("\0")
    .update(proxy.server)
    .update("\0")
    .update(proxy.username || "")
    .digest("hex")
    .slice(0, 24);
  const profile = join(root, identity);
  mkdirSync(profile, { recursive: true });
  return profile;
};

export const openProxiedBrowser = async ({ accountId, url, proxy }) => {
  if (!executablePath) throw new Error("Google Chrome est introuvable sur le poste Windows.");
  const context = await chromium.launchPersistentContext(proxyProfileDir(accountId, proxy), {
    executablePath,
    headless: false,
    proxy,
    locale: "fr-FR",
    viewport: null,
    acceptDownloads: false,
    args: [
      "--start-maximized",
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-sync",
      "--disable-background-networking",
      "--disable-component-update",
      "--disable-default-apps",
      "--disable-domain-reliability",
      "--disable-features=AutofillServerCommunication,PasswordManagerOnboarding,OptimizationHints,MediaRouter",
      "--disable-preconnect",
      "--dns-prefetch-disable",
      "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
      "--enforce-webrtc-ip-permission-check",
    ],
  });
  try {
    const page = context.pages()[0] || await context.newPage();
    await page.goto(url, { waitUntil: "domcontentloaded", timeout: 60_000 });
    await page.bringToFront();
    activeBrowserContexts.add(context);
    context.once("close", () => activeBrowserContexts.delete(context));
  } catch (error) {
    await context.close().catch(() => undefined);
    throw error;
  }
};

export const runAccountBrowserRequest = async (value) => {
  const request = parseAccountBrowserRequest(value);
  if (!request.proxy) {
    openDefaultBrowser(request.url);
    return { ok: true, mode: "direct" };
  }
  await openProxiedBrowser(request);
  return { ok: true, mode: "proxy" };
};

const requestInteractiveBroker = (payload) => new Promise((resolveRequest, rejectRequest) => {
  const socket = createConnection(ACCOUNT_BROWSER_PIPE);
  let response = "";
  const fail = () => rejectRequest(new Error("Le relais graphique Switch est indisponible."));
  socket.setTimeout(75_000, () => {
    socket.destroy();
    fail();
  });
  socket.once("error", fail);
  socket.on("data", (chunk) => {
    response += chunk.toString("utf8");
    if (response.length > 65_536) {
      socket.destroy();
      fail();
      return;
    }
    const newline = response.indexOf("\n");
    if (newline === -1) return;
    socket.end();
    try {
      resolveRequest(JSON.parse(response.slice(0, newline)));
    } catch {
      fail();
    }
  });
  socket.once("connect", () => socket.write(`${payload.trim()}\n`));
});

export const runAccountBrowser = async () => {
  const payload = await readStdin();
  const parsed = parseAccountBrowserRequest(JSON.parse(payload));
  // L'ouverture standard via rundll32 est relayee par Explorer jusque dans la
  // session utilisateur. Un Chrome Playwright avec son propre profil ne l'est
  // pas : il doit etre cree par le broker interactif lance au logon Windows.
  const result = parsed.proxy
    ? await requestInteractiveBroker(payload)
    : await runAccountBrowserRequest(parsed);
  writeResult(result);
  if (!result?.ok) process.exitCode = 1;
};

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  runAccountBrowser().catch((error) => {
    writeResult({
      ok: false,
      error: String(error instanceof Error ? error.message : error),
    });
    process.exitCode = 1;
  });
}
