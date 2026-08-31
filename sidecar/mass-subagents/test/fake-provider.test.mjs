import assert from "node:assert/strict";
import test from "node:test";

import {
  DeterministicFakeProvider,
  fakeProviderReplayHash,
} from "../src/fake-provider.mjs";

test("P0-FAKE-001: the same seed and inputs replay exactly", () => {
  const tasks = Array.from({ length: 100 }, (_, index) => ({
    logicalKey: `logical-${index + 1}`,
  }));
  const scenario = { throttleEvery: 7, timeoutEvery: 11, duplicateEvery: 13 };
  assert.equal(
    fakeProviderReplayHash({ seed: 42, scenario, tasks }),
    fakeProviderReplayHash({ seed: 42, scenario, tasks }),
  );
});

test("retries preserve the idempotency key and converge", () => {
  const provider = new DeterministicFakeProvider({
    seed: 42,
    scenario: { throttleEvery: 2, timeoutEvery: 3 },
  });
  const request = {
    logicalKey: "logical-2",
    ordinal: 2,
    idempotencyKey: "run:agent:attempt",
  };
  assert.equal(provider.execute({ ...request, attempt: 1 }).reason, "rate_limited");
  assert.equal(provider.execute({ ...request, attempt: 2 }).status, "success");
});

test("scenario values are bounded", () => {
  assert.throws(
    () => new DeterministicFakeProvider({ scenario: { throttleEvery: -1 } }),
    /throttleEvery/,
  );
});

test("the fake provider models virtual latency, duplicate and out-of-order delivery", () => {
  const provider = new DeterministicFakeProvider({
    seed: 42,
    scenario: {
      duplicateEvery: 2,
      outOfOrderEvery: 2,
      latencyEvery: 2,
      latencyMs: 75,
    },
  });
  const outcome = provider.execute({
    logicalKey: "logical-latency",
    ordinal: 2,
    attempt: 1,
    idempotencyKey: "run:logical-latency",
  });
  assert.equal(outcome.status, "success");
  assert.equal(outcome.duplicateDelivery, true);
  assert.equal(outcome.outOfOrderDelivery, true);
  assert.equal(outcome.latencyMs, 75);
});
