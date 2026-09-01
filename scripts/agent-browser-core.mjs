import { existsSync, mkdirSync } from "node:fs";
import { spawn } from "node:child_process";
import { lookup as dnsLookup } from "node:dns/promises";
import { createServer as createHttpServer, request as httpRequest } from "node:http";
import { connect as connectTcp, isIP } from "node:net";
import { homedir } from "node:os";
import { join } from "node:path";

export const AGENT_BROWSER_PIPE = "\\\\.\\pipe\\CodexSwitchAgentBrowser";
export const MAX_AGENT_BROWSER_REQUEST_BYTES = 65_536;
export const MAX_AGENT_BROWSER_RESPONSE_BYTES = 524_288;

const SESSION_ID_PATTERN = /^[A-Za-z0-9_-]{16,128}$/;
const ELEMENT_REF_PATTERN = /^s[0-9]{1,8}-e[0-9]{1,4}$/;
const SAFE_KEYS = new Set([
  "Enter",
  "Escape",
  "Tab",
  "Shift+Tab",
  "Backspace",
  "Delete",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "Space",
]);

const asTrimmedString = (value, label, maximum) => {
  if (typeof value !== "string") throw new Error(`${label} invalide.`);
  const result = value.trim();
  if (!result || result.length > maximum) throw new Error(`${label} invalide.`);
  return result;
};

const privateIpv4 = (hostname) => {
  const parts = hostname.split(".").map(Number);
  if (parts.length !== 4 || parts.some((part) => !Number.isInteger(part) || part < 0 || part > 255)) return false;
  const [a, b] = parts;
  return a === 0
    || a === 10
    || a === 127
    || (a === 100 && b >= 64 && b <= 127)
    || (a === 169 && b === 254)
    || (a === 172 && b >= 16 && b <= 31)
    || (a === 192 && (b === 0 || b === 168))
    || (a === 198 && (b === 18 || b === 19 || b === 51))
    || (a === 203 && b === 0)
    || a >= 224;
};

export const isDisallowedAddress = (rawAddress) => {
  const address = String(rawAddress || "").toLowerCase().replace(/^\[|\]$/g, "").replace(/%.+$/, "");
  const family = isIP(address);
  if (family === 4) return privateIpv4(address);
  if (family !== 6) return true;

  const mappedIpv4 = address.match(/(?:^|:)(\d+\.\d+\.\d+\.\d+)$/)?.[1];
  if (mappedIpv4) return privateIpv4(mappedIpv4);
  const first = Number.parseInt(address.split(":", 1)[0] || "0", 16);
  // Seul l'espace IPv6 unicast global 2000::/3 peut sortir du poste. Cette
  // allowlist bloque notamment loopback, link-local, ULA, multicast et les
  // adresses IPv4 mappees/compatibles.
  if (!Number.isInteger(first) || first < 0x2000 || first > 0x3fff) return true;
  return address.startsWith("2001:db8:")
    || address === "2001:db8::"
    || address.startsWith("2001:0000:")
    || address.startsWith("2001:0:");
};

export const isDisallowedHost = (rawHostname) => {
  const hostname = String(rawHostname || "").toLowerCase().replace(/^\[|\]$/g, "").replace(/\.$/, "");
  if (!hostname
    || hostname === "localhost"
    || hostname.endsWith(".localhost")
    || hostname.endsWith(".local")
    || hostname.endsWith(".internal")
    || !hostname.includes(".")) return true;
  return isIP(hostname) ? isDisallowedAddress(hostname) : false;
};

export const validatedWebUrl = (rawUrl) => {
  const value = asTrimmedString(rawUrl, "URL", 4096);
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new Error("L'URL de la page est invalide.");
  }
  if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) {
    throw new Error("Seules les pages HTTP ou HTTPS sans identifiants integres sont autorisees.");
  }
  if (isDisallowedHost(url.hostname)) {
    throw new Error("Les pages locales et les adresses reseau privees ne sont pas accessibles au navigateur agent.");
  }
  return url.toString();
};

