import { assertDevelopmentSwitchBaseUrl } from "./dev-safety.mjs";

export const SWITCH_HTTP_V1_OPERATIONS = Object.freeze({
  health: Object.freeze({ method: "GET", path: "/healthz", authenticationRequired: false, readOnly: true }),
  listActiveChatTurns: Object.freeze({
    method: "GET",
    path: "/api/chat/turns/active",
    authenticationRequired: true,
    readOnly: true,
  }),
  listOrchestrations: Object.freeze({
    method: "GET",
    path: "/api/orchestrations",
    authenticationRequired: true,
    readOnly: true,
  }),
});

export const SWITCH_HTTP_V2_OPERATIONS = Object.freeze({
  ...SWITCH_HTTP_V1_OPERATIONS,
  startChatTurn: Object.freeze({
    method: "POST",
    path: "/api/chat/turns",
    authenticationRequired: true,
    readOnly: false,
  }),
  getChatTurn: Object.freeze({
    method: "GET",
    pathTemplate: "/api/chat/turns/{id}",
    authenticationRequired: true,
    readOnly: true,
  }),
  cancelChatTurn: Object.freeze({
    method: "DELETE",
    pathTemplate: "/api/chat/turns/{id}",
    authenticationRequired: true,
    readOnly: false,
  }),
});

export const SWITCH_HTTP_V1_ROUTES = Object.freeze(Object.fromEntries(
  Object.entries(SWITCH_HTTP_V1_OPERATIONS).map(([name, operation]) => [name, operation.path]),
));

export class SwitchClientError extends Error {
  constructor(message, { code, operation, status } = {}) {
    super(message);
    this.name = "SwitchClientError";
    this.code = code ?? "SWITCH_CLIENT_ERROR";
    if (operation !== undefined) this.operation = operation;
    if (status !== undefined) this.status = status;
  }
}

const validateTimeout = (timeoutMs) => {
  if (!Number.isInteger(timeoutMs) || timeoutMs <= 0) {
    throw new TypeError("timeoutMs must be a positive integer.");
  }
  return timeoutMs;
};

const validateToken = (token) => {
  if (token === undefined || token === null || token === "") return null;
  if (typeof token !== "string" || token.trim() === "") {
    throw new TypeError("token must be a non-empty string when provided.");
  }
  return token.trim();
};

const isPlainObject = (value) => value !== null
  && typeof value === "object"
  && !Array.isArray(value)
  && (Object.getPrototypeOf(value) === Object.prototype
    || Object.getPrototypeOf(value) === null);

const requiredText = (value, label, maximum = 65_536) => {
  if (typeof value !== "string" || value.trim() === "" || value.length > maximum) {
    throw new TypeError(`${label} must be a non-empty string of at most ${maximum} characters.`);
  }
  if (value.includes("\0")) throw new TypeError(`${label} must not contain a NUL byte.`);
  return value;
};

const optionalText = (value, label, maximum = 4_096) => {
  if (value === undefined || value === null) return value;
  return requiredText(value, label, maximum);
};

const START_CHAT_KEYS = new Set([
  "accountId",
  "sessionId",
  "prompt",
  "imageAttachments",
  "projectDir",
  "mode",
  "toolScope",
  "model",
  "reasoningEffort",
  "appConnectors",
  "appWriteApproved",
  "agentTools",
  "agentSkills",
  "questionTool",
  "proofTool",
  "sourceChatKey",
]);

export const validateStartChatTurnRequest = (candidate) => {
  if (!isPlainObject(candidate)) throw new TypeError("chat turn request must be an object.");
  const extra = Object.keys(candidate).filter((key) => !START_CHAT_KEYS.has(key));
  if (extra.length > 0) throw new TypeError(`chat turn request has unknown fields: ${extra.join(", ")}.`);
  requiredText(candidate.accountId, "chat turn accountId", 256);
  requiredText(candidate.prompt, "chat turn prompt", 131_072);
  optionalText(candidate.sessionId, "chat turn sessionId", 512);
  optionalText(candidate.projectDir, "chat turn projectDir", 4_096);
  optionalText(candidate.model, "chat turn model", 128);
  optionalText(candidate.reasoningEffort, "chat turn reasoningEffort", 32);
  optionalText(candidate.sourceChatKey, "chat turn sourceChatKey", 160);
  if (candidate.mode !== "ask") {
    throw new TypeError("mass-subagent chat turns must use Switch read-only ask mode.");
  }
  if (candidate.toolScope !== "none") {
    throw new TypeError("mass-subagent chat turns must use Switch toolScope=none.");
  }
  for (const field of ["imageAttachments", "appConnectors", "agentTools", "agentSkills"]) {
    if (!Array.isArray(candidate[field]) || candidate[field].length !== 0) {
      throw new TypeError(`chat turn ${field} must be an explicit empty array.`);
    }
  }
  for (const field of ["appWriteApproved", "questionTool", "proofTool"]) {
    if (candidate[field] !== false) {
      throw new TypeError(`chat turn ${field} must be false.`);
    }
  }
  return candidate;
};

