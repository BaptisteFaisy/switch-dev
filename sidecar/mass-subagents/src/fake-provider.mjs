import { createHash } from "node:crypto";

const DEFAULT_SCENARIO = Object.freeze({
  throttleEvery: 0,
  timeoutEvery: 0,
  duplicateEvery: 0,
  outOfOrderEvery: 0,
  latencyEvery: 0,
  latencyMs: 0,
});

const positiveInteger = (value, label, maximum = 1_000_000) => {
  const parsed = Number(value ?? 0);
  if (!Number.isInteger(parsed) || parsed < 0 || parsed > maximum) {
    throw new TypeError(`${label} must be an integer between 0 and ${maximum}`);
  }
  return parsed;
};

const digest = (value) => createHash("sha256").update(value).digest("hex");

export const normalizeFakeScenario = (scenario = {}) => {
  if (scenario === null || typeof scenario !== "object" || Array.isArray(scenario)) {
    throw new TypeError("fakeScenario must be an object");
  }
  const extra = Object.keys(scenario).filter((key) => !Object.hasOwn(DEFAULT_SCENARIO, key));
  if (extra.length > 0) throw new TypeError(`fakeScenario has unknown fields: ${extra.join(", ")}`);
  return Object.freeze({
    throttleEvery: positiveInteger(
      scenario.throttleEvery ?? DEFAULT_SCENARIO.throttleEvery,
      "throttleEvery",
    ),
    timeoutEvery: positiveInteger(
      scenario.timeoutEvery ?? DEFAULT_SCENARIO.timeoutEvery,
      "timeoutEvery",
    ),
    duplicateEvery: positiveInteger(
      scenario.duplicateEvery ?? DEFAULT_SCENARIO.duplicateEvery,
      "duplicateEvery",
    ),
    outOfOrderEvery: positiveInteger(
      scenario.outOfOrderEvery ?? DEFAULT_SCENARIO.outOfOrderEvery,
      "outOfOrderEvery",
    ),
    latencyEvery: positiveInteger(
      scenario.latencyEvery ?? DEFAULT_SCENARIO.latencyEvery,
      "latencyEvery",
    ),
    latencyMs: positiveInteger(
      scenario.latencyMs ?? DEFAULT_SCENARIO.latencyMs,
      "latencyMs",
      60_000,
    ),
  });
};

const matches = (ordinal, every) => every > 0 && ordinal > 0 && ordinal % every === 0;

/**
 * Fournisseur sans réseau, déterministe et rejouable. Les fautes ne sont
 * injectées qu'à la première tentative : le scheduler doit les reprendre avec
 * la même clé d'idempotence, sans produire un second effet logique.
 */
export class DeterministicFakeProvider {
  constructor({ seed = 42, scenario } = {}) {
    this.seed = positiveInteger(seed, "seed", Number.MAX_SAFE_INTEGER);
    this.scenario = normalizeFakeScenario(scenario);
  }

  execute({ logicalKey, ordinal, attempt, idempotencyKey }) {
    if (typeof logicalKey !== "string" || logicalKey.length === 0) {
      throw new TypeError("logicalKey is required");
    }
    if (typeof idempotencyKey !== "string" || idempotencyKey.length === 0) {
      throw new TypeError("idempotencyKey is required");
    }
    const normalizedOrdinal = positiveInteger(ordinal, "ordinal");
    const normalizedAttempt = positiveInteger(attempt, "attempt");
    // L'idempotency key remains run-specific so two runs cannot share an
    // external effect. The deterministic payload deliberately depends on the
    // stable logical key instead, allowing an exact replay of the same plan.
    const basis = `${this.seed}:${logicalKey}`;
    const latencyMs = matches(normalizedOrdinal, this.scenario.latencyEvery)
      ? this.scenario.latencyMs
      : 0;

    if (normalizedAttempt === 1 && matches(normalizedOrdinal, this.scenario.throttleEvery)) {
      return Object.freeze({
        status: "retry",
        reason: "rate_limited",
        retryAfterMs: 25 + (Number.parseInt(digest(basis).slice(0, 2), 16) % 50),
        latencyMs,
      });
    }
    if (normalizedAttempt === 1 && matches(normalizedOrdinal, this.scenario.timeoutEvery)) {
      return Object.freeze({ status: "retry", reason: "timeout", retryAfterMs: 10, latencyMs });
    }

    const payload = Object.freeze({
      logicalKey,
      result: "verified",
      seed: this.seed,
    });
    return Object.freeze({
      status: "success",
      payload,
      payloadSha256: digest(JSON.stringify(payload)),
      duplicateDelivery: matches(normalizedOrdinal, this.scenario.duplicateEvery),
      outOfOrderDelivery: matches(normalizedOrdinal, this.scenario.outOfOrderEvery),
      latencyMs,
    });
  }
}

export const fakeProviderReplayHash = ({ seed = 42, scenario, tasks }) => {
  const provider = new DeterministicFakeProvider({ seed, scenario });
  const results = tasks.map((task, index) => provider.execute({
    logicalKey: task.logicalKey,
    ordinal: task.ordinal ?? index + 1,
    attempt: task.attempt ?? 1,
    idempotencyKey: task.idempotencyKey ?? `fake:${task.logicalKey}`,
  }));
  return digest(JSON.stringify(results));
};
