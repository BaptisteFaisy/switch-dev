import { createServer } from "node:http";
import {
  createCipheriv,
  createDecipheriv,
  createHash,
  createHmac,
  randomBytes,
  randomUUID,
  timingSafeEqual,
} from "node:crypto";
import { mkdir, readFile, readdir, rename, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createCivilTime } from "./time.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const publicDir = path.join(here, "public");
const port = Number.parseInt(process.env.PORT ?? "8090", 10);
const dataDir = process.env.SOCIAL_DATA_DIR ?? "/data";
const storePath = path.join(dataDir, "social-store.json");
const switchAuthUrl = process.env.SWITCH_AUTH_URL
  ?? "http://switch-developpement:8080/api/auth/me";
const timezone = process.env.SOCIAL_TIMEZONE ?? "Europe/Paris";
const demoMode = (process.env.SOCIAL_DEMO_MODE ?? "false").toLowerCase() === "true";
const demoSeedEnabled = (process.env.SOCIAL_DEMO_SEED ?? "false").toLowerCase() === "true";
const allowedRanges = new Set([7, 30, 90]);
const csrfHeader = "x-switch-social-request";
const maxDemoAccountsPerOwner = 12;
const socialProxyKey = process.env.SOCIAL_PROXY_KEY?.trim() ?? "";
const tokenEncryptionSecret = process.env.SOCIAL_TOKEN_ENCRYPTION_KEY?.trim() ?? "";
const socialPublicOrigin = configuredOrigin(process.env.SOCIAL_PUBLIC_ORIGIN?.trim() ?? "");
const oauthTtlMilliseconds = 10 * 60 * 1_000;
const maxPaginationPages = 200;
const initialSyncDays = Math.max(1, Math.min(
  90,
  Number.parseInt(process.env.SOCIAL_INITIAL_SYNC_DAYS ?? "90", 10) || 90,
));
const syncIntervalMilliseconds = Math.max(
  15,
  Number.parseInt(process.env.SOCIAL_SYNC_INTERVAL_MINUTES ?? "360", 10) || 360,
) * 60 * 1_000;
const scheduledSyncEnabled = (process.env.SOCIAL_SCHEDULED_SYNC_ENABLED ?? "true")
  .toLowerCase() !== "false";
const bridgeDir = process.env.SOCIAL_BRIDGE_DIR?.trim()
  || path.join(dataDir, "bridge");
const bridgePlatformBySource = {
  "youtube-scraper": "youtube",
  "tiktok-views-api": "tiktok",
  "instagram-views-api": "instagram",
};

if (!socialProxyKey) {
  throw new Error("SOCIAL_PROXY_KEY est requis");
}
if (!socialPublicOrigin) {
  throw new Error("SOCIAL_PUBLIC_ORIGIN doit contenir l’origine HTTPS exacte de Switch développement");
}

function configuredOrigin(value) {
  try {
    const url = new URL(value);
    if (!new Set(["https:", "http:"]).has(url.protocol) || url.username || url.password) return null;
    return url.origin;
  } catch {
    return null;
  }
}

const providerConfiguration = {
  instagram: {
    clientId: process.env.INSTAGRAM_CLIENT_ID?.trim() ?? "",
    clientSecret: process.env.INSTAGRAM_CLIENT_SECRET?.trim() ?? "",
    redirectUri: process.env.INSTAGRAM_REDIRECT_URI?.trim() ?? "",
    authorizeUrl: process.env.INSTAGRAM_AUTHORIZE_URL?.trim()
      || "https://www.instagram.com/oauth/authorize",
    enableFacebookLogin: (process.env.INSTAGRAM_ENABLE_FB_LOGIN ?? "false")
      .trim().toLowerCase() === "true",
    forceAuthentication: (process.env.INSTAGRAM_FORCE_AUTHENTICATION ?? "true")
      .trim().toLowerCase() !== "false",
    tokenUrl: process.env.INSTAGRAM_TOKEN_URL?.trim()
      || "https://api.instagram.com/oauth/access_token",
    longTokenUrl: process.env.INSTAGRAM_LONG_TOKEN_URL?.trim()
      || "https://graph.instagram.com/access_token",
    graphUrl: (process.env.INSTAGRAM_GRAPH_URL?.trim()
      || "https://graph.instagram.com/v26.0").replace(/\/$/, ""),
    scopes: normalizeScopes([
      process.env.INSTAGRAM_SCOPES?.trim() || "",
      "instagram_business_basic",
      "instagram_business_manage_insights",
    ]).join(","),
    refreshUrl: process.env.INSTAGRAM_REFRESH_URL?.trim()
      || "https://graph.instagram.com/refresh_access_token",
  },
  tiktok: {
    clientKey: process.env.TIKTOK_CLIENT_KEY?.trim()
      || process.env.TIKTOK_CLIENT_ID?.trim()
      || "",
    clientSecret: process.env.TIKTOK_CLIENT_SECRET?.trim() ?? "",
    redirectUri: process.env.TIKTOK_REDIRECT_URI?.trim() ?? "",
    authorizeUrl: process.env.TIKTOK_AUTHORIZE_URL?.trim()
      || "https://www.tiktok.com/v2/auth/authorize/",
    tokenUrl: process.env.TIKTOK_TOKEN_URL?.trim()
      || "https://open.tiktokapis.com/v2/oauth/token/",
    refreshUrl: process.env.TIKTOK_REFRESH_URL?.trim()
      || "https://open.tiktokapis.com/v2/oauth/token/",
    revokeUrl: process.env.TIKTOK_REVOKE_URL?.trim()
      || "https://open.tiktokapis.com/v2/oauth/revoke/",
    apiUrl: (process.env.TIKTOK_API_URL?.trim()
      || "https://open.tiktokapis.com/v2").replace(/\/$/, ""),
    scopes: normalizeScopes([
      process.env.TIKTOK_SCOPES?.trim() || "",
      "user.info.basic",
      "user.info.profile",
      "video.list",
    ]).join(","),
  },
};

const emptyStore = () => ({
  version: 2,
  accounts: [],
  metrics: [],
  media: [],
  mediaSnapshots: [],
  oauthAttempts: [],
  demoSeededOwners: [],
});

let mutationQueue = Promise.resolve();
const accountSyncLocks = new Map();
const tokenEncryptionKey = tokenEncryptionSecret.length >= 32
  ? createHash("sha256").update(tokenEncryptionSecret).digest()
  : null;

async function loadStore() {
  await mkdir(dataDir, { recursive: true });
  try {
    const parsed = JSON.parse(await readFile(storePath, "utf8"));
    if (
      !parsed
      || ![1, 2].includes(parsed.version)
      || !Array.isArray(parsed.accounts)
      || !Array.isArray(parsed.metrics)
    ) {
      throw new Error("format de stockage invalide");
    }
    const migratedFromV1 = parsed.version === 1;
    if (migratedFromV1) {
      parsed.version = 2;
      if (!Array.isArray(parsed.media)) parsed.media = [];
      if (!Array.isArray(parsed.mediaSnapshots)) parsed.mediaSnapshots = [];
    }
    if (!Array.isArray(parsed.media) || !Array.isArray(parsed.mediaSnapshots)) {
      throw new Error("format de stockage invalide");
    }
    if (!Array.isArray(parsed.oauthAttempts)) parsed.oauthAttempts = [];
    if (!Array.isArray(parsed.demoSeededOwners)) parsed.demoSeededOwners = [];
    if (migratedFromV1) await saveStore(parsed);
    return parsed;
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
    return emptyStore();
  }
}

async function saveStore(store) {
  const temporaryPath = `${storePath}.${process.pid}.${Date.now()}.tmp`;
  await writeFile(temporaryPath, `${JSON.stringify(store, null, 2)}\n`, {
    encoding: "utf8",
    mode: 0o600,
  });
  await rename(temporaryPath, storePath);
}

async function mutateStore(mutator) {
  const operation = mutationQueue.then(async () => {
    const store = await loadStore();
    const result = await mutator(store);
    await saveStore(store);
    return result;
  });
  mutationQueue = operation.catch(() => undefined);
  return operation;
}

function providerIsConfigured(platform) {
  const provider = providerConfiguration[platform];
  return Boolean(
    (platform === "tiktok" ? provider?.clientKey : provider?.clientId)
    && provider.clientSecret
    && provider.redirectUri
    && tokenEncryptionKey
  );
}

function encryptCredentials(credentials) {
  if (!tokenEncryptionKey) throw new Error("Clé de chiffrement des jetons absente");
  const nonce = randomBytes(12);
  const cipher = createCipheriv("aes-256-gcm", tokenEncryptionKey, nonce);
  const plaintext = Buffer.from(JSON.stringify(credentials));
  const ciphertext = Buffer.concat([cipher.update(plaintext), cipher.final()]);
  return {
    version: 1,
    algorithm: "aes-256-gcm",
    nonce: nonce.toString("base64"),
    ciphertext: ciphertext.toString("base64"),
    tag: cipher.getAuthTag().toString("base64"),
  };
}

function decryptCredentials(encrypted) {
  if (!tokenEncryptionKey || encrypted?.algorithm !== "aes-256-gcm") {
    throw new Error("Jetons du compte illisibles");
  }
  const decipher = createDecipheriv(
    "aes-256-gcm",
    tokenEncryptionKey,
    Buffer.from(encrypted.nonce, "base64"),
  );
  decipher.setAuthTag(Buffer.from(encrypted.tag, "base64"));
  const plaintext = Buffer.concat([
    decipher.update(Buffer.from(encrypted.ciphertext, "base64")),
    decipher.final(),
  ]);
  return JSON.parse(plaintext.toString("utf8"));
}

function providerFailure(platform, detail = "") {
  const error = new Error(`La plateforme ${platform} n’a pas accepté la requête.`);
  error.status = 502;
  error.providerDetail = detail;
  return error;
}