export const resolvePublicAddresses = async (rawHostname, lookupImpl = dnsLookup) => {
  const hostname = String(rawHostname || "").toLowerCase().replace(/^\[|\]$/g, "").replace(/\.$/, "");
  if (isDisallowedHost(hostname)) {
    throw new Error("Destination reseau locale ou privee refusee.");
  }
  if (isIP(hostname)) {
    return [{ address: hostname, family: isIP(hostname) }];
  }
  const records = await lookupImpl(hostname, { all: true, verbatim: true });
  if (!Array.isArray(records) || records.length === 0) {
    throw new Error("La destination web ne peut pas etre resolue.");
  }
  if (records.some((record) => isDisallowedAddress(record?.address))) {
    throw new Error("La destination web se resout vers un reseau local ou prive.");
  }
  return records.map((record) => ({
    address: record.address,
    family: Number(record.family) === 6 ? 6 : 4,
  }));
};

const proxyError = (response, status, message) => {
  if (response.headersSent) {
    response.destroy();
    return;
  }
  response.writeHead(status, { "content-type": "text/plain; charset=utf-8", connection: "close" });
  response.end(`${message}\n`);
};

export const createSafeWebProxy = ({
  lookupImpl = dnsLookup,
  requestImpl = httpRequest,
  connectImpl = connectTcp,
} = {}) => new Promise((resolveProxy, rejectProxy) => {
  const server = createHttpServer((request, response) => {
    void (async () => {
      let target;
      try {
        target = new URL(request.url || "");
      } catch {
        proxyError(response, 400, "URL proxy invalide.");
        return;
      }
      if (target.protocol !== "http:" || target.username || target.password) {
        proxyError(response, 403, "Seules les requetes HTTP directes sont acceptees par le proxy.");
        return;
      }
      const [endpoint] = await resolvePublicAddresses(target.hostname, lookupImpl);
      const headers = { ...request.headers, host: target.host };
      delete headers["proxy-authorization"];
      delete headers["proxy-connection"];
      headers.connection = "close";
      const upstream = requestImpl({
        hostname: endpoint.address,
        family: endpoint.family,
        port: Number(target.port || 80),
        method: request.method,
        path: `${target.pathname}${target.search}`,
        headers,
      }, (upstreamResponse) => {
        response.writeHead(upstreamResponse.statusCode || 502, upstreamResponse.headers);
        upstreamResponse.pipe(response);
      });
      upstream.setTimeout(45_000, () => upstream.destroy(new Error("Timeout proxy.")));
      upstream.once("error", () => proxyError(response, 502, "Destination web indisponible."));
      request.pipe(upstream);
    })().catch(() => proxyError(response, 403, "Destination web refusee."));
  });

  server.on("connect", (request, clientSocket, head) => {
    void (async () => {
      let target;
      try {
        target = new URL(`http://${request.url || ""}`);
      } catch {
        clientSocket.end("HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n");
        return;
      }
      const port = Number(target.port || 443);
      if (!target.hostname || !Number.isInteger(port) || port < 1 || port > 65_535) {
        clientSocket.end("HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n");
        return;
      }
      const [endpoint] = await resolvePublicAddresses(target.hostname, lookupImpl);
      const upstream = connectImpl({ host: endpoint.address, family: endpoint.family, port });
      upstream.setTimeout(45_000, () => upstream.destroy());
      upstream.once("connect", () => {
        clientSocket.write("HTTP/1.1 200 Connection Established\r\n\r\n");
        if (head?.length) upstream.write(head);
        upstream.pipe(clientSocket);
        clientSocket.pipe(upstream);
      });
      upstream.once("error", () => {
        if (!clientSocket.destroyed) {
          clientSocket.end("HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n");
        }
      });
      clientSocket.once("error", () => upstream.destroy());
    })().catch(() => {
      if (!clientSocket.destroyed) {
        clientSocket.end("HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n");
      }
    });
  });
  server.on("clientError", (_error, socket) => socket.destroy());
  server.once("error", rejectProxy);
  server.listen(0, "127.0.0.1", () => {
    server.removeListener("error", rejectProxy);
    const address = server.address();
    if (!address || typeof address === "string") {
      server.close();
      rejectProxy(new Error("Le proxy navigateur local ne peut pas demarrer."));
      return;
    }
    resolveProxy({
      url: `http://127.0.0.1:${address.port}`,
      close: () => new Promise((resolveClose) => server.close(() => resolveClose())),
    });
  });
});