const validateTurnId = (value) => {
  if (Number.isSafeInteger(value) && value > 0) return String(value);
  if (typeof value === "string" && /^[1-9][0-9]{0,19}$/u.test(value)) return value;
  throw new TypeError("chat turn id must be a positive integer.");
};

const readBoundedBody = async (response, operation, maximumBytes) => {
  const declared = Number(response.headers.get("content-length"));
  if (Number.isFinite(declared) && declared > maximumBytes) {
    throw new SwitchClientError(`Switch ${operation} response exceeds the configured limit.`, {
      code: "SWITCH_RESPONSE_TOO_LARGE",
      operation,
      status: response.status,
    });
  }
  if (!response.body?.getReader) {
    const body = await response.text();
    if (Buffer.byteLength(body, "utf8") > maximumBytes) {
      throw new SwitchClientError(`Switch ${operation} response exceeds the configured limit.`, {
        code: "SWITCH_RESPONSE_TOO_LARGE",
        operation,
        status: response.status,
      });
    }
    return body;
  }
  const reader = response.body.getReader();
  const chunks = [];
  let size = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > maximumBytes) {
      await reader.cancel().catch(() => {});
      throw new SwitchClientError(`Switch ${operation} response exceeds the configured limit.`, {
        code: "SWITCH_RESPONSE_TOO_LARGE",
        operation,
        status: response.status,
      });
    }
    chunks.push(Buffer.from(value));
  }
  return Buffer.concat(chunks).toString("utf8");
};