async function fetchProviderJson(platform, url, options = {}) {
  let response;
  try {
    response = await fetch(url, {
      ...options,
      signal: AbortSignal.timeout(15_000),
    });
  } catch (error) {
    throw providerFailure(platform, error?.message);
  }
  let payload;
  const responseBody = await response.text();
  if (!responseBody.trim()) {
    if (response.ok) return {};
    throw providerFailure(platform, `HTTP ${response.status}`);
  }
  try {
    payload = JSON.parse(responseBody);
  } catch {
    throw providerFailure(platform, `HTTP ${response.status}`);
  }
  const tiktokTopLevelRejected = platform === "tiktok"
    && payload?.code != null
    && String(payload.code) !== "0"
    && String(payload.code).toLowerCase() !== "ok";
  const tiktokErrorObject = platform === "tiktok"
    && payload?.error
    && typeof payload.error === "object"
    ? payload.error
    : null;
  const tiktokErrorCode = tiktokErrorObject
    ? String(tiktokErrorObject.code ?? "").toLowerCase()
    : "";
  const tiktokNestedRejected = Boolean(tiktokErrorObject)
    && !["0", "ok"].includes(tiktokErrorCode);
  const genericRejected = platform === "tiktok"
    ? typeof payload?.error === "string"
    : Boolean(payload?.error);
  if (!response.ok || tiktokTopLevelRejected || tiktokNestedRejected || genericRejected) {
    const detail = payload?.message
      || payload?.error?.message
      || payload?.error_description
      || (typeof payload?.error === "string" ? payload.error : "")
      || `HTTP ${response.status}`;
    throw providerFailure(platform, String(detail).slice(0, 240));
  }
  return payload;
}

function numberHash(input) {
  const digest = createHash("sha256").update(input).digest();
  return digest.readUInt32BE(0);
}

function deterministicUnit(input) {
  return numberHash(input) / 0xffffffff;
}

function generatedViews(account, date, position) {
  const seed = account.metricSeed ?? numberHash(`${account.platform}:${account.handle}`);
  const base = account.platform === "tiktok"
    ? 26_000 + (seed % 22_000)
    : 10_000 + (seed % 17_000);
  const random = deterministicUnit(`${seed}:${date}`);
  const wave = 0.82 + Math.sin((position + (seed % 9)) / 3.15) * 0.14;
  const growth = 0.78 + position / 420;
  const spike = random > 0.86 ? 1.35 + random * 0.72 : 1;
  return Math.max(120, Math.round(base * wave * growth * (0.78 + random * 0.45) * spike));
}

const accountColours = ["#f37fd5", "#65e8df", "#f3c66e", "#9aa8ff", "#9ee37d", "#f58b77"];

function createDemoAccount(ownerUserId, platform, handle, displayName, colourIndex = 0) {
  const normalizedHandle = handle.replace(/^@/, "").toLowerCase();
  return {
    id: randomUUID(),
    ownerUserId,
    platform,
    handle: normalizedHandle,
    displayName: displayName.trim() || `@${normalizedHandle}`,
    colour: accountColours[colourIndex % accountColours.length],
    connectionMode: "demo",
    status: "connected",
    metricSeed: numberHash(`${ownerUserId}:${platform}:${normalizedHandle}`),
    createdAt: new Date().toISOString(),
    syncedAt: new Date().toISOString(),
  };
}

function appendGeneratedMetrics(store, account, count = 200) {
  const existingDates = new Set(
    store.metrics
      .filter((metric) => metric.accountId === account.id)
      .map((metric) => metric.date),
  );
  dateKeys(count).forEach((date, index) => {
    if (existingDates.has(date)) return;
    store.metrics.push({
      accountId: account.id,
      ownerUserId: account.ownerUserId,
      date,
      views: generatedViews(account, date, index),
      source: "demo",
      provisional: date === dayKey(),
    });
  });
}

function ensureDemoSeed(store, ownerUserId) {
  if (!demoMode || !demoSeedEnabled) return false;
  if (store.demoSeededOwners.includes(ownerUserId)) return false;
  if (store.accounts.some((account) => account.ownerUserId === ownerUserId)) {
    store.demoSeededOwners.push(ownerUserId);
    return true;
  }
  const accounts = [
    createDemoAccount(ownerUserId, "instagram", "atelier.noa", "Atelier Noa", 0),
    createDemoAccount(ownerUserId, "tiktok", "atelier.noa", "Atelier Noa", 1),
    createDemoAccount(ownerUserId, "instagram", "noa.studio", "Noa Studio", 2),
  ];
  accounts.forEach((account) => {
    store.accounts.push(account);
    appendGeneratedMetrics(store, account);
  });
  store.demoSeededOwners.push(ownerUserId);
  return true;
}

function publicAccount(account) {
  return {
    id: account.id,
    platform: account.platform,
    handle: account.handle,
    displayName: account.displayName,
    colour: account.colour,
    connectionMode: account.connectionMode,
    status: account.status,
    createdAt: account.createdAt,
    syncedAt: account.syncedAt,
  };
}

function securityHeaders(extra = {}) {
  return {
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
    "referrer-policy": "same-origin",
    "cross-origin-resource-policy": "same-origin",
    ...extra,
  };
}

function sendJson(response, status, payload) {
  const body = JSON.stringify(payload);
  response.writeHead(status, securityHeaders({
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(body),
  }));
  response.end(body);
}

function sendRedirect(response, location) {
  response.writeHead(302, securityHeaders({
    location,
    "content-length": "0",
  }));
  response.end();
}

function socialShellReturn(status, platform) {
  const query = new URLSearchParams({ switch_social: status, provider: platform });
  return `/?${query}#switch-social`;
}

async function readJsonBody(request) {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    if (size > 64 * 1024) throw Object.assign(new Error("corps trop volumineux"), { status: 413 });
    chunks.push(chunk);
  }
  if (!chunks.length) return {};
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    throw Object.assign(new Error("JSON invalide"), { status: 400 });
  }
}

async function authenticatedUser(request) {
  const cookie = request.headers.cookie;
  if (!cookie) return null;
  try {
    const response = await fetch(switchAuthUrl, {
      headers: { cookie, accept: "application/json" },
      signal: AbortSignal.timeout(5_000),
    });
    if (!response.ok) return null;
    const payload = await response.json();
    return payload?.user?.id ? payload.user : null;
  } catch {
    return null;
  }
}

function sessionCookie(request) {
  const rawCookie = request.headers.cookie;
  if (typeof rawCookie !== "string") return "";
  for (const item of rawCookie.split(";")) {
    const separator = item.indexOf("=");
    if (separator < 1 || item.slice(0, separator).trim() !== "cst_session") continue;
    return item.slice(separator + 1).trim();
  }
  return "";
}

function csrfToken(request, user) {
  const session = sessionCookie(request);
  if (!session || !user?.id) return "";
  return createHmac("sha256", socialProxyKey)
    .update(`switch-social-v1\0${user.id}\0${createHash("sha256").update(session).digest("base64url")}`)
    .digest("base64url");
}

function requireCsrf(request, user) {
  const supplied = request.headers[csrfHeader];
  const expected = csrfToken(request, user);
  const origin = typeof request.headers.origin === "string" ? request.headers.origin : "";
  const suppliedBuffer = typeof supplied === "string" ? Buffer.from(supplied) : Buffer.alloc(0);
  const expectedBuffer = Buffer.from(expected);
  if (
    origin !== socialPublicOrigin
    || !expected
    || suppliedBuffer.length !== expectedBuffer.length
    || !timingSafeEqual(suppliedBuffer, expectedBuffer)
  ) {
    throw Object.assign(new Error("requête refusée"), { status: 403 });
  }
}

function hasValidProxyKey(request) {
  const candidate = request.headers["x-social-proxy-key"];
  if (typeof candidate !== "string") return false;
  const expectedBuffer = Buffer.from(socialProxyKey);
  const candidateBuffer = Buffer.from(candidate);
  return expectedBuffer.length === candidateBuffer.length
    && timingSafeEqual(expectedBuffer, candidateBuffer);
}

function normalizePlatform(value) {
  return value === "instagram" || value === "tiktok" ? value : null;
}

function normalizeHandle(value) {
  if (typeof value !== "string") return null;
  const handle = value.trim().replace(/^@/, "").toLowerCase();
  return /^[a-z0-9._]{2,30}$/.test(handle) ? handle : null;
}

function accountIsVisible(account) {
  return demoMode || account.connectionMode !== "demo";
}

function sanitizeText(value, maximumLength = 240) {
  if (typeof value !== "string") return "";
  return value
    .replace(/[\u0000-\u001f\u007f-\u009f]/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, maximumLength);
}

function sanitizeHttpUrl(value) {
  if (typeof value !== "string" || !value) return null;
  try {
    const url = new URL(value);
    return url.protocol === "https:" ? url.toString() : null;
  } catch {
    return null;
  }
}

function normalizePublishedAt(value) {
  if (typeof value !== "string" && typeof value !== "number") return null;
  const parsed = typeof value === "number" || /^\d+$/.test(String(value))
    ? new Date(Number(value) * 1_000)
    : new Date(value);
  return Number.isFinite(parsed.getTime()) ? parsed.toISOString() : null;
}

function normalizeCumulativeViews(value) {
  if (typeof value !== "number" && typeof value !== "string") return null;
  if (typeof value === "string" && !value.trim()) return null;
  const number = Number(value);
  return Number.isSafeInteger(number) && number >= 0 ? number : null;
}

function normalizePaginationCursor(value) {
  if (value == null) return null;
  if (typeof value !== "string" && typeof value !== "number") return null;
  if (typeof value === "number" && !Number.isFinite(value)) return null;
  const cursor = String(value);
  return cursor.length > 0
    && cursor.length <= 2_048
    && !/[\u0000-\u001f\u007f-\u009f]/.test(cursor)
    ? cursor
    : null;
}

function oauthStateHash(state) {
  return createHash("sha256").update(state).digest("hex");
}