export const requiresLocalApproval = (action, metadata = {}, key = "") => {
  if (action === "select") return true;
  if (action === "press") return ["Enter", "Space"].includes(key);
  if (action !== "click") return false;
  const label = String(metadata.label || "").toLowerCase();
  return metadata.tag === "button"
    || metadata.role === "button"
    || ["button", "submit", "image"].includes(String(metadata.type || "").toLowerCase())
    || /(?:acheter|commander|payer|envoyer|soumettre|supprimer|retirer|publier|confirmer|buy|purchase|order|pay|send|submit|delete|remove|publish|confirm|sign[- ]?out|logout)/.test(label);
};

export const confirmWindowsBrowserAction = ({ action, metadata, key, url }) => new Promise((resolveApproval) => {
  if (!requiresLocalApproval(action, metadata, key)) {
    resolveApproval(true);
    return;
  }
  const description = [
    `Action demandee par Switch : ${action}${key ? ` (${key})` : ""}`,
    metadata?.label ? `Element : ${String(metadata.label).slice(0, 240)}` : "",
    url ? `Page : ${String(url).slice(0, 500)}` : "",
    "",
    "Autoriser cette action dans Chrome ?",
  ].filter((line) => line !== "").join("\r\n");
  const message = Buffer.from(description, "utf8").toString("base64");
  const script = [
    "Add-Type -AssemblyName System.Windows.Forms",
    `$message=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('${message}'))`,
    "$result=[Windows.Forms.MessageBox]::Show($message,'Switch - confirmation navigateur',[Windows.Forms.MessageBoxButtons]::YesNo,[Windows.Forms.MessageBoxIcon]::Warning,[Windows.Forms.MessageBoxDefaultButton]::Button2)",
    "if ($result -ne [Windows.Forms.DialogResult]::Yes) { exit 3 }",
  ].join("; ");
  const encoded = Buffer.from(script, "utf16le").toString("base64");
  const child = spawn("powershell.exe", [
    "-NoProfile",
    "-STA",
    "-NonInteractive",
    "-EncodedCommand",
    encoded,
  ], { stdio: "ignore", windowsHide: true });
  let settled = false;
  const finish = (approved) => {
    if (settled) return;
    settled = true;
    clearTimeout(timer);
    resolveApproval(approved);
  };
  const timer = setTimeout(() => {
    child.kill();
    finish(false);
  }, 25_000);
  timer.unref?.();
  child.once("error", () => finish(false));
  child.once("close", (code) => finish(code === 0));
});

