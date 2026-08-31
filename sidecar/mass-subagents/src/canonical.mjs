import { createHash } from "node:crypto";

const normalize = (value) => {
  if (Array.isArray(value)) return value.map(normalize);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, normalize(value[key])]),
    );
  }
  return value;
};

export const canonicalJson = (value) => JSON.stringify(normalize(value));

export const sha256 = (value) => createHash("sha256")
  .update(typeof value === "string" || Buffer.isBuffer(value) ? value : canonicalJson(value))
  .digest("hex");

export const deepFreeze = (value) => {
  if (!value || typeof value !== "object" || Object.isFrozen(value)) return value;
  Object.freeze(value);
  for (const child of Object.values(value)) deepFreeze(child);
  return value;
};