function createAuthorizationUrl(platform, state) {
  const provider = providerConfiguration[platform];
  const url = new URL(provider.authorizeUrl);
  if (platform === "instagram") {
    url.searchParams.set("client_id", provider.clientId);
    url.searchParams.set("enable_fb_login", provider.enableFacebookLogin ? "1" : "0");
    url.searchParams.set("force_authentication", provider.forceAuthentication ? "1" : "0");
  } else {
    url.searchParams.set("client_key", provider.clientKey);
  }
  url.searchParams.set("redirect_uri", provider.redirectUri);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("scope", provider.scopes);
  url.searchParams.set("state", state);
  return url.toString();
}

function normalizeScopes(value) {
  const values = Array.isArray(value) ? value : [value];
  return [...new Set(values
    .flatMap((item) => String(item ?? "").split(/[\s,]+/))
    .map((item) => item.trim())
    .filter(Boolean))];
}

function exactProviderIdentifier(platform, value, fieldName) {
  if (typeof value === "string" && value.trim()) return value.trim();
  if (typeof value === "number" && Number.isSafeInteger(value) && value >= 0) {
    return String(value);
  }
  throw providerFailure(platform, `${fieldName} absent ou non exact`);
}

function requireProviderScopes(platform, grantedValue, requiredValue) {
  const granted = normalizeScopes(grantedValue);
  const missing = normalizeScopes(requiredValue).filter((scope) => !granted.includes(scope));
  if (missing.length) {
    throw providerFailure(platform, `permissions requises non accordées : ${missing.join(", ")}`);
  }
  return granted.join(",");
}

function requireProviderText(platform, value, fieldName) {
  if (typeof value !== "string" || !value.trim()) {
    throw providerFailure(platform, `${fieldName} absent`);
  }
  return value.trim();
}

function requirePositiveSeconds(platform, value, fieldName) {
  const seconds = Number(value);
  if (!Number.isFinite(seconds) || seconds <= 0) {
    throw providerFailure(platform, `${fieldName} invalide`);
  }
  return seconds;
}

function requireBearerTokenType(platform, value) {
  if (typeof value !== "string" || value.toLowerCase() !== "bearer") {
    throw providerFailure(platform, "token_type invalide");
  }
}

async function exchangeInstagramCode(code) {
  const provider = providerConfiguration.instagram;
  const body = new FormData();
  body.set("client_id", provider.clientId);
  body.set("client_secret", provider.clientSecret);
  body.set("grant_type", "authorization_code");
  body.set("redirect_uri", provider.redirectUri);
  body.set("code", code);
  const payload = await fetchProviderJson("instagram", provider.tokenUrl, {
    method: "POST",
    body,
  });
  const shortToken = Array.isArray(payload?.data) ? payload.data[0] : payload?.data ?? payload;
  if (!shortToken?.access_token) {
    throw providerFailure("instagram", "access_token absent");
  }
  const scope = requireProviderScopes(
    "instagram",
    shortToken.permissions ?? shortToken.scope,
    provider.scopes,
  );

  const longUrl = new URL(provider.longTokenUrl);
  longUrl.searchParams.set("grant_type", "ig_exchange_token");
  longUrl.searchParams.set("client_secret", provider.clientSecret);
  longUrl.searchParams.set("access_token", shortToken.access_token);
  const longToken = await fetchProviderJson("instagram", longUrl);
  const accessToken = requireProviderText(
    "instagram",
    longToken?.access_token,
    "access_token longue durée",
  );
  const expiresIn = requirePositiveSeconds("instagram", longToken?.expires_in, "expires_in");
  return {
    accessToken,
    issuedAt: Date.now(),
    accessTokenExpiresAt: Date.now() + expiresIn * 1_000,
    scope,
  };
}

async function exchangeTikTokCode(code) {
  const provider = providerConfiguration.tiktok;
  const body = new URLSearchParams({
    client_key: provider.clientKey,
    client_secret: provider.clientSecret,
    code,
    grant_type: "authorization_code",
    redirect_uri: provider.redirectUri,
  });
  const payload = await fetchProviderJson("tiktok", provider.tokenUrl, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body,
  });
  const token = payload?.data ?? payload;
  const accessToken = requireProviderText("tiktok", token?.access_token, "access_token");
  const refreshToken = requireProviderText("tiktok", token?.refresh_token, "refresh_token");
  const externalAccountId = exactProviderIdentifier("tiktok", token?.open_id, "open_id");
  const expiresIn = requirePositiveSeconds("tiktok", token?.expires_in, "expires_in");
  const refreshExpiresIn = requirePositiveSeconds(
    "tiktok",
    token?.refresh_expires_in ?? token?.refresh_token_expires_in,
    "refresh_expires_in",
  );
  requireBearerTokenType("tiktok", token?.token_type);
  const scope = requireProviderScopes("tiktok", token.scope, provider.scopes);
  return {
    accessToken,
    issuedAt: Date.now(),
    accessTokenExpiresAt: Date.now() + expiresIn * 1_000,
    refreshToken,
    refreshTokenExpiresAt: Date.now() + refreshExpiresIn * 1_000,
    externalAccountId,
    scope,
  };
}

async function fetchInstagramProfile(credentials) {
  const provider = providerConfiguration.instagram;
  const url = new URL(`${provider.graphUrl}/me`);
  url.searchParams.set(
    "fields",
    "id,user_id,username,name,account_type,profile_picture_url,followers_count,follows_count,media_count",
  );
  const profile = await fetchProviderJson("instagram", url, {
    headers: { authorization: `Bearer ${credentials.accessToken}` },
  });
  const canonicalId = exactProviderIdentifier("instagram", profile.user_id, "user_id");
  const appScopedId = exactProviderIdentifier("instagram", profile.id, "id app-scoped");
  const handle = normalizeHandle(profile.username)
    || `instagram.${canonicalId.slice(-8).toLowerCase()}`;
  return {
    externalAccountId: canonicalId,
    appScopedAccountId: appScopedId,
    handle,
    displayName: sanitizeText(String(profile.name || `@${handle}`), 80),
  };
}

function tiktokProfileObject(payload) {
  const data = payload?.data ?? payload ?? {};
  return data.business
    || data.user
    || data.profile
    || (Array.isArray(data.businesses) ? data.businesses[0] : null)
    || data;
}

async function fetchTikTokAccountData(credentials, fields, range = null) {
  const provider = providerConfiguration.tiktok;
  const url = new URL(`${provider.apiUrl}/user/info/`);
  url.searchParams.set("fields", fields.join(","));
  return fetchProviderJson("tiktok", url, {
    headers: { authorization: `Bearer ${credentials.accessToken}` },
  });
}

async function fetchTikTokProfile(credentials) {
  const payload = await fetchTikTokAccountData(credentials, [
    "open_id",
    "union_id",
    "username",
    "display_name",
    "avatar_url",
  ]);
  const profile = tiktokProfileObject(payload);
  const id = exactProviderIdentifier("tiktok", profile.open_id, "open_id");
  if (id !== credentials.externalAccountId) {
    throw providerFailure("tiktok", "open_id du profil différent du jeton");
  }
  const handle = normalizeHandle(profile.username);
  if (!handle) throw providerFailure("tiktok", "username TikTok absent ou invalide");
  return {
    payload,
    profile,
    externalAccountId: id,
    unionId: profile.union_id ? String(profile.union_id) : null,
    handle,
    displayName: sanitizeText(String(profile.display_name || profile.nickname || `@${handle}`), 80),
  };
}

async function refreshTikTokCredentials(credentials) {
  if (!credentials.refreshToken) throw providerFailure("tiktok", "refresh_token absent");
  const provider = providerConfiguration.tiktok;
  const body = new URLSearchParams({
    client_key: provider.clientKey,
    client_secret: provider.clientSecret,
    grant_type: "refresh_token",
    refresh_token: credentials.refreshToken,
  });
  const payload = await fetchProviderJson("tiktok", provider.refreshUrl, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body,
  });
  const token = payload?.data ?? payload;
  const accessToken = requireProviderText(
    "tiktok",
    token?.access_token,
    "access_token après refresh",
  );
  const refreshToken = requireProviderText(
    "tiktok",
    token?.refresh_token,
    "refresh_token après refresh",
  );
  const expiresIn = requirePositiveSeconds("tiktok", token?.expires_in, "expires_in après refresh");
  const refreshExpiresIn = requirePositiveSeconds(
    "tiktok",
    token?.refresh_expires_in ?? token?.refresh_token_expires_in,
    "refresh_expires_in après refresh",
  );
  requireBearerTokenType("tiktok", token?.token_type);
  if (!token?.open_id || String(token.open_id) !== credentials.externalAccountId) {
    throw providerFailure("tiktok", "open_id différent après refresh");
  }
  const scope = requireProviderScopes(
    "tiktok",
    token.scope,
    provider.scopes,
  );
  return {
    ...credentials,
    accessToken,
    issuedAt: Date.now(),
    accessTokenExpiresAt: Date.now() + expiresIn * 1_000,
    refreshToken,
    refreshTokenExpiresAt: Date.now() + refreshExpiresIn * 1_000,
    scope,
  };
}

async function revokeTikTokCredentials(credentials) {
  const provider = providerConfiguration.tiktok;
  const body = new URLSearchParams({
    client_key: provider.clientKey,
    client_secret: provider.clientSecret,
    token: credentials.accessToken,
  });
  await fetchProviderJson("tiktok", provider.revokeUrl, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body,
  });
}

async function refreshInstagramCredentials(credentials) {
  const provider = providerConfiguration.instagram;
  const url = new URL(provider.refreshUrl);
  url.searchParams.set("grant_type", "ig_refresh_token");
  url.searchParams.set("access_token", credentials.accessToken);
  const token = await fetchProviderJson("instagram", url);
  const accessToken = requireProviderText(
    "instagram",
    token?.access_token,
    "access_token après refresh",
  );
  const expiresIn = requirePositiveSeconds(
    "instagram",
    token?.expires_in,
    "expires_in après refresh",
  );
  return {
    ...credentials,
    accessToken,
    issuedAt: Date.now(),
    accessTokenExpiresAt: Date.now() + expiresIn * 1_000,
  };
}