export const validateAgentBrowserRequest = (value) => {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Demande de controle du navigateur invalide.");
  }
  if (value.kind !== "agent-browser") {
    throw new Error("Type de demande navigateur invalide.");
  }
  const sessionId = asTrimmedString(value.sessionId, "Session navigateur", 128);
  if (!SESSION_ID_PATTERN.test(sessionId)) {
    throw new Error("Session navigateur invalide.");
  }
  const action = asTrimmedString(value.action, "Action navigateur", 32);
  const request = { kind: "agent-browser", sessionId, action };

  switch (action) {
    case "open":
      request.url = validatedWebUrl(value.url);
      break;
    case "snapshot":
    case "back":
    case "close":
    case "health":
      break;
    case "click":
      request.ref = asTrimmedString(value.ref, "Reference d'element", 32);
      if (!ELEMENT_REF_PATTERN.test(request.ref)) throw new Error("Reference d'element invalide.");
      break;
    case "fill":
      request.ref = asTrimmedString(value.ref, "Reference d'element", 32);
      if (!ELEMENT_REF_PATTERN.test(request.ref)) throw new Error("Reference d'element invalide.");
      if (typeof value.text !== "string" || value.text.length > 4000) {
        throw new Error("Le texte a saisir est invalide ou trop long.");
      }
      request.text = value.text;
      break;
    case "select":
      request.ref = asTrimmedString(value.ref, "Reference d'element", 32);
      if (!ELEMENT_REF_PATTERN.test(request.ref)) throw new Error("Reference d'element invalide.");
      request.value = asTrimmedString(value.value, "Option", 512);
      break;
    case "press":
      request.key = asTrimmedString(value.key, "Touche", 32);
      if (!SAFE_KEYS.has(request.key)) throw new Error("Cette touche n'est pas autorisee.");
      if (value.ref !== undefined && value.ref !== null && value.ref !== "") {
        request.ref = asTrimmedString(value.ref, "Reference d'element", 32);
        if (!ELEMENT_REF_PATTERN.test(request.ref)) throw new Error("Reference d'element invalide.");
      }
      break;
    default:
      throw new Error("Action navigateur non prise en charge.");
  }
  return request;
};

export const isSensitiveEditable = ({
  type = "",
  autocomplete = "",
  name = "",
  id = "",
  placeholder = "",
  ariaLabel = "",
  label = "",
}) => {
  const autocompleteTokens = String(autocomplete).toLowerCase().trim().split(/\s+/).filter(Boolean);
  if (autocompleteTokens.some((token) => token.startsWith("cc-") || [
    "current-password",
    "new-password",
    "one-time-code",
  ].includes(token))) return true;
  const haystack = `${type} ${autocomplete} ${name} ${id} ${placeholder} ${ariaLabel} ${label}`
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-");
  return type.toLowerCase() === "password"
    || /(?:^|-)(password|passwd|passphrase|passcode|pin|current-password|new-password|one-time-code|otp|totp|verification-code|security-code|auth-code|cvv|cvc|csc|cid|card-number|cardnumber|credit-card|debit-card|bank-account|account-number|routing-number|sort-code|iban)(?:$|-)/.test(haystack);
};

const defaultChromeCandidates = () => [
  process.env.CST_CHROME_PATH,
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
  `${process.env.LOCALAPPDATA || ""}/Google/Chrome/Application/chrome.exe`,
].filter(Boolean);

const defaultProfileDirectory = () => join(
  process.env.CST_AGENT_BROWSER_PROFILE_DIR
    || process.env.LOCALAPPDATA
    || join(homedir(), "AppData", "Local"),
  ...(process.env.CST_AGENT_BROWSER_PROFILE_DIR ? [] : ["CodexSwitchTerminal", "agent-browser-profile"]),
);

export class AgentBrowserController {
  constructor({
    importPlaywright = () => import("playwright-core"),
    executablePath = defaultChromeCandidates().find(existsSync),
    profileDirectory = defaultProfileDirectory(),
    createProxy = createSafeWebProxy,
    confirmAction = confirmWindowsBrowserAction,
    maxSessions = 24,
    idleMilliseconds = 30 * 60 * 1000,
    requestTimeoutMilliseconds = 90_000,
  } = {}) {
    this.importPlaywright = importPlaywright;
    this.executablePath = executablePath;
    this.profileDirectory = profileDirectory;
    this.createProxy = createProxy;
    this.confirmAction = confirmAction;
    this.maxSessions = maxSessions;
    this.idleMilliseconds = idleMilliseconds;
    this.requestTimeoutMilliseconds = requestTimeoutMilliseconds;
    this.contextPromise = null;
    this.proxy = null;
    this.sessions = new Map();
    this.queues = new Map();
    this.preparedPages = new WeakSet();
    this.cleanupTimer = setInterval(() => void this.pruneIdleSessions(), 60_000);
    this.cleanupTimer.unref?.();
  }