const decodeResponse = async (response, operation, maximumBytes) => {
  if (!response.ok) {
    throw new SwitchClientError(`Switch ${operation} failed with HTTP ${response.status}.`, {
      code: "SWITCH_HTTP_ERROR",
      operation,
      status: response.status,
    });
  }
  if (response.status === 204) return null;
  const body = await readBoundedBody(response, operation, maximumBytes);
  if (body === "") return null;
  const contentType = response.headers.get("content-type") ?? "";
  const looksLikeJson = /^[\s]*[\[{]/.test(body);
  if (!contentType.toLowerCase().includes("json") && !looksLikeJson) return body;
  try {
    return JSON.parse(body);
  } catch {
    throw new SwitchClientError(`Switch ${operation} returned invalid JSON.`, {
      code: "SWITCH_INVALID_RESPONSE",
      operation,
      status: response.status,
    });
  }
};

const requireArray = (value, operation) => {
  if (Array.isArray(value)) return value;
  throw new SwitchClientError(`Switch ${operation} did not return an array.`, {
    code: "SWITCH_INVALID_RESPONSE",
    operation,
  });
};

const CHAT_TURN_STATUSES = new Set(["running", "finalizing", "completed", "failed", "cancelled"]);

const requireChatTurn = (value, operation) => {
  const validId = Number.isSafeInteger(value?.id)
    ? value.id > 0
    : typeof value?.id === "string" && /^[1-9][0-9]{0,19}$/u.test(value.id);
  if (!isPlainObject(value)
    || !validId
    || !CHAT_TURN_STATUSES.has(value.status)
    || (value.sourceChatKey !== null
      && value.sourceChatKey !== undefined
      && typeof value.sourceChatKey !== "string")) {
    throw new SwitchClientError(`Switch ${operation} returned an invalid chat turn.`, {
      code: "SWITCH_INVALID_RESPONSE",
      operation,
    });
  }
  return value;
};

export const createSwitchClient = ({
  baseUrl,
  token,
  timeoutMs = 8_000,
  maxResponseBytes = 4 * 1024 * 1024,
  allowChatDispatch = false,
  fetchImpl = globalThis.fetch,
} = {}) => {
  const origin = assertDevelopmentSwitchBaseUrl(baseUrl);
  const bearerToken = validateToken(token);
  const requestTimeoutMs = validateTimeout(timeoutMs);
  if (!Number.isInteger(maxResponseBytes)
    || maxResponseBytes < 1_024
    || maxResponseBytes > 32 * 1024 * 1024) {
    throw new TypeError("maxResponseBytes must be between 1024 and 33554432.");
  }
  if (typeof allowChatDispatch !== "boolean") {
    throw new TypeError("allowChatDispatch must be a boolean.");
  }
  if (typeof fetchImpl !== "function") throw new TypeError("fetchImpl must be a function.");

  const request = async (operation, { signal, body, turnId } = {}) => {
    const contract = SWITCH_HTTP_V2_OPERATIONS[operation];
    if (!contract) throw new TypeError(`unknown Switch operation ${operation}`);
    if (!contract.readOnly && !allowChatDispatch) {
      throw new SwitchClientError(
        `Switch ${operation} is disabled until read-only chat dispatch is explicitly enabled.`,
        { code: "SWITCH_WRITE_DISABLED", operation },
      );
    }
    if (contract.authenticationRequired && bearerToken === null) {
      throw new SwitchClientError(
        `Switch ${operation} requires the development administrator token.`,
        { code: "SWITCH_AUTH_REQUIRED", operation },
      );
    }
    if (signal !== undefined && !(signal instanceof AbortSignal)) {
      throw new TypeError("signal must be an AbortSignal when provided.");
    }

    const controller = new AbortController();
    let timedOut = false;
    const abortFromCaller = () => controller.abort();
    if (signal?.aborted) abortFromCaller();
    else signal?.addEventListener("abort", abortFromCaller, { once: true });
    const timer = setTimeout(() => {
      timedOut = true;
      controller.abort();
    }, requestTimeoutMs);
    timer.unref?.();

    const headers = { Accept: "application/json" };
    if (contract.authenticationRequired) headers.Authorization = `Bearer ${bearerToken}`;
    let encodedBody;
    if (body !== undefined) {
      encodedBody = JSON.stringify(body);
      headers["Content-Type"] = "application/json";
    }
    const route = contract.pathTemplate
      ? contract.pathTemplate.replace("{id}", validateTurnId(turnId))
      : contract.path;

    try {
      const response = await fetchImpl(new URL(route, `${origin}/`), {
        method: contract.method,
        headers,
        ...(encodedBody === undefined ? {} : { body: encodedBody }),
        cache: "no-store",
        redirect: "error",
        signal: controller.signal,
      });
      return await decodeResponse(response, operation, maxResponseBytes);
    } catch (error) {
      if (error instanceof SwitchClientError) throw error;
      if (timedOut) {
        throw new SwitchClientError(
          `Switch ${operation} timed out after ${requestTimeoutMs} ms.`,
          { code: "SWITCH_TIMEOUT", operation },
        );
      }
      if (signal?.aborted || controller.signal.aborted) {
        throw new SwitchClientError(`Switch ${operation} was aborted.`, {
          code: "SWITCH_ABORTED",
          operation,
        });
      }
      throw new SwitchClientError(`Switch ${operation} could not reach the development endpoint.`, {
        code: "SWITCH_NETWORK_ERROR",
        operation,
      });
    } finally {
      clearTimeout(timer);
      signal?.removeEventListener("abort", abortFromCaller);
    }
  };

  const health = (options) => request("health", options);
  const listActiveChatTurns = async (options) =>
    requireArray(await request("listActiveChatTurns", options), "listActiveChatTurns")
      .map((turn) => requireChatTurn(turn, "listActiveChatTurns"));
  const listOrchestrations = async (options) =>
    requireArray(await request("listOrchestrations", options), "listOrchestrations");
  const startChatTurn = async (candidate, options = {}) => requireChatTurn(
    await request("startChatTurn", { ...options, body: validateStartChatTurnRequest(candidate) }),
    "startChatTurn",
  );
  const getChatTurn = async (turnId, options = {}) => requireChatTurn(
    await request("getChatTurn", { ...options, turnId }),
    "getChatTurn",
  );
  const cancelChatTurn = async (turnId, options = {}) => requireChatTurn(
    await request("cancelChatTurn", { ...options, turnId }),
    "cancelChatTurn",
  );
  const findActiveChatTurnBySourceChatKey = async (
    sourceChatKey,
    { accountId, signal } = {},
  ) => {
    requiredText(sourceChatKey, "sourceChatKey", 160);
    optionalText(accountId, "accountId", 256);
    const matches = (await listActiveChatTurns({ signal }))
      .filter((turn) => turn?.sourceChatKey === sourceChatKey
        && (accountId === undefined || turn?.accountId === accountId))
      .map((turn) => requireChatTurn(turn, "listActiveChatTurns"));
    if (matches.length > 1) {
      throw new SwitchClientError(
        "Switch returned multiple active chat turns for one sourceChatKey.",
        { code: "SWITCH_DUPLICATE_SOURCE_CHAT_KEY", operation: "listActiveChatTurns" },
      );
    }
    return matches[0] ?? null;
  };
  const probeContract = async (options) => {
    if (bearerToken === null) {
      throw new SwitchClientError(
        "Switch contract probing requires the development administrator token.",
        { code: "SWITCH_AUTH_REQUIRED", operation: "probeContract" },
      );
    }
    const [healthResult, activeChatTurns, orchestrations] = await Promise.all([
      health(options),
      listActiveChatTurns(options),
      listOrchestrations(options),
    ]);
    return { health: healthResult, activeChatTurns, orchestrations };
  };

  return Object.freeze({
    authenticated: bearerToken !== null,
    chatDispatchEnabled: allowChatDispatch,
    health,
    listActiveChatTurns,
    listOrchestrations,
    startChatTurn,
    getChatTurn,
    cancelChatTurn,
    findActiveChatTurnBySourceChatKey,
    probeContract,
  });
};