function upsertMetric(store, account, date, views, provisional = false, source = "live") {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date) || !Number.isFinite(Number(views))) return;
  const normalizedViews = Math.max(0, Math.round(Number(views)));
  const existing = store.metrics.find(
    (metric) => metric.ownerUserId === account.ownerUserId
      && metric.accountId === account.id
      && metric.date === date,
  );
  if (existing) {
    existing.views = normalizedViews;
    existing.source = source;
    existing.provisional = provisional;
    existing.fetchedAt = new Date().toISOString();
    return;
  }
  store.metrics.push({
    accountId: account.id,
    ownerUserId: account.ownerUserId,
    date,
    views: normalizedViews,
    source,
    provisional,
    fetchedAt: new Date().toISOString(),
  });
}

function upsertProviderMedia(store, account, item, snapshotDate, capturedAt) {
  if (!item.externalMediaId) return null;
  let media = store.media.find((candidate) =>
    candidate.ownerUserId === account.ownerUserId
      && candidate.accountId === account.id
      && candidate.platform === account.platform
      && candidate.externalMediaId === item.externalMediaId
  );
  if (!media) {
    media = {
      id: randomUUID(),
      ownerUserId: account.ownerUserId,
      accountId: account.id,
      platform: account.platform,
      externalMediaId: item.externalMediaId,
      createdAt: capturedAt,
    };
    store.media.push(media);
  }
  media.title = sanitizeText(item.title, 240);
  media.permalink = sanitizeHttpUrl(item.permalink);
  media.thumbnailUrl = sanitizeHttpUrl(item.thumbnailUrl);
  media.publishedAt = item.publishedAt || media.publishedAt || null;
  media.lastSeenAt = capturedAt;
  media.lastSeenDate = snapshotDate;
  media.updatedAt = capturedAt;

  if (item.views == null) return media;
  let snapshot = store.mediaSnapshots.find((candidate) =>
    candidate.ownerUserId === account.ownerUserId
      && candidate.accountId === account.id
      && candidate.mediaId === media.id
      && candidate.date === snapshotDate
  );
  if (!snapshot) {
    snapshot = {
      id: randomUUID(),
      ownerUserId: account.ownerUserId,
      accountId: account.id,
      mediaId: media.id,
      date: snapshotDate,
      source: "live",
    };
    store.mediaSnapshots.push(snapshot);
  }
  snapshot.views = item.views;
  snapshot.capturedAt = capturedAt;
  return media;
}

function removeTikTokAccountMetric(store, account, date) {
  store.metrics = store.metrics.filter((metric) => !(
    metric.ownerUserId === account.ownerUserId
      && metric.accountId === account.id
      && metric.date === date
      && metric.source === "media_snapshots"
  ));
}

function snapshotPairRepresentsDay(previous, current) {
  const previousAt = Date.parse(previous?.capturedAt);
  const currentAt = Date.parse(current?.capturedAt);
  if (!Number.isFinite(previousAt) || !Number.isFinite(currentAt)) return false;
  const elapsed = currentAt - previousAt;
  return elapsed >= 18 * 60 * 60 * 1_000 && elapsed <= 30 * 60 * 60 * 1_000;
}

function deriveTikTokAccountMetric(
  store,
  account,
  date,
  syncedMediaIds,
  snapshottedMediaIds,
) {
  const previousDate = shiftedDayKey(date, -1);
  const currentByMedia = new Map(store.mediaSnapshots
    .filter((snapshot) =>
      snapshot.ownerUserId === account.ownerUserId
        && snapshot.accountId === account.id
        && snapshot.date === date
    )
    .map((snapshot) => [snapshot.mediaId, snapshot]));
  const previousByMedia = new Map(store.mediaSnapshots
    .filter((snapshot) =>
      snapshot.ownerUserId === account.ownerUserId
        && snapshot.accountId === account.id
        && snapshot.date === previousDate
    )
    .map((snapshot) => [snapshot.mediaId, snapshot]));
  const inventoryIsComparable = syncedMediaIds.size > 0
    && snapshottedMediaIds.size === syncedMediaIds.size
    && currentByMedia.size === syncedMediaIds.size
    && previousByMedia.size === syncedMediaIds.size
    && [...syncedMediaIds].every((mediaId) =>
      snapshottedMediaIds.has(mediaId)
        && currentByMedia.has(mediaId)
        && previousByMedia.has(mediaId)
    );
  if (!inventoryIsComparable) {
    removeTikTokAccountMetric(store, account, date);
    return false;
  }
  const deltas = [];
  for (const mediaId of syncedMediaIds) {
    const current = currentByMedia.get(mediaId);
    const previous = previousByMedia.get(mediaId);
    const currentViews = normalizeCumulativeViews(current?.views);
    const previousViews = normalizeCumulativeViews(previous?.views);
    if (
      currentViews == null
      || previousViews == null
      || !snapshotPairRepresentsDay(previous, current)
      || currentViews < previousViews
    ) {
      removeTikTokAccountMetric(store, account, date);
      return false;
    }
    deltas.push(currentViews - previousViews);
  }
  if (!deltas.length) {
    removeTikTokAccountMetric(store, account, date);
    return false;
  }
  upsertMetric(
    store,
    account,
    date,
    deltas.reduce((sum, value) => sum + value, 0),
    date === dayKey(),
    "media_snapshots",
  );
  return true;
}

async function connectLiveAccount(ownerUserId, platform, credentials) {
  const profile = platform === "instagram"
    ? await fetchInstagramProfile(credentials)
    : await fetchTikTokProfile(credentials);
  const verifiedCredentials = {
    ...credentials,
    externalAccountId: profile.externalAccountId,
    ...(profile.appScopedAccountId ? { appScopedAccountId: profile.appScopedAccountId } : {}),
  };
  const account = await mutateStore((store) => {
    const existing = store.accounts.find((candidate) =>
      candidate.ownerUserId === ownerUserId
      && candidate.platform === platform
      && (
        candidate.externalAccountId === profile.externalAccountId
        || (
          platform === "tiktok"
          && profile.unionId
          && candidate.unionId === profile.unionId
        )
        || (
          platform === "instagram"
          && profile.appScopedAccountId
          && (
            candidate.appScopedAccountId === profile.appScopedAccountId
            || candidate.externalAccountId === profile.appScopedAccountId
          )
        )
      )
    );
    const now = new Date().toISOString();
    if (existing) {
      const identityChanged = existing.externalAccountId !== profile.externalAccountId;
      if (identityChanged) {
        store.metrics = store.metrics.filter((metric) => !(
          metric.accountId === existing.id && metric.ownerUserId === ownerUserId
        ));
        store.media = store.media.filter((media) => !(
          media.accountId === existing.id && media.ownerUserId === ownerUserId
        ));
        store.mediaSnapshots = store.mediaSnapshots.filter(
          (snapshot) => !(
            snapshot.accountId === existing.id && snapshot.ownerUserId === ownerUserId
          ),
        );
        existing.syncedAt = null;
        delete existing.lastSyncAttemptAt;
        delete existing.lastSyncPartialAt;
        delete existing.lastSyncError;
      }
      existing.externalAccountId = profile.externalAccountId;
      if (profile.appScopedAccountId) existing.appScopedAccountId = profile.appScopedAccountId;
      if (profile.unionId) existing.unionId = profile.unionId;
      existing.handle = profile.handle;
      existing.displayName = profile.displayName;
      existing.encryptedCredentials = encryptCredentials(verifiedCredentials);
      existing.connectionMode = "live";
      existing.status = "syncing";
      existing.updatedAt = now;
      return publicAccount(existing);
    }
    const ownerAccounts = store.accounts.filter((candidate) => candidate.ownerUserId === ownerUserId);
    const created = {
      id: randomUUID(),
      ownerUserId,
      platform,
      externalAccountId: profile.externalAccountId,
      ...(profile.appScopedAccountId ? { appScopedAccountId: profile.appScopedAccountId } : {}),
      ...(profile.unionId ? { unionId: profile.unionId } : {}),
      handle: profile.handle,
      displayName: profile.displayName,
      colour: accountColours[ownerAccounts.length % accountColours.length],
      connectionMode: "live",
      status: "syncing",
      encryptedCredentials: encryptCredentials(verifiedCredentials),
      createdAt: now,
      syncedAt: null,
    };
    store.accounts.push(created);
    return publicAccount(created);
  });
  const providerLimit = platform === "tiktok" ? 60 : 90;
  void syncLiveAccount(
    ownerUserId,
    account.id,
    Math.min(initialSyncDays, providerLimit),
  ).catch(async (error) => {
    await mutateStore((store) => {
      const current = store.accounts.find((candidate) =>
        candidate.id === account.id && candidate.ownerUserId === ownerUserId
      );
      if (!current) return;
      current.status = "error";
      current.lastSyncError = new Date().toISOString();
    });
    console.warn(`Synchronisation initiale ${platform} différée: ${error.message}`);
  });
  return account;
}

async function syncInstagramAccount(account, credentials, days) {
  const dates = dateKeys(days);
  const collected = [];
  let partialFailures = 0;
  for (const date of dates) {
    try {
      const url = new URL(`${providerConfiguration.instagram.graphUrl}/${encodeURIComponent(account.externalAccountId)}/insights`);
      url.searchParams.set("metric", "views");
      url.searchParams.set("period", "day");
      url.searchParams.set("metric_type", "total_value");
      url.searchParams.set("since", unixDayBoundary(date));
      url.searchParams.set(
        "until",
        String(Number(unixDayBoundary(shiftedDayKey(date, 1))) - 1),
      );
      const payload = await fetchProviderJson("instagram", url, {
        headers: { authorization: `Bearer ${credentials.accessToken}` },
      });
      const metric = payload?.data?.find?.((candidate) => candidate.name === "views");
      const value = metric?.total_value?.value ?? metric?.values?.[0]?.value;
      const views = normalizeCumulativeViews(value);
      if (views == null) partialFailures += 1;
      else collected.push({ date, views });
    } catch {
      partialFailures += 1;
    }
  }
  if (!collected.length && partialFailures === dates.length) {
    throw new Error("Instagram Insights est temporairement indisponible pour toute la période demandée.");
  }
  return { metrics: collected, partialFailures };
}