  async context() {
    if (!this.executablePath) throw new Error("Google Chrome est introuvable sur le poste Windows.");
    if (!this.contextPromise) {
      this.contextPromise = (async () => {
        mkdirSync(this.profileDirectory, { recursive: true });
        const proxy = await this.createProxy();
        this.proxy = proxy;
        const { chromium } = await this.importPlaywright();
        let context;
        try {
          context = await chromium.launchPersistentContext(this.profileDirectory, {
            executablePath: this.executablePath,
            headless: false,
            locale: "fr-FR",
            viewport: null,
            acceptDownloads: false,
            serviceWorkers: "block",
            proxy: { server: proxy.url, bypass: "<-loopback>" },
            args: [
              "--start-maximized",
              "--no-first-run",
              "--no-default-browser-check",
              "--disable-sync",
              "--disable-extensions",
              "--disable-component-update",
              "--disable-default-apps",
              "--disable-quic",
              "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
              "--webrtc-ip-handling-policy=disable_non_proxied_udp",
              "--disk-cache-size=268435456",
              "--media-cache-size=67108864",
              "--disable-features=AutofillServerCommunication,PasswordManagerOnboarding,OptimizationHints,MediaRouter",
            ],
          });
        } catch (error) {
          await proxy.close().catch(() => undefined);
          if (this.proxy === proxy) this.proxy = null;
          throw error;
        }
        await context.route("**/*", async (route) => {
          try {
            const target = new URL(route.request().url());
            if (["http:", "https:"].includes(target.protocol) && isDisallowedHost(target.hostname)) {
              await route.abort("blockedbyclient");
              return;
            }
          } catch {
            // Les URL internes du navigateur (about:, blob:, data:) ne sont pas
            // des navigations reseau et restent gerees par Chrome.
          }
          await route.continue();
        });
        context.once("close", () => {
          this.contextPromise = null;
          this.sessions.clear();
          if (this.proxy === proxy) this.proxy = null;
          void proxy.close().catch(() => undefined);
        });
        return context;
      })().catch((error) => {
        this.contextPromise = null;
        throw error;
      });
    }
    return this.contextPromise;
  }

  preparePage(page) {
    if (this.preparedPages.has(page)) return;
    this.preparedPages.add(page);
    page.setDefaultTimeout(10_000);
    page.on("dialog", (dialog) => void dialog.dismiss().catch(() => undefined));
    page.on("download", (download) => void download.cancel().catch(() => undefined));
  }

  async pageFor(sessionId) {
    await this.pruneIdleSessions();
    const existing = this.sessions.get(sessionId);
    if (existing && !existing.page.isClosed()) {
      existing.lastAccess = Date.now();
      return existing;
    }
    if (this.sessions.size >= this.maxSessions) {
      throw new Error("Trop de pages Switch sont deja controlees. Fermez une session puis recommencez.");
    }
    const context = await this.context();
    const reusableBlankPage = this.sessions.size === 0
      ? context.pages().find((candidate) => !candidate.isClosed() && candidate.url() === "about:blank")
      : null;
    const page = reusableBlankPage || await context.newPage();
    this.preparePage(page);
    const session = { page, lastAccess: Date.now(), snapshotId: 0, elements: new Map() };
    this.sessions.set(sessionId, session);
    return session;
  }

  async pruneIdleSessions() {
    const cutoff = Date.now() - this.idleMilliseconds;
    const closing = [];
    for (const [sessionId, session] of this.sessions) {
      if (session.page.isClosed() || session.lastAccess < cutoff) {
        this.sessions.delete(sessionId);
        if (!session.page.isClosed()) closing.push(session.page.close().catch(() => undefined));
      }
    }
    await Promise.all(closing);
  }

