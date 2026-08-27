import http from "node:http";
import net from "node:net";
import { pipeline } from "node:stream";

const listenPort = positiveInteger(process.env.LISTEN_PORT, 8081);
const mainUpstream = internalHttpUrl(process.env.MAIN_UPSTREAM_URL ?? "http://cst:8080");
const socialUpstream = internalHttpUrl(process.env.SOCIAL_UPSTREAM_URL ?? "http://social:8090");
const proxyKey = process.env.SOCIAL_PROXY_KEY?.trim() ?? "";
const forwardedProto = process.env.FORWARDED_PROTO?.trim() || "https";
const socialTimeoutMs = positiveInteger(process.env.SOCIAL_REQUEST_TIMEOUT_MS, 30_000);
const socialSyncTimeoutMs = Math.max(
  socialTimeoutMs,
  positiveInteger(process.env.SOCIAL_SYNC_TIMEOUT_MS, 600_000),
);
const mainTimeoutMs = positiveInteger(process.env.MAIN_REQUEST_TIMEOUT_MS, 600_000);
const maximumSocialBodyBytes = positiveInteger(process.env.SOCIAL_MAX_BODY_BYTES, 65_536);

if (Buffer.byteLength(proxyKey) < 32) {
  throw new Error("SOCIAL_PROXY_KEY doit contenir au moins 32 octets");
}

function positiveInteger(value, fallback) {
  const parsed = Number.parseInt(value ?? "", 10);
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : fallback;
}

function internalHttpUrl(value) {
  const url = new URL(value);
  if (url.protocol !== "http:" || url.username || url.password || url.pathname !== "/") {
    throw new Error("Les upstreams doivent être des URL internes http:// sans identifiants ni chemin");
  }
  return url;
}

function requestRoute(rawUrl) {
  try {
    const parsed = new URL(rawUrl ?? "/", "http://switch-development.internal");
    const pathname = parsed.pathname;
    const social = pathname === "/social"
      || pathname.startsWith("/social/")
      || pathname === "/api/social"
      || pathname.startsWith("/api/social/");
    return { social, pathname, path: `${pathname}${parsed.search}` };
  } catch {
    return null;
  }
}

function stripHopByHop(headers) {
  const clean = { ...headers };
  const connectionTokens = String(headers.connection ?? "")
    .split(",")
    .map((token) => token.trim().toLowerCase())
    .filter(Boolean);
  for (const name of [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    ...connectionTokens,
  ]) delete clean[name];
  return clean;
}

function forwardedHeaders(request, target, social) {
  const headers = stripHopByHop(request.headers);
  for (const name of [
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-port",
    "x-forwarded-proto",
    "x-social-proxy-key",
  ]) delete headers[name];
  headers.host = target.host;
  headers["x-forwarded-host"] = request.headers.host ?? "";
  headers["x-forwarded-proto"] = forwardedProto;
  if (request.socket.remoteAddress) headers["x-forwarded-for"] = request.socket.remoteAddress;
  if (social) {
    delete headers.authorization;
    headers["x-social-proxy-key"] = proxyKey;
  }
  return headers;
}

function downstreamHeaders(upstreamResponse, social) {
  const headers = stripHopByHop(upstreamResponse.headers);
  if (social) delete headers["set-cookie"];
  return headers;
}

function sendJson(response, status, payload) {
  const body = JSON.stringify(payload);
  response.writeHead(status, {
    "cache-control": "no-store",
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(body),
    "x-content-type-options": "nosniff",
  });
  response.end(body);
}

async function upstreamReady(url, headers = {}) {
  try {
    const response = await fetch(url, {
      headers,
      cache: "no-store",
      signal: AbortSignal.timeout(3_000),
    });
    return response.ok;
  } catch {
    return false;
  }
}