function instagramPagingCursor(payload) {
  const cursor = normalizePaginationCursor(payload?.paging?.cursors?.after);
  if (cursor) return cursor;
  const next = payload?.paging?.next;
  if (!next) return null;
  try {
    const nextUrl = new URL(next);
    const configuredUrl = new URL(providerConfiguration.instagram.graphUrl);
    const configuredPath = configuredUrl.pathname.replace(/\/$/, "");
    if (
      nextUrl.protocol !== "https:"
      || nextUrl.origin !== configuredUrl.origin
      || !nextUrl.pathname.startsWith(`${configuredPath}/`)
    ) return null;
    return normalizePaginationCursor(nextUrl.searchParams.get("after"));
  } catch {
    return null;
  }
}

function instagramIsReel(media) {
  return String(media?.media_product_type ?? "").toUpperCase() === "REELS"
    || Object.prototype.hasOwnProperty.call(media ?? {}, "is_shared_to_feed");
}

async function fetchInstagramMediaInsights(mediaId, credentials) {
  const url = new URL(`${providerConfiguration.instagram.graphUrl}/${encodeURIComponent(mediaId)}/insights`);
  url.searchParams.set("metric", "views");
  const payload = await fetchProviderJson("instagram", url, {
    headers: { authorization: `Bearer ${credentials.accessToken}` },
  });
  const metric = payload?.data?.find?.((candidate) => candidate.name === "views");
  return normalizeCumulativeViews(metric?.total_value?.value ?? metric?.values?.[0]?.value);
}

async function fetchInstagramMediaPage(account, credentials, after) {
  const fields = [
    "id",
    "caption",
    "media_type",
    "is_shared_to_feed",
    "permalink",
    "thumbnail_url",
    "timestamp",
    "username",
  ];
  const url = new URL(`${providerConfiguration.instagram.graphUrl}/${encodeURIComponent(account.externalAccountId)}/media`);
  url.searchParams.set("fields", fields.join(","));
  url.searchParams.set("limit", "100");
  if (after) url.searchParams.set("after", after);
  return fetchProviderJson("instagram", url, {
    headers: { authorization: `Bearer ${credentials.accessToken}` },
  });
}

async function collectInstagramMedia(account, credentials) {
  const collected = new Map();
  const seenCursors = new Set();
  let after = null;
  let partialFailures = 0;
  for (let page = 0; page < maxPaginationPages; page += 1) {
    let payload;
    try {
      payload = await fetchInstagramMediaPage(account, credentials, after);
    } catch (error) {
      if (page === 0) throw error;
      partialFailures += 1;
      break;
    }
    const rows = Array.isArray(payload?.data) ? payload.data : [];
    for (const media of rows) {
      if (!instagramIsReel(media)) continue;
      const externalMediaId = String(media?.id ?? "");
      if (!externalMediaId) {
        partialFailures += 1;
        continue;
      }
      let views = null;
      try {
        views = await fetchInstagramMediaInsights(externalMediaId, credentials);
      } catch {
        partialFailures += 1;
      }
      if (views == null) partialFailures += 1;
      collected.set(externalMediaId, {
        externalMediaId,
        title: sanitizeText(media.caption, 240) || "Reel Instagram",
        permalink: sanitizeHttpUrl(media.permalink),
        thumbnailUrl: sanitizeHttpUrl(media.thumbnail_url),
        publishedAt: normalizePublishedAt(media.timestamp),
        views,
      });
    }
    const nextCursor = instagramPagingCursor(payload);
    if (!nextCursor) return { items: [...collected.values()], partialFailures };
    if (seenCursors.has(nextCursor)) {
      partialFailures += 1;
      return { items: [...collected.values()], partialFailures };
    }
    seenCursors.add(nextCursor);
    after = nextCursor;
  }
  return { items: [...collected.values()], partialFailures: partialFailures + 1 };
}

async function collectTikTokMedia(credentials) {
  const fields = [
    "id",
    "create_time",
    "title",
    "video_description",
    "share_url",
    "cover_image_url",
    "duration",
    "view_count",
    "like_count",
    "comment_count",
    "share_count",
  ];
  const collected = new Map();
  const seenCursors = new Set();
  let cursor = null;
  let partialFailures = 0;
  for (let page = 0; page < maxPaginationPages; page += 1) {
    const url = new URL(`${providerConfiguration.tiktok.apiUrl}/video/list/`);
    url.searchParams.set("fields", fields.join(","));
    const body = { max_count: 20 };
    if (cursor != null) body.cursor = cursor;
    let payload;
    try {
      payload = await fetchProviderJson("tiktok", url, {
        method: "POST",
        headers: {
          authorization: `Bearer ${credentials.accessToken}`,
          "content-type": "application/json",
        },
        body: JSON.stringify(body),
      });
    } catch (error) {
      if (page === 0) throw error;
      partialFailures += 1;
      break;
    }
    const rows = Array.isArray(payload?.data?.videos) ? payload.data.videos : [];
    for (const video of rows) {
      const externalMediaId = String(video?.id ?? "");
      if (!externalMediaId) {
        partialFailures += 1;
        continue;
      }
      const views = normalizeCumulativeViews(video.view_count);
      if (views == null) partialFailures += 1;
      collected.set(externalMediaId, {
        externalMediaId,
        title: sanitizeText(video.title || video.video_description, 240) || "Vidéo TikTok",
        permalink: sanitizeHttpUrl(video.share_url),
        thumbnailUrl: sanitizeHttpUrl(video.cover_image_url),
        publishedAt: normalizePublishedAt(video.create_time),
        views,
      });
    }
    if (payload?.data?.has_more !== true) {
      return { items: [...collected.values()], partialFailures, complete: true };
    }
    const nextCursor = payload?.data?.cursor;
    const cursorKey = normalizePaginationCursor(nextCursor) ?? "";
    if (!cursorKey || seenCursors.has(cursorKey)) {
      partialFailures += 1;
      return { items: [...collected.values()], partialFailures, complete: false };
    }
    seenCursors.add(cursorKey);
    cursor = nextCursor;
  }
  return { items: [...collected.values()], partialFailures: partialFailures + 1, complete: false };
}

async function syncLiveAccountUnlocked(ownerUserId, accountId, days = 3) {
  let account = await mutateStore((store) => {
    const found = store.accounts.find((candidate) =>
      candidate.id === accountId
      && candidate.ownerUserId === ownerUserId
      && candidate.connectionMode === "live"
    );
    return found ? structuredClone(found) : null;
  });
  if (!account?.encryptedCredentials) throw Object.assign(new Error("Compte officiel introuvable."), { status: 404 });
  const providerLimit = account.platform === "tiktok" ? 60 : 90;
  days = Math.max(1, Math.min(providerLimit, Number(days) || 3));
  let credentials = decryptCredentials(account.encryptedCredentials);
  const shouldRefreshTikTok = account.platform === "tiktok"
    && Number(credentials.accessTokenExpiresAt) < Date.now() + 5 * 60 * 1_000;
  const shouldRefreshInstagram = account.platform === "instagram"
    && Number(credentials.accessTokenExpiresAt) < Date.now() + 7 * 24 * 60 * 60 * 1_000;
  if (shouldRefreshTikTok || shouldRefreshInstagram) {
    credentials = shouldRefreshTikTok
      ? await refreshTikTokCredentials(credentials)
      : await refreshInstagramCredentials(credentials);
    account = await mutateStore((store) => {
      const current = store.accounts.find((candidate) => candidate.id === account.id);
      if (!current || current.ownerUserId !== ownerUserId) return null;
      current.encryptedCredentials = encryptCredentials(credentials);
      return structuredClone(current);
    });
  }
  let metricsResult = { metrics: [], partialFailures: 0 };
  let mediaResult = { items: [], partialFailures: 0, complete: false };
  if (account.platform === "instagram") {
    const [metricsAttempt, mediaAttempt] = await Promise.allSettled([
      syncInstagramAccount(account, credentials, days),
      collectInstagramMedia(account, credentials),
    ]);
    if (metricsAttempt.status === "fulfilled") metricsResult = metricsAttempt.value;
    else metricsResult.partialFailures += 1;
    if (mediaAttempt.status === "fulfilled") mediaResult = mediaAttempt.value;
    else mediaResult.partialFailures += 1;
    if (metricsAttempt.status === "rejected" && mediaAttempt.status === "rejected") {
      throw mediaAttempt.reason;
    }
  } else {
    mediaResult = await collectTikTokMedia(credentials);
  }
  const capturedAt = new Date().toISOString();
  const snapshotDate = dayKey(new Date(capturedAt));
  await mutateStore((store) => {
    const current = store.accounts.find((candidate) =>
      candidate.id === account.id && candidate.ownerUserId === ownerUserId
    );
    if (!current) return;
    metricsResult.metrics.forEach((metric) => upsertMetric(store, current, metric.date, metric.views, false));
    const syncedMediaIds = new Set();
    const snapshottedMediaIds = new Set();
    mediaResult.items.forEach((item) => {
      const media = upsertProviderMedia(store, current, item, snapshotDate, capturedAt);
      if (!media) return;
      syncedMediaIds.add(media.id);
      if (normalizeCumulativeViews(item.views) != null) snapshottedMediaIds.add(media.id);
    });
    if (current.platform === "tiktok") {
      if (mediaResult.complete === true && mediaResult.partialFailures === 0) {
        deriveTikTokAccountMetric(
          store,
          current,
          snapshotDate,
          syncedMediaIds,
          snapshottedMediaIds,
        );
      } else {
        removeTikTokAccountMetric(store, current, snapshotDate);
      }
    }
    const partialFailures = metricsResult.partialFailures + mediaResult.partialFailures;
    current.status = metricsResult.metrics.length || mediaResult.items.length ? "connected" : "no_data";
    current.lastSyncAttemptAt = capturedAt;
    const syncWatermarkSafe = current.platform === "instagram"
      ? metricsResult.partialFailures === 0
      : mediaResult.complete === true && mediaResult.partialFailures === 0;
    if (syncWatermarkSafe) current.syncedAt = capturedAt;
    if (partialFailures) current.lastSyncPartialAt = capturedAt;
    else delete current.lastSyncPartialAt;
    delete current.lastSyncError;
  });
  return {
    metrics: metricsResult.metrics.length,
    media: mediaResult.items.length,
    partial: metricsResult.partialFailures + mediaResult.partialFailures > 0,
  };
}