  enqueue(sessionId, operation) {
    const previous = this.queues.get(sessionId) || Promise.resolve();
    const next = previous.catch(() => undefined).then(operation);
    this.queues.set(sessionId, next);
    void next.finally(() => {
      if (this.queues.get(sessionId) === next) this.queues.delete(sessionId);
    }).catch(() => undefined);
    return next;
  }

  async snapshot(session) {
    session.lastAccess = Date.now();
    session.snapshotId = (session.snapshotId % 9_999_999) + 1;
    const snapshotId = session.snapshotId;
    for (const { handle } of session.elements.values()) {
      await handle.dispose().catch(() => undefined);
    }
    session.elements = new Map();
    const state = await session.page.evaluate((maxText) => {
      const clean = (value, limit = 240) => String(value || "").replace(/\s+/g, " ").trim().slice(0, limit);
      return {
        url: window.location.href,
        title: clean(document.title, 300),
        text: clean(document.body?.innerText || "", maxText),
      };
    }, 16_000);
    const selector = [
      "a[href]", "button", "input", "textarea", "select", "summary",
      "[role='button']", "[role='link']", "[role='textbox']", "[contenteditable='true']",
    ].join(",");
    const candidates = await session.page.locator(selector).elementHandles();
    const elements = [];
    for (const handle of candidates) {
      if (elements.length >= 160) {
        await handle.dispose().catch(() => undefined);
        continue;
      }
      const metadata = await handle.evaluate((element) => {
        const clean = (value, limit = 240) => String(value || "").replace(/\s+/g, " ").trim().slice(0, limit);
        const style = window.getComputedStyle(element);
        const rect = element.getBoundingClientRect();
        if (!element.isConnected || style.visibility === "hidden" || style.display === "none" || rect.width <= 0 || rect.height <= 0) {
          return null;
        }
        const tag = element.tagName.toLowerCase();
        const inputType = tag === "input" ? clean(element.getAttribute("type") || "text", 40).toLowerCase() : "";
        const associatedLabel = element.labels
          ? [...element.labels].map((candidate) => candidate.textContent || "").join(" ")
          : "";
        const sensitiveHaystack = [
          inputType,
          element.getAttribute("autocomplete"),
          element.getAttribute("name"),
          element.getAttribute("id"),
          element.getAttribute("placeholder"),
          element.getAttribute("aria-label"),
          associatedLabel,
        ].join(" ").toLowerCase().replace(/[^a-z0-9]+/g, "-");
        const sensitive = inputType === "password"
          || /(?:^|-)(?:cc-[a-z0-9-]+|password|passwd|passphrase|passcode|pin|one-time-code|otp|totp|verification-code|security-code|auth-code|cvv|cvc|csc|cid|card-number|cardnumber|credit-card|debit-card|bank-account|account-number|routing-number|sort-code|iban)(?:$|-)/.test(sensitiveHaystack);
        const editable = tag === "input"
          || tag === "textarea"
          || tag === "select"
          || element.getAttribute("contenteditable") === "true"
          || element.getAttribute("role") === "textbox";
        const label = clean(
          element.getAttribute("aria-label")
          || element.getAttribute("placeholder")
          || element.getAttribute("alt")
          || element.getAttribute("title")
          || associatedLabel
          || (!editable ? element.innerText : "")
          || element.getAttribute("name")
          || element.getAttribute("id"),
        );
        return {
          tag,
          role: clean(element.getAttribute("role"), 40) || undefined,
          type: inputType || undefined,
          label: label || undefined,
          disabled: Boolean(element.disabled || element.getAttribute("aria-disabled") === "true") || undefined,
          options: tag === "select" && !sensitive
            ? [...element.options].slice(0, 30).map((option) => ({ label: clean(option.textContent, 120) }))
            : undefined,
          manualEntryOnly: sensitive || undefined,
          editable: {
            tag,
            type: element.getAttribute("type") || "",
            autocomplete: element.getAttribute("autocomplete") || "",
            name: element.getAttribute("name") || "",
            id: element.getAttribute("id") || "",
            placeholder: element.getAttribute("placeholder") || "",
            ariaLabel: element.getAttribute("aria-label") || "",
            label: associatedLabel,
            contenteditable: element.getAttribute("contenteditable") || "",
          },
        };
      }).catch(() => null);
      if (!metadata) {
        await handle.dispose().catch(() => undefined);
        continue;
      }
      const ref = `s${snapshotId}-e${elements.length + 1}`;
      const { editable: _editable, ...publicMetadata } = metadata;
      session.elements.set(ref, { handle, metadata });
      elements.push({ ref, ...publicMetadata });
    }
    await session.page.bringToFront().catch(() => undefined);
    return { ...state, elements };
  }

