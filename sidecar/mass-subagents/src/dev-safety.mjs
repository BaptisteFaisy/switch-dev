export const CANONICAL_SWITCH_DEVELOPMENT_ORIGIN =
  "https://pc-fixe-cst.tail3a8bdf.ts.net:10000";
export const COMPOSE_SWITCH_DEVELOPMENT_ORIGIN = "http://switch:8080";

const LOCAL_BASE_URL = /^https?:\/\/(?:localhost|127\.0\.0\.1)(?::[0-9]{1,5})?\/?$/i;
const CANONICAL_BASE_URL =
  /^https:\/\/pc-fixe-cst\.tail3a8bdf\.ts\.net:10000\/?$/i;
const COMPOSE_BASE_URL = /^http:\/\/switch:8080\/?$/i;

export class SwitchDevelopmentSafetyError extends Error {
  constructor(message) {
    super(message);
    this.name = "SwitchDevelopmentSafetyError";
    this.code = "SWITCH_DEVELOPMENT_URL_REFUSED";
  }
}

const refuse = () => {
  throw new SwitchDevelopmentSafetyError(
    "Switch URL refused: only explicit local endpoints and the canonical development endpoint are allowed.",
  );
};

export const assertDevelopmentSwitchBaseUrl = (candidate) => {
  if (typeof candidate !== "string") refuse();

  const raw = candidate.trim();
  if (!LOCAL_BASE_URL.test(raw)
    && !CANONICAL_BASE_URL.test(raw)
    && !COMPOSE_BASE_URL.test(raw)) refuse();

  let parsed;
  try {
    parsed = new URL(raw);
  } catch {
    refuse();
  }

  if (parsed.username || parsed.password || parsed.search || parsed.hash) refuse();
  if (parsed.pathname !== "/") refuse();

  const local = parsed.hostname === "localhost" || parsed.hostname === "127.0.0.1";
  const compose = parsed.origin === COMPOSE_SWITCH_DEVELOPMENT_ORIGIN;
  if (!local && !compose && parsed.origin !== CANONICAL_SWITCH_DEVELOPMENT_ORIGIN) refuse();

  if (parsed.port) {
    const port = Number(parsed.port);
    if (!Number.isInteger(port) || port < 1 || port > 65_535) refuse();
  }

  return parsed.origin;
};

export const isDevelopmentSwitchBaseUrl = (candidate) => {
  try {
    assertDevelopmentSwitchBaseUrl(candidate);
    return true;
  } catch (error) {
    if (error instanceof SwitchDevelopmentSafetyError) return false;
    throw error;
  }
};