async function syncLiveAccount(ownerUserId, accountId, days = 3) {
  const lockKey = `${ownerUserId}:${accountId}`;
  const existing = accountSyncLocks.get(lockKey);
  if (existing) return existing;
  const operation = syncLiveAccountUnlocked(ownerUserId, accountId, days);
  accountSyncLocks.set(lockKey, operation);
  try {
    return await operation;
  } finally {
    if (accountSyncLocks.get(lockKey) === operation) accountSyncLocks.delete(lockKey);
  }
}

function catchUpDays(account, overlapDays = 4) {
  const providerLimit = account.platform === "tiktok" ? 60 : 90;
  if (!account.syncedAt) return Math.min(initialSyncDays, providerLimit);
  const elapsedMilliseconds = Date.now() - Date.parse(account.syncedAt);
  const elapsedDays = Number.isFinite(elapsedMilliseconds)
    ? Math.max(0, Math.ceil(elapsedMilliseconds / 86_400_000))
    : initialSyncDays;
  return Math.min(providerLimit, Math.max(overlapDays, elapsedDays + overlapDays));
}

let scheduledSyncRunning = false;

async function runScheduledSync() {
  if (scheduledSyncRunning) return;
  scheduledSyncRunning = true;
  try {
    const accounts = await mutateStore((store) => store.accounts
      .filter((account) => account.connectionMode === "live")
      .map((account) => ({
        id: account.id,
        ownerUserId: account.ownerUserId,
        platform: account.platform,
        syncedAt: account.syncedAt,
      })));
    for (const account of accounts) {
      try {
        await syncLiveAccount(account.ownerUserId, account.id, catchUpDays(account));
      } catch (error) {
        await mutateStore((store) => {
          const current = store.accounts.find((candidate) => candidate.id === account.id);
          if (!current || current.ownerUserId !== account.ownerUserId) return;
          current.status = "error";
          current.lastSyncError = new Date().toISOString();
        });
        console.warn(`Synchronisation planifiée différée pour ${account.id}: ${error.message}`);
      }
    }
  } finally {
    scheduledSyncRunning = false;
  }
}

function configPayload(request, user) {
  return {
    mode: demoMode ? "demo" : "live",
    demoMode,
    timezone,
    csrfToken: csrfToken(request, user),
    providers: {
      instagram: {
        configured: providerIsConfigured("instagram"),
        connection: "oauth",
        scope: "Comptes professionnels Business ou Creator",
        authorizeHost: new URL(providerConfiguration.instagram.authorizeUrl).hostname,
      },
      tiktok: {
        configured: providerIsConfigured("tiktok"),
        connection: "oauth",
        scope: "TikTok Login Kit et Display API v2 · vidéos publiques",
        authorizeHost: new URL(providerConfiguration.tiktok.authorizeUrl).hostname,
      },
      youtube: {
        configured: true,
        connection: "bridge-download",
        scope: "Chaînes publiques via le pont local (yt-dlp + événements JSON)",
        authorizeHost: null,
      },
    },
    metricNotes: {
      instagram: "Les vues journalières réelles nécessitent un compte professionnel et l’accès Insights.",
      tiktok: "Les vues vidéo sont cumulatives ; les vues journalières exigent deux snapshots quotidiens adjacents.",
    },
  };
}

function bridgeUploadDateToIso(value) {
  if (typeof value !== "string") return null;
  const match = value.match(/^(\d{4})(\d{2})(\d{2})$/);
  if (!match) return null;
  const parsed = new Date(`${match[1]}-${match[2]}-${match[3]}T12:00:00.000Z`);
  return Number.isFinite(parsed.getTime()) ? parsed.toISOString() : null;
}

const bridgeCache = { loadedAt: 0, events: null };

/**
 * Lit les événements JSON immuables du pont local (`media-views-*.json`,
 * schéma switch.social.media_views/1) écrits par les services hôte
 * (youtube-scraper, tiktok-views-api, …). Ne garde que l'événement le plus
 * récent par plateforme+compte. Cache en mémoire 60 s.
 */
async function loadBridgeEvents() {
  const now = Date.now();
  if (bridgeCache.events && now - bridgeCache.loadedAt < 60_000) {
    return bridgeCache.events;
  }
  const latestByAccount = new Map();
  let names = [];
  try {
    names = await readdir(bridgeDir);
  } catch {
    bridgeCache.events = latestByAccount;
    bridgeCache.loadedAt = now;
    return latestByAccount;
  }
  for (const name of names) {
    if (!name.startsWith("media-views-") || !name.endsWith(".json")) continue;
    let event;
    try {
      event = JSON.parse(await readFile(path.join(bridgeDir, name), "utf8"));
    } catch {
      continue;
    }
    if (event?.schema !== "switch.social.media_views/1" || !Array.isArray(event.videos)) continue;
    const source = String(event.source ?? "");
    const platform = bridgePlatformBySource[source] || event?.account?.platform;
    if (!["instagram", "tiktok", "youtube"].includes(platform)) continue;
    const handle = String(event.account?.handle ?? "").trim().toLowerCase();
    if (!handle) continue;
    const key = `${platform}:${handle}`;
    const previous = latestByAccount.get(key);
    if (
      previous
      && String(previous.event.generatedAt ?? "")
        .localeCompare(String(event.generatedAt ?? "")) > 0
    ) {
      continue;
    }
    latestByAccount.set(key, { platform, handle, event });
  }
  bridgeCache.events = latestByAccount;
  bridgeCache.loadedAt = now;
  return latestByAccount;
}