  async elementFor(session, ref) {
    if (!ref.startsWith(`s${session.snapshotId}-`)) {
      throw new Error("Cette reference est perimee. Demandez un nouvel instantane de la page.");
    }
    const selected = session.elements.get(ref);
    if (!selected) {
      throw new Error("L'element n'est plus disponible. Demandez un nouvel instantane de la page.");
    }
    const connected = await selected.handle.evaluate((element) => element.isConnected).catch(() => false);
    if (!connected) {
      throw new Error("L'element n'est plus disponible. Demandez un nouvel instantane de la page.");
    }
    return selected;
  }

  async currentEditable(selected) {
    return selected.handle.evaluate((element) => ({
      tag: element.tagName.toLowerCase(),
      type: element.getAttribute("type") || "",
      autocomplete: element.getAttribute("autocomplete") || "",
      name: element.getAttribute("name") || "",
      id: element.getAttribute("id") || "",
      placeholder: element.getAttribute("placeholder") || "",
      ariaLabel: element.getAttribute("aria-label") || "",
      label: element.labels ? [...element.labels].map((candidate) => candidate.textContent || "").join(" ") : "",
      contenteditable: element.getAttribute("contenteditable") || "",
    }));
  }

  async approveAction(session, action, selected = null, key = "") {
    const approved = await this.confirmAction({
      action,
      metadata: selected?.metadata || {},
      key,
      url: session.page.url(),
    });
    if (!approved) {
      throw new Error("Action annulee ou non confirmee sur le PC.");
    }
  }

  async handle(rawRequest) {
    const request = validateAgentBrowserRequest(rawRequest);
    return this.enqueue(request.sessionId, async () => {
      if (request.action === "health") {
        return { ok: true, ready: true };
      }
      if (request.action === "close") {
        const session = this.sessions.get(request.sessionId);
        this.sessions.delete(request.sessionId);
        if (session && !session.page.isClosed()) await session.page.close().catch(() => undefined);
        return { ok: true, closed: true };
      }
      // Chien de garde : une action qui depasse le delai (page lente, CDP
      // bloque) est interrompue et la session liberee, au lieu de bloquer la
      // file pour toujours — meme mecanique que le relais ecran persistant.
      let timedOut = false;
      let timer;
      const watchdog = new Promise((_resolve, rejectTimeout) => {
        timer = setTimeout(() => {
          timedOut = true;
          rejectTimeout(new Error("Le navigateur Switch a mis trop de temps a repondre."));
        }, this.requestTimeoutMilliseconds);
      });
      timer.unref?.();
      try {
        return await Promise.race([this.operate(request), watchdog]);
      } catch (error) {
        if (timedOut) {
          const session = this.sessions.get(request.sessionId);
          this.sessions.delete(request.sessionId);
          if (session && !session.page.isClosed()) await session.page.close().catch(() => undefined);
        }
        throw error;
      } finally {
        clearTimeout(timer);
      }
    });
  }

