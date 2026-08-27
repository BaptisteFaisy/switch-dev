import assert from "node:assert/strict";
import test from "node:test";
import { createCivilTime } from "../time.mjs";

test("les journées Europe/Paris suivent les passages DST", () => {
  const time = createCivilTime("Europe/Paris");
  const spring = time.dayWindow("2026-03-29");
  assert.equal(new Date(spring.since).toISOString(), "2026-03-28T23:00:00.000Z");
  assert.equal(new Date(spring.untilExclusive).toISOString(), "2026-03-29T22:00:00.000Z");
  assert.equal(spring.durationHours, 23);

  const autumn = time.dayWindow("2026-10-25");
  assert.equal(new Date(autumn.since).toISOString(), "2026-10-24T22:00:00.000Z");
  assert.equal(new Date(autumn.untilExclusive).toISOString(), "2026-10-25T23:00:00.000Z");
  assert.equal(autumn.durationHours, 25);
});

test("les clés civiles restent ordonnées autour des changements d’heure", () => {
  const time = createCivilTime("Europe/Paris");
  assert.deepEqual(
    time.dateKeys(4, new Date("2026-03-30T12:00:00.000Z")),
    ["2026-03-27", "2026-03-28", "2026-03-29", "2026-03-30"],
  );
  assert.equal(time.shiftedDayKey("2026-10-25", 1), "2026-10-26");
});
