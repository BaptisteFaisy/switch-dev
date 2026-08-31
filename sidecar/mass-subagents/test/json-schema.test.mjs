import assert from "node:assert/strict";
import test from "node:test";

import { validateJsonSchema } from "../src/json-schema.mjs";

const schema = {
  type: "object",
  additionalProperties: false,
  required: ["kind", "items"],
  properties: {
    kind: { enum: ["ok", "none"] },
    items: {
      type: "array",
      minItems: 1,
      uniqueItems: true,
      items: { $ref: "#/$defs/item" },
    },
    detail: { type: ["object", "null"] },
  },
  $defs: {
    item: { type: "string", pattern: "^[a-z]+$", maxLength: 8 },
  },
  allOf: [{
    if: { properties: { kind: { const: "none" } }, required: ["kind"] },
    then: { properties: { detail: { type: "null" } } },
  }],
};

test("the bundled validator resolves local refs and conditional strict schemas", () => {
  assert.equal(validateJsonSchema({ kind: "ok", items: ["alpha"] }, schema).valid, true);
  const result = validateJsonSchema({
    kind: "none",
    items: ["ALPHA", "ALPHA"],
    detail: {},
    extra: true,
  }, schema);
  assert.equal(result.valid, false);
  assert.ok(result.errors.some((error) => error.includes("unknown property extra")));
  assert.ok(result.errors.some((error) => error.includes("items are not unique")));
  assert.ok(result.errors.some((error) => error.includes("does not match pattern")));
  assert.ok(result.errors.some((error) => error.includes("expected null")));
});