async function handleApi(request, response, url) {
  const user = await authenticatedUser(request);
  if (!user) {
    sendJson(response, 401, { ok: false, error: "Session Switch requise." });
    return;
  }

  const route = url.pathname.slice("/api/social".length) || "/";

  if (request.method === "GET" && route === "/config") {
    await mutateStore((store) => ensureDemoSeed(store, user.id));
    sendJson(response, 200, { ok: true, ...configPayload(request, user) });
    return;
  }

  if (request.method === "GET" && route === "/accounts") {
    const accounts = await mutateStore((store) => {
      ensureDemoSeed(store, user.id);
      return store.accounts
        .filter((account) => account.ownerUserId === user.id && accountIsVisible(account))
        .map(publicAccount);
    });
    sendJson(response, 200, { ok: true, accounts });
    return;
  }

  if (request.method === "POST" && route === "/accounts/demo") {
    requireCsrf(request, user);
    if (!demoMode) throw Object.assign(new Error("Le mode démonstration est désactivé."), { status: 403 });
    const body = await readJsonBody(request);
    const platform = normalizePlatform(body.platform);
    const handle = normalizeHandle(body.handle);
    const displayName = typeof body.displayName === "string"
      ? body.displayName.trim().slice(0, 80)
      : "";
    if (!platform || !handle) {
      throw Object.assign(new Error("Plateforme ou identifiant invalide."), { status: 400 });
    }
    const account = await mutateStore((store) => {
      const duplicate = store.accounts.some((candidate) =>
        candidate.ownerUserId === user.id
        && candidate.platform === platform
        && candidate.handle === handle
      );
      if (duplicate) throw Object.assign(new Error("Ce compte est déjà connecté."), { status: 409 });
      const ownerAccounts = store.accounts.filter((candidate) => candidate.ownerUserId === user.id);
      if (ownerAccounts.filter((candidate) => candidate.connectionMode === "demo").length >= maxDemoAccountsPerOwner) {
        throw Object.assign(new Error(`Maximum ${maxDemoAccountsPerOwner} comptes test par utilisateur.`), { status: 429 });
      }
      const created = createDemoAccount(
        user.id,
        platform,
        handle,
        displayName || `@${handle}`,
        ownerAccounts.length,
      );
      store.accounts.push(created);
      appendGeneratedMetrics(store, created);
      return publicAccount(created);
    });
    sendJson(response, 201, { ok: true, account });
    return;
  }

  const accountMatch = route.match(/^\/accounts\/([0-9a-f-]+)$/i);
  if (request.method === "DELETE" && accountMatch) {
    requireCsrf(request, user);
    const accountId = accountMatch[1];
    const pendingSync = accountSyncLocks.get(`${user.id}:${accountId}`);
    if (pendingSync) await pendingSync.catch(() => {});
    const targetAccount = await mutateStore((store) => {
      const account = store.accounts.find(
        (candidate) => candidate.id === accountId
          && candidate.ownerUserId === user.id
          && accountIsVisible(candidate),
      );
      return account ? structuredClone(account) : null;
    });
    if (!targetAccount) {
      sendJson(response, 404, { ok: false, error: "Compte introuvable." });
      return;
    }
    if (
      targetAccount.connectionMode === "live"
      && targetAccount.platform === "tiktok"
      && targetAccount.encryptedCredentials
    ) {
      try {
        await revokeTikTokCredentials(decryptCredentials(targetAccount.encryptedCredentials));
      } catch (error) {
        console.warn(`Révocation TikTok refusée pour ${accountId}: ${error.message}`);
        throw Object.assign(
          new Error("TikTok n’a pas confirmé la déconnexion. Réessayez avant de supprimer ce compte."),
          { status: 502 },
        );
      }
    }
    const removed = await mutateStore((store) => {
      const index = store.accounts.findIndex(
        (account) => account.id === accountId && account.ownerUserId === user.id,
      );
      if (index < 0) return false;
      store.accounts.splice(index, 1);
      store.metrics = store.metrics.filter(
        (metric) => !(metric.accountId === accountId && metric.ownerUserId === user.id),
      );
      store.media = store.media.filter(
        (media) => !(media.accountId === accountId && media.ownerUserId === user.id),
      );
      store.mediaSnapshots = store.mediaSnapshots.filter((snapshot) => !(
        snapshot.accountId === accountId
        && snapshot.ownerUserId === user.id
      ));
      return true;
    });
    if (!removed) throw Object.assign(new Error("Compte introuvable."), { status: 404 });
    sendJson(response, 200, { ok: true });
    return;
  }

  if (request.method === "GET" && route === "/metrics") {
    const requestedRange = Number.parseInt(url.searchParams.get("range") ?? "7", 10);
    const range = allowedRanges.has(requestedRange) ? requestedRange : 7;
    const requestedAccountIds = new Set(
      (url.searchParams.get("accounts") ?? "")
        .split(",")
        .map((value) => value.trim())
        .filter(Boolean),
    );
    const result = await mutateStore((store) => {
      ensureDemoSeed(store, user.id);
      const accounts = store.accounts.filter((account) =>
        account.ownerUserId === user.id
        && accountIsVisible(account)
        && (!requestedAccountIds.size || requestedAccountIds.has(account.id)),
      );
      accounts.forEach((account) => {
        if (account.connectionMode === "demo") appendGeneratedMetrics(store, account);
      });
      const allDates = dateKeys(range * 2);
      const currentDates = allDates.slice(range);
      const previousDates = allDates.slice(0, range);
      const wantedDates = new Set(allDates);
      const accountIds = new Set(accounts.map((account) => account.id));
      const metrics = store.metrics.filter((metric) =>
        metric.ownerUserId === user.id
        && accountIds.has(metric.accountId)
        && wantedDates.has(metric.date),
      );
      const lookup = new Map(metrics.map((metric) => [`${metric.accountId}:${metric.date}`, metric]));
      const series = accounts.map((account) => ({
        account: publicAccount(account),
        points: currentDates.map((date) => {
          const metric = lookup.get(`${account.id}:${date}`);
          return {
            date,
            views: metric ? metric.views : null,
            available: Boolean(metric),
            provisional: Boolean(metric && metric.date === dayKey()),
          };
        }),
      }));
      const currentTotal = series.reduce(
        (total, item) => total + item.points.reduce(
          (sum, point) => sum + (Number.isFinite(point.views) ? point.views : 0),
          0,
        ),
        0,
      );
      const rawPreviousTotal = accounts.reduce(
        (total, account) => total + previousDates.reduce(
          (sum, date) => sum + (lookup.get(`${account.id}:${date}`)?.views ?? 0),
          0,
        ),
        0,
      );
      const comparisonAvailable = accounts.length > 0 && accounts.every((account) =>
        previousDates.every((date) => lookup.has(`${account.id}:${date}`)),
      );
      const missingPoints = series.reduce(
        (total, item) => total + item.points.filter((point) => !point.available).length,
        0,
      );
      return {
        range,
        timezone,
        dates: currentDates,
        series,
        currentTotal,
        previousTotal: comparisonAvailable ? rawPreviousTotal : null,
        comparisonAvailable,
        missingPoints,
        generatedAt: new Date().toISOString(),
      };
    });
    sendJson(response, 200, { ok: true, ...result });
    return;
  }

  if (request.method === "GET" && route === "/media") {
    const requestedRange = Number.parseInt(url.searchParams.get("range") ?? "7", 10);
    const range = allowedRanges.has(requestedRange) ? requestedRange : 7;
    const requestedAccountIds = new Set(
      (url.searchParams.get("accounts") ?? "")
        .split(",")
        .map((value) => value.trim())
        .filter(Boolean),
    );
    const result = await mutateStore((store) => {
      ensureDemoSeed(store, user.id);
      const accounts = store.accounts.filter((account) =>
        account.ownerUserId === user.id
        && accountIsVisible(account)
        && (!requestedAccountIds.size || requestedAccountIds.has(account.id)),
      );
      const accountIds = new Set(accounts.map((account) => account.id));
      const media = store.media.filter((item) =>
        item.ownerUserId === user.id && accountIds.has(item.accountId),
      );
      const mediaIds = new Set(media.map((item) => item.id));
      const snapshots = store.mediaSnapshots.filter((snapshot) =>
        snapshot.ownerUserId === user.id
        && accountIds.has(snapshot.accountId)
        && mediaIds.has(snapshot.mediaId)
        && /^\d{4}-\d{2}-\d{2}$/.test(snapshot.date)
        && normalizeCumulativeViews(snapshot.views) != null,
      );
      const snapshotsByMedia = new Map();
      snapshots.forEach((snapshot) => {
        if (!snapshotsByMedia.has(snapshot.mediaId)) snapshotsByMedia.set(snapshot.mediaId, []);
        snapshotsByMedia.get(snapshot.mediaId).push(snapshot);
      });
      const dates = dateKeys(range);
      const items = media.map((item) => {
        const itemSnapshots = snapshotsByMedia.get(item.id) ?? [];
        const byDate = new Map();
        itemSnapshots.forEach((snapshot) => {
          const existing = byDate.get(snapshot.date);
          if (!existing || String(snapshot.capturedAt ?? "") > String(existing.capturedAt ?? "")) {
            byDate.set(snapshot.date, snapshot);
          }
        });
        const latest = [...byDate.values()].sort((left, right) =>
          String(right.date).localeCompare(String(left.date))
            || String(right.capturedAt ?? "").localeCompare(String(left.capturedAt ?? ""))
        )[0] ?? null;
        const points = dates.map((date) => {
          const current = byDate.get(date);
          const previous = byDate.get(shiftedDayKey(date, -1));
          const available = Boolean(
            current
              && previous
              && snapshotPairRepresentsDay(previous, current)
              && current.views >= previous.views
          );
          return {
            date,
            views: available ? current.views - previous.views : null,
            available,
          };
        });
        const availablePoints = points.filter((point) => point.available);
        return {
          id: item.id,
          accountId: item.accountId,
          platform: item.platform,
          externalMediaId: sanitizeText(String(item.externalMediaId ?? ""), 128),
          title: sanitizeText(item.title, 240),
          permalink: sanitizeHttpUrl(item.permalink),
          publishedAt: normalizePublishedAt(item.publishedAt),
          latestViews: latest ? latest.views : null,
          latestViewsAt: latest ? latest.capturedAt || latest.date : null,
          periodViews: availablePoints.length
            ? availablePoints.reduce((sum, point) => sum + point.views, 0)
            : null,
          periodComplete: points.every((point) => point.available),
          points,
        };
      }).sort((left, right) =>
        String(right.publishedAt ?? "").localeCompare(String(left.publishedAt ?? ""))
          || left.externalMediaId.localeCompare(right.externalMediaId)
      );
      return {
        range,
        timezone,
        dates,
        items,
        missingPoints: items.reduce(
          (total, item) => total + item.points.filter((point) => !point.available).length,
          0,
        ),
        generatedAt: new Date().toISOString(),
      };
    });
    sendJson(response, 200, { ok: true, ...result });
    return;
  }

  if (request.method === "GET" && route === "/top-videos") {
    const requestedPlatforms = new Set(
      (url.searchParams.get("platforms") ?? "all")
        .split(",")
        .map((value) => value.trim())
        .filter(Boolean),
    );
    const platformsFilter = requestedPlatforms.has("all")
      ? null
      : requestedPlatforms;
    const limit = Math.max(1, Math.min(20, Number.parseInt(
      url.searchParams.get("limit") ?? "8",
      10,
    ) || 8));
    const requestedAccountIds = new Set(
      (url.searchParams.get("accounts") ?? "")
        .split(",")
        .map((value) => value.trim())
        .filter(Boolean),
    );
    const officialEntries = await mutateStore((store) => {
      ensureDemoSeed(store, user.id);
      const accounts = store.accounts.filter((account) =>
        account.ownerUserId === user.id
        && accountIsVisible(account)
        && (!platformsFilter || platformsFilter.has(account.platform))
        && (!requestedAccountIds.size || requestedAccountIds.has(account.id)),
      );
      const accountIds = new Set(accounts.map((account) => account.id));
      const byId = new Map(accounts.map((account) => [account.id, account]));
      const media = store.media.filter((item) =>
        item.ownerUserId === user.id
        && accountIds.has(item.accountId)
        && ["instagram", "tiktok"].includes(item.platform),
      );
      const mediaIds = new Set(media.map((item) => item.id));
      const latestByMedia = new Map();
      store.mediaSnapshots.forEach((snapshot) => {
        if (
          snapshot.ownerUserId !== user.id
          || !accountIds.has(snapshot.accountId)
          || !mediaIds.has(snapshot.mediaId)
          || !/^\d{4}-\d{2}-\d{2}$/.test(snapshot.date)
          || normalizeCumulativeViews(snapshot.views) == null
        ) {
          return;
        }
        const previous = latestByMedia.get(snapshot.mediaId);
        if (
          !previous
          || String(snapshot.date).localeCompare(String(previous.date)) > 0
          || (
            snapshot.date === previous.date
            && String(snapshot.capturedAt ?? "")
              .localeCompare(String(previous.capturedAt ?? "")) > 0
          )
        ) {
          latestByMedia.set(snapshot.mediaId, snapshot);
        }
      });
      const entries = [];
      media.forEach((item) => {
        const account = byId.get(item.accountId);
        const latest = latestByMedia.get(item.id) ?? null;
        const views = latest ? normalizeCumulativeViews(latest.views) : null;
        if (views == null) return;
        entries.push({
          platform: item.platform,
          accountHandle: account?.handle ?? "",
          accountDisplayName: account?.displayName ?? "",
          externalId: sanitizeText(String(item.externalMediaId ?? ""), 128),
          title: sanitizeText(item.title, 240),
          views,
          thumbnailUrl: sanitizeHttpUrl(item.thumbnailUrl),
          permalink: sanitizeHttpUrl(item.permalink),
          publishedAt: normalizePublishedAt(item.publishedAt),
          source: "official",
        });
      });
      return entries;
    });
    // Fusion avec les événements du pont local (YouTube et autres plateformes
    // publiques) : le compteur officiel prime en cas de doublon.
    const byKey = new Map();
    officialEntries.forEach((entry) => {
      byKey.set(`${entry.platform}:${entry.externalId}`, entry);
    });
    const bridgeAccounts = await loadBridgeEvents();
    for (const { platform, handle, event } of bridgeAccounts.values()) {
      if (platformsFilter && !platformsFilter.has(platform)) continue;
      for (const video of event.videos) {
        const externalId = sanitizeText(String(video?.id ?? ""), 128);
        if (!externalId) continue;
        const key = `${platform}:${externalId}`;
        if (byKey.has(key)) continue;
        const views = normalizeCumulativeViews(video?.views);
        if (views == null) continue;
        byKey.set(key, {
          platform,
          accountHandle: handle,
          accountDisplayName: handle,
          externalId,
          title: sanitizeText(video?.title ?? video?.desc, 240)
            || (platform === "youtube" ? "Vidéo YouTube" : "Vidéo sans titre"),
          views,
          thumbnailUrl: sanitizeHttpUrl(video?.thumbnail ?? video?.thumbnailUrl),
          permalink: sanitizeHttpUrl(video?.url),
          publishedAt: bridgeUploadDateToIso(video?.uploadDate),
          source: String(event.source ?? "bridge"),
        });
      }
    }
    const items = [...byKey.values()]
      .filter((entry) => Number.isFinite(entry.views))
      .sort((left, right) => (right.views ?? 0) - (left.views ?? 0))
      .slice(0, limit);
    sendJson(response, 200, {
      ok: true,
      platforms: [...new Set(items.map((item) => item.platform))],
      items,
      generatedAt: new Date().toISOString(),
    });
    return;
  }

  if (request.method === "POST" && route === "/sync") {
    requireCsrf(request, user);
    const liveAccounts = await mutateStore((store) => {
      ensureDemoSeed(store, user.id);
      const today = dayKey();
      store.accounts
        .filter((account) => account.ownerUserId === user.id && accountIsVisible(account))
        .forEach((account) => {
          if (account.connectionMode !== "demo") return;
          account.syncedAt = new Date().toISOString();
          const metric = store.metrics.find(
            (candidate) => candidate.accountId === account.id && candidate.date === today,
          );
          if (metric) {
            const bump = 37 + (numberHash(`${account.id}:${Date.now() >> 14}`) % 480);
            metric.views += bump;
            metric.provisional = true;
          } else {
            appendGeneratedMetrics(store, account);
          }
        });
      return store.accounts
        .filter((account) =>
          account.ownerUserId === user.id
          && accountIsVisible(account)
          && account.connectionMode === "live"
        )
        .map((account) => ({
          id: account.id,
          platform: account.platform,
          syncedAt: account.syncedAt,
        }));
    });
    const syncErrors = [];
    const partialSyncs = [];
    for (const account of liveAccounts) {
      try {
        const syncResult = await syncLiveAccount(user.id, account.id, catchUpDays(account));
        if (syncResult?.partial) partialSyncs.push(account.id);
      } catch (error) {
        syncErrors.push(account.id);
        await mutateStore((store) => {
          const current = store.accounts.find((candidate) =>
            candidate.id === account.id && candidate.ownerUserId === user.id
          );
          if (!current) return;
          current.status = "error";
          current.lastSyncError = new Date().toISOString();
        });
        console.warn(`Synchronisation sociale différée pour ${account.id}: ${error.message}`);
      }
    }
    const accounts = await mutateStore((store) => store.accounts
        .filter((account) => account.ownerUserId === user.id && accountIsVisible(account))
        .map(publicAccount));
    sendJson(response, 200, {
      ok: true,
      accounts,
      syncedAt: new Date().toISOString(),
      partial: syncErrors.length > 0 || partialSyncs.length > 0,
      failedAccounts: syncErrors.length,
      partialAccounts: partialSyncs.length,
    });
    return;
  }

  const oauthStartMatch = route.match(/^\/oauth\/(instagram|tiktok)\/start$/);
  if (request.method === "POST" && oauthStartMatch) {
    requireCsrf(request, user);
    const platform = oauthStartMatch[1];
    if (!providerIsConfigured(platform)) {
      sendJson(response, 503, {
        ok: false,
        error: `Connexion ${platform} non configurée : identifiants, URL de retour et clé de chiffrement requis.`,
        code: "provider_not_configured",
      });
      return;
    }
    const state = randomBytes(32).toString("base64url");
    const expiresAt = Date.now() + oauthTtlMilliseconds;
    await mutateStore((store) => {
      const now = Date.now();
      store.oauthAttempts = store.oauthAttempts.filter((attempt) => attempt.expiresAt > now);
      const recentOwnerAttempts = store.oauthAttempts
        .filter((attempt) => attempt.ownerUserId === user.id)
        .sort((left, right) => left.createdAt - right.createdAt)
        .slice(-4);
      const retainedOwnerStates = new Set(recentOwnerAttempts.map((attempt) => attempt.stateHash));
      store.oauthAttempts = store.oauthAttempts.filter((attempt) =>
        attempt.ownerUserId !== user.id || retainedOwnerStates.has(attempt.stateHash)
      );
      store.oauthAttempts.push({
        stateHash: oauthStateHash(state),
        ownerUserId: user.id,
        platform,
        createdAt: now,
        expiresAt,
      });
    });
    sendJson(response, 200, {
      ok: true,
      authorizeUrl: createAuthorizationUrl(platform, state),
      expiresAt: new Date(expiresAt).toISOString(),
    });
    return;
  }

  const oauthCallbackMatch = route.match(/^\/oauth\/(instagram|tiktok)\/callback$/);
  if (request.method === "GET" && oauthCallbackMatch) {
    const platform = oauthCallbackMatch[1];
    const providerErrorCode = url.searchParams.get("error")
      || url.searchParams.get("error_code")
      || url.searchParams.get("error_reason");
    const state = url.searchParams.get("state") ?? "";
    const code = url.searchParams.get("code") || url.searchParams.get("auth_code") || "";
    const validAttempt = await mutateStore((store) => {
      const hash = oauthStateHash(state);
      const now = Date.now();
      const index = store.oauthAttempts.findIndex((attempt) =>
        attempt.stateHash === hash
        && attempt.ownerUserId === user.id
        && attempt.platform === platform
        && attempt.expiresAt > now
      );
      if (index < 0) {
        store.oauthAttempts = store.oauthAttempts.filter((attempt) => attempt.expiresAt > now);
        return false;
      }
      store.oauthAttempts.splice(index, 1);
      return true;
    });
    if (!validAttempt || providerErrorCode || !code) {
      sendRedirect(response, socialShellReturn("error", platform));
      return;
    }
    try {
      const credentials = platform === "instagram"
        ? await exchangeInstagramCode(code)
        : await exchangeTikTokCode(code);
      await connectLiveAccount(user.id, platform, credentials);
      sendRedirect(response, socialShellReturn("connected", platform));
    } catch (error) {
      console.warn(`Connexion OAuth ${platform} refusée: ${error.message}`);
      sendRedirect(response, socialShellReturn("error", platform));
    }
    return;
  }

  sendJson(response, 404, { ok: false, error: "Route Social Analytics introuvable." });
}

