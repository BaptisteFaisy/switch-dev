import { createServer } from "node:http";
import { timingSafeEqual } from "node:crypto";

const MAX_BODY_BYTES = 2 * 1024 * 1024;

const json = (response, status, body) => {
  const payload = Buffer.from(`${JSON.stringify(body)}\n`);
  response.writeHead(status, {
    "cache-control": "no-store",
    "content-length": payload.length,
    "content-type": "application/json; charset=utf-8",
    "x-content-type-options": "nosniff",
  });
  response.end(payload);
};

const errorResponse = (response, status, code, message) =>
  json(response, status, { error: { code, message } });

const authenticate = (request, token) => {
  const header = request.headers.authorization;
  if (typeof header !== "string" || !header.startsWith("Bearer ")) return false;
  const provided = Buffer.from(header.slice(7), "utf8");
  const expected = Buffer.from(token, "utf8");
  return provided.length === expected.length && timingSafeEqual(provided, expected);
};

const readJson = async (request) => {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    if (size > MAX_BODY_BYTES) {
      const error = new Error("request body is too large");
      error.code = "BODY_TOO_LARGE";
      throw error;
    }
    chunks.push(chunk);
  }
  if (chunks.length === 0) return {};
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    const error = new Error("request body is not valid JSON");
    error.code = "INVALID_JSON";
    throw error;
  }
};

const integerQuery = (url, name, fallback) => {
  const raw = url.searchParams.get(name);
  if (raw === null) return fallback;
  const value = Number(raw);
  if (!Number.isInteger(value) || value < 0) throw new TypeError(`${name} is invalid`);
  return value;
};

const routeMatch = (pathname, suffix = "") => {
  const expression = suffix
    ? new RegExp(`^/v1/runs/([a-zA-Z0-9_-]+)${suffix}$`, "u")
    : /^\/v1\/runs\/([a-zA-Z0-9_-]+)$/u;
  return pathname.match(expression);
};

export const createMassSubagentHttpServer = ({
  service,
  scheduler,
  adminToken,
  environment = "development",
  realChatDispatchEnabled = false,
  onShutdownRequested = null,
  releaseId = "development-unpackaged",
}) => {
  if (!service || !scheduler) throw new TypeError("service and scheduler are required");
  if (environment !== "development") throw new Error("SWITCH_ENV must be development");
  if (typeof adminToken !== "string" || adminToken.length < 32) {
    throw new TypeError("MASS_SUBAGENTS_ADMIN_TOKEN must contain at least 32 characters");
  }
  if (onShutdownRequested !== null && typeof onShutdownRequested !== "function") {
    throw new TypeError("onShutdownRequested must be a function when provided");
  }
  if (typeof releaseId !== "string" || !/^[a-z0-9][a-z0-9._-]{0,127}$/u.test(releaseId)) {
    throw new TypeError("releaseId has an invalid format");
  }

  const server = createServer(async (request, response) => {
    const url = new URL(request.url ?? "/", "http://sidecar.invalid");
    if (request.method === "GET" && url.pathname === "/healthz") {
      json(response, 200, {
        ok: true,
        environment: "development",
        executors: realChatDispatchEnabled ? ["fake", "switch_readonly"] : ["fake"],
        switchCapacity: scheduler.capacity,
        realChatDispatchEnabled,
        releaseId,
      });
      return;
    }
    if (!authenticate(request, adminToken)) {
      errorResponse(response, 401, "AUTH_REQUIRED", "authentication required");
      return;
    }

    try {
      if (request.method === "POST" && url.pathname === "/v1/admin/shutdown") {
        if (!onShutdownRequested) {
          errorResponse(response, 409, "SHUTDOWN_DISABLED", "graceful shutdown is not configured");
          return;
        }
        json(response, 202, { ok: true, status: "shutting_down" });
        setImmediate(onShutdownRequested);
        return;
      }
      if (request.method === "GET" && url.pathname === "/v1/roles") {
        json(response, 200, service.roles());
        return;
      }
      if (request.method === "GET" && url.pathname === "/v1/runs") {
        json(response, 200, await service.list({
          limit: integerQuery(url, "limit", 100),
          cursor: integerQuery(url, "cursor", 0),
        }));
        return;
      }
      if (request.method === "POST" && url.pathname === "/v1/runs") {
        const created = await service.create(await readJson(request));
        json(response, created.idempotentReplay === true ? 200 : 201, created);
        return;
      }
      if (request.method === "GET" && url.pathname === "/v1/switch/contract/probe") {
        json(response, 200, await service.probeSwitchContract());
        return;
      }
      const agentsMatch = routeMatch(url.pathname, "/agents");
      if (request.method === "GET" && agentsMatch) {
        json(response, 200, await service.agents(agentsMatch[1], {
          limit: integerQuery(url, "limit", 100),
          cursor: integerQuery(url, "cursor", 0),
        }));
        return;
      }
      const controlMatch = routeMatch(url.pathname, "/control");
      if (request.method === "POST" && controlMatch) {
        const body = await readJson(request);
        json(response, 200, await service.control(controlMatch[1], body.action));
        return;
      }
      const runMatch = routeMatch(url.pathname);
      if (request.method === "GET" && runMatch) {
        json(response, 200, await service.get(runMatch[1]));
        return;
      }
      errorResponse(response, 404, "NOT_FOUND", "route not found");
    } catch (error) {
      if (error?.code === "BODY_TOO_LARGE") {
        errorResponse(response, 413, "BODY_TOO_LARGE", error.message);
      } else if (error?.code === "INVALID_JSON" || error instanceof TypeError) {
        errorResponse(response, 400, "INVALID_REQUEST", error.message);
      } else if (error?.code === "ENOENT") {
        errorResponse(response, 404, "RUN_NOT_FOUND", "run not found");
      } else if (typeof error?.code === "string" && error.code.startsWith("REAL_DISPATCH_")) {
        errorResponse(response, 409, error.code, error.message);
      } else if (error?.code === "SWITCH_CANCEL_INCOMPLETE") {
        errorResponse(response, 409, error.code, error.message);
      } else if (error?.code === "STORE_RUN_LIMIT_REACHED"
        || error?.code === "STORE_ARTIFACT_TOO_LARGE") {
        errorResponse(response, 507, error.code, error.message);
      } else {
        errorResponse(response, 500, "INTERNAL_ERROR", "request failed");
      }
    }
  });
  server.on("clientError", (_error, socket) => {
    socket.end("HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n");
  });
  return server;
};

export const httpServerInternals = Object.freeze({ authenticate, readJson, routeMatch });
