function parseDayKey(value) {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!match) throw new Error(`Date civile invalide: ${value}`);
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const candidate = new Date(Date.UTC(year, month - 1, day, 12));
  if (
    candidate.getUTCFullYear() !== year
    || candidate.getUTCMonth() !== month - 1
    || candidate.getUTCDate() !== day
  ) throw new Error(`Date civile invalide: ${value}`);
  return { year, month, day };
}

function formattedParts(formatter, instant) {
  const values = Object.fromEntries(
    formatter.formatToParts(instant)
      .filter((part) => part.type !== "literal")
      .map((part) => [part.type, Number(part.value)]),
  );
  return {
    year: values.year,
    month: values.month,
    day: values.day,
    hour: values.hour,
    minute: values.minute,
    second: values.second,
  };
}

export function createCivilTime(timeZone) {
  const formatter = new Intl.DateTimeFormat("en-CA", {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hourCycle: "h23",
  });

  function dayKey(instant = new Date()) {
    const parts = formattedParts(formatter, instant);
    return `${String(parts.year).padStart(4, "0")}-${String(parts.month).padStart(2, "0")}-${String(parts.day).padStart(2, "0")}`;
  }

  function shiftedDayKey(key, amount) {
    const { year, month, day } = parseDayKey(key);
    const shifted = new Date(Date.UTC(year, month - 1, day + amount, 12));
    return shifted.toISOString().slice(0, 10);
  }

  function dateKeys(count, end = new Date()) {
    const total = Math.max(0, Number.parseInt(count, 10) || 0);
    const endKey = dayKey(end);
    return Array.from({ length: total }, (_, index) => shiftedDayKey(endKey, index - total + 1));
  }

  function midnightEpochMilliseconds(key) {
    const { year, month, day } = parseDayKey(key);
    const civilMidnightAsUtc = Date.UTC(year, month - 1, day, 0, 0, 0);
    let candidate = civilMidnightAsUtc;
    for (let iteration = 0; iteration < 6; iteration += 1) {
      const parts = formattedParts(formatter, new Date(candidate));
      const renderedAsUtc = Date.UTC(
        parts.year,
        parts.month - 1,
        parts.day,
        parts.hour,
        parts.minute,
        parts.second,
      );
      const next = civilMidnightAsUtc - (renderedAsUtc - candidate);
      if (next === candidate) break;
      candidate = next;
    }
    const resolved = formattedParts(formatter, new Date(candidate));
    if (
      resolved.year !== year
      || resolved.month !== month
      || resolved.day !== day
      || resolved.hour !== 0
      || resolved.minute !== 0
      || resolved.second !== 0
    ) throw new Error(`Minuit introuvable pour ${key} dans ${timeZone}`);
    return candidate;
  }

  function unixDayBoundary(key) {
    return String(Math.floor(midnightEpochMilliseconds(key) / 1_000));
  }

  function dayWindow(key) {
    const since = midnightEpochMilliseconds(key);
    const untilExclusive = midnightEpochMilliseconds(shiftedDayKey(key, 1));
    return { since, untilExclusive, durationHours: (untilExclusive - since) / 3_600_000 };
  }

  return { dayKey, shiftedDayKey, dateKeys, unixDayBoundary, dayWindow };
}