const server = http.createServer(async (request, response) => {
  const route = requestRoute(request.url);
  if (!route) {
    sendJson(response, 400, { ok: false, error: "URL de requête invalide" });
    return;
  }

  if (request.method === "GET" && route.pathname === "/healthz") {
    sendJson(response, 200, { ok: true, service: "switch-development-gateway" });
    return;
  }
  if (request.method === "GET" && route.pathname === "/readyz") {
    const [mainReady, socialReady] = await Promise.all([
      upstreamReady(new URL("/healthz", mainUpstream)),
      upstreamReady(new URL("/readyz", socialUpstream)),
    ]);
    sendJson(response, mainReady && socialReady ? 200 : 503, {
      ok: mainReady && socialReady,
      mainReady,
      socialReady,
    });
    return;
  }
  if (request.method === "GET" && route.pathname === "/api/social/availability") {
    const available = await upstreamReady(new URL("/readyz", socialUpstream));
    sendJson(response, available ? 200 : 503, { ok: available, available });
    return;
  }

  const declaredLength = Number.parseInt(request.headers["content-length"] ?? "0", 10);
  if (route.social && Number.isFinite(declaredLength) && declaredLength > maximumSocialBodyBytes) {
    sendJson(response, 413, { ok: false, error: "Corps de requête trop volumineux" });
    request.resume();
    return;
  }

  const target = route.social ? socialUpstream : mainUpstream;
  const socialSync = route.social
    && request.method === "POST"
    && route.pathname === "/api/social/sync";
  const timeoutMs = socialSync ? socialSyncTimeoutMs : route.social ? socialTimeoutMs : mainTimeoutMs;
  const proxyRequest = http.request({
    hostname: target.hostname,
    port: Number.parseInt(target.port || "80", 10),
    method: request.method,
    path: route.path,
    headers: forwardedHeaders(request, target, route.social),
  }, (upstreamResponse) => {
    response.writeHead(
      upstreamResponse.statusCode ?? 502,
      upstreamResponse.statusMessage,
      downstreamHeaders(upstreamResponse, route.social),
    );
    pipeline(upstreamResponse, response, (error) => {
      if (error && !response.destroyed) response.destroy(error);
    });
  });

  proxyRequest.setTimeout(timeoutMs, () => {
    proxyRequest.destroy(new Error("Délai upstream dépassé"));
  });
  proxyRequest.on("error", () => {
    if (!response.headersSent) {
      sendJson(response, 502, { ok: false, error: "Service amont indisponible" });
    } else if (!response.destroyed) {
      response.destroy();
    }
  });
  request.on("aborted", () => proxyRequest.destroy());
  request.pipe(proxyRequest);
});

server.on("upgrade", (request, clientSocket, head) => {
  const route = requestRoute(request.url);
  if (!route || route.social) {
    clientSocket.destroy();
    return;
  }
  const targetPort = Number.parseInt(mainUpstream.port || "80", 10);
  const upstreamSocket = net.connect(targetPort, mainUpstream.hostname);
  upstreamSocket.setTimeout(10_000, () => upstreamSocket.destroy());
  upstreamSocket.on("connect", () => {
    upstreamSocket.setTimeout(0);
    const headers = forwardedHeaders(request, mainUpstream, false);
    headers.connection = "Upgrade";
    headers.upgrade = request.headers.upgrade ?? "websocket";
    const requestLine = `${request.method} ${route.path} HTTP/${request.httpVersion}\r\n`;
    const headerLines = Object.entries(headers)
      .flatMap(([name, value]) => Array.isArray(value)
        ? value.map((item) => `${name}: ${item}\r\n`)
        : value == null ? [] : [`${name}: ${value}\r\n`])
      .join("");
    upstreamSocket.write(`${requestLine}${headerLines}\r\n`);
    if (head.length) upstreamSocket.write(head);
    clientSocket.pipe(upstreamSocket).pipe(clientSocket);
  });
  upstreamSocket.on("error", () => clientSocket.destroy());
  clientSocket.on("error", () => upstreamSocket.destroy());
});

server.listen(listenPort, "0.0.0.0", () => {
  console.log(`Switch développement gateway écoute sur 0.0.0.0:${listenPort}`);
});