  async operate(request) {
    const session = await this.pageFor(request.sessionId);
    session.lastAccess = Date.now();
      switch (request.action) {
        case "open":
          await session.page.goto(request.url, { waitUntil: "domcontentloaded", timeout: 40_000 });
          return { ok: true, page: await this.snapshot(session) };
        case "snapshot":
          return { ok: true, page: await this.snapshot(session) };
        case "back":
          await session.page.goBack({ waitUntil: "domcontentloaded", timeout: 20_000 });
          return { ok: true, page: await this.snapshot(session) };
        case "click": { 
          const selected = await this.elementFor(session, request.ref);
          await this.approveAction(session, "click", selected);
          const popup = session.page.waitForEvent("popup", { timeout: 1500 }).catch(() => null);
          await selected.handle.click();
          const opened = await popup;
          if (opened) {
            const previousPage = session.page;
            session.page = opened;
            this.preparePage(opened);
            await opened.waitForLoadState("domcontentloaded", { timeout: 8_000 }).catch(() => undefined);
            await previousPage.close().catch(() => undefined);
          } else {
            await session.page.waitForLoadState("domcontentloaded", { timeout: 3_000 }).catch(() => undefined);
          }
          return { ok: true, page: await this.snapshot(session) };
        }
        case "fill": {
          const selected = await this.elementFor(session, request.ref);
          const editable = await this.currentEditable(selected);
          if (isSensitiveEditable(editable)) {
            throw new Error("Switch ne saisit pas les mots de passe, codes de verification ou donnees bancaires. Saisissez-les vous-meme dans la fenetre Chrome.");
          }
          if (!["input", "textarea"].includes(editable.tag) && editable.contenteditable !== "true") {
            throw new Error("Cet element n'accepte pas de texte.");
          }
          await selected.handle.fill(request.text);
          return { ok: true, page: await this.snapshot(session) };
        }
        case "select": {
          const selected = await this.elementFor(session, request.ref);
          const editable = await this.currentEditable(selected);
          if (isSensitiveEditable(editable)) {
            throw new Error("Switch ne choisit pas de donnee bancaire ou d'identifiant sensible. Selectionnez-la vous-meme dans Chrome.");
          }
          await this.approveAction(session, "select", selected);
          await selected.handle.selectOption({ label: request.value });
          return { ok: true, page: await this.snapshot(session) };
        }
        case "press": {
          if (request.ref) {
            const selected = await this.elementFor(session, request.ref);
            const editable = await this.currentEditable(selected);
            if (isSensitiveEditable(editable)) {
              throw new Error("Switch ne modifie pas un champ sensible. Utilisez directement Chrome.");
            }
            await this.approveAction(session, "press", selected, request.key);
            await selected.handle.press(request.key);
          } else {
            const active = await session.page.evaluate(() => {
              const element = document.activeElement;
              if (!element) return {};
              return {
                tag: element.tagName.toLowerCase(),
                type: element.getAttribute("type") || "",
                autocomplete: element.getAttribute("autocomplete") || "",
                name: element.getAttribute("name") || "",
                id: element.getAttribute("id") || "",
                placeholder: element.getAttribute("placeholder") || "",
                ariaLabel: element.getAttribute("aria-label") || "",
                label: element.labels ? [...element.labels].map((candidate) => candidate.textContent || "").join(" ") : "",
              };
            });
            if (isSensitiveEditable(active)) {
              throw new Error("Switch ne modifie pas un champ sensible. Utilisez directement Chrome.");
            }
            await this.approveAction(session, "press", null, request.key);
            await session.page.keyboard.press(request.key);
          }
          return { ok: true, page: await this.snapshot(session) };
        }
        default:
          throw new Error("Action navigateur non prise en charge.");
      }
  }

  // Pre-chauffe : lance Chrome et le proxy en arriere-plan (broker demarre)
  // pour que la premiere action de l'agent ne paie pas le lancement a froid.
  async warmUp() {
    await this.context().catch(() => undefined);
  }

  async close() {
    clearInterval(this.cleanupTimer);
    const context = await this.contextPromise?.catch(() => null);
    this.sessions.clear();
    if (context) await context.close().catch(() => undefined);
    const proxy = this.proxy;
    this.proxy = null;
    if (proxy) await proxy.close().catch(() => undefined);
  }
}