const staticFiles = new Map([
  ["/social/", ["index.html", "text/html; charset=utf-8"]],
  ["/social/index.html", ["index.html", "text/html; charset=utf-8"]],
  ["/social/app.js", ["app.js", "text/javascript; charset=utf-8"]],
  ["/social/styles.css", ["styles.css", "text/css; charset=utf-8"]],
  ["/social/loader.js", ["loader.js", "text/javascript; charset=utf-8"]],
]);

async function serveStatic(response, pathname) {
  const descriptor = staticFiles.get(pathname);
  if (!descriptor) return false;
  const [file, contentType] = descriptor;
  const body = await readFile(path.join(publicDir, file));
  response.writeHead(200, securityHeaders({
    "content-type": contentType,
    "content-length": body.length,
    "content-security-policy": pathname.endsWith("index.html") || pathname === "/social/"
      ? "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: https:; connect-src 'self'; frame-ancestors 'self'; base-uri 'self'; form-action 'self'"
      : "default-src 'none'",
  }));
  response.end(body);
  return true;
}

const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url ?? "/", "http://switch-social.internal");
    if (request.method === "GET" && url.pathname === "/healthz") {
      sendJson(response, 200, { ok: true, service: "switch-social-analytics", demoMode });
      return;
    }
    if (request.method === "GET" && url.pathname === "/readyz") {
      const store = await loadStore();
      sendJson(response, 200, {
        ok: true,
        service: "switch-social-analytics",
        storageVersion: store.version,
      });
      return;
    }
    if (!hasValidProxyKey(request)) {
      sendJson(response, 403, { ok: false, error: "Accès direct refusé." });
      return;
    }
    if (url.pathname.startsWith("/api/social")) {
      await handleApi(request, response, url);
      return;
    }
    if (request.method === "GET" && url.pathname === "/social") {
      sendRedirect(response, "/social/");
      return;
    }
    if (request.method === "GET" && await serveStatic(response, url.pathname)) return;
    sendJson(response, 404, { ok: false, error: "Ressource introuvable." });
  } catch (error) {
    const status = Number.isInteger(error?.status) ? error.status : 500;
    if (status >= 500) console.error(error);
    sendJson(response, status, {
      ok: false,
      error: status >= 500 ? "Service Social Analytics indisponible." : error.message,
    });
  }
});

server.listen(port, "0.0.0.0", () => {
  console.log(`Switch Social Analytics écoute sur 0.0.0.0:${port}`);
});

if (scheduledSyncEnabled) {
  const firstScheduledSync = setTimeout(() => void runScheduledSync(), 30_000);
  firstScheduledSync.unref();
  const scheduledSync = setInterval(() => void runScheduledSync(), syncIntervalMilliseconds);
  scheduledSync.unref();
}
