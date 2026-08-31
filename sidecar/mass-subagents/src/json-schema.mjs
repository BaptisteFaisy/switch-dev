import { canonicalJson } from "./canonical.mjs";

const typeMatches = (value, type) => {
  if (type === "null") return value === null;
  if (type === "array") return Array.isArray(value);
  if (type === "object") return value !== null && typeof value === "object" && !Array.isArray(value);
  if (type === "integer") return Number.isSafeInteger(value);
  if (type === "number") return typeof value === "number" && Number.isFinite(value);
  return typeof value === type;
};

const resolveReference = (root, reference) => {
  if (typeof reference !== "string" || !reference.startsWith("#/")) return null;
  let current = root;
  for (const encoded of reference.slice(2).split("/")) {
    const segment = encoded.replaceAll("~1", "/").replaceAll("~0", "~");
    current = current?.[segment];
  }
  return current ?? null;
};

const validateNode = (value, schema, root, path, errors) => {
  if (!schema || typeof schema !== "object" || Array.isArray(schema)) {
    errors.push(`${path}: invalid schema node`);
    return;
  }
  if (schema.$ref) {
    const resolved = resolveReference(root, schema.$ref);
    if (!resolved) errors.push(`${path}: unresolved schema reference ${schema.$ref}`);
    else validateNode(value, resolved, root, path, errors);
    return;
  }
  if (Object.hasOwn(schema, "const") && canonicalJson(value) !== canonicalJson(schema.const)) {
    errors.push(`${path}: value does not match const`);
  }
  if (Array.isArray(schema.enum)
    && !schema.enum.some((entry) => canonicalJson(entry) === canonicalJson(value))) {
    errors.push(`${path}: value is not in enum`);
  }
  if (schema.type !== undefined) {
    const types = Array.isArray(schema.type) ? schema.type : [schema.type];
    if (!types.some((type) => typeMatches(value, type))) {
      errors.push(`${path}: expected ${types.join(" or ")}`);
      return;
    }
  }

  if (typeof value === "string") {
    if (Number.isInteger(schema.minLength) && value.length < schema.minLength) {
      errors.push(`${path}: shorter than minLength`);
    }
    if (Number.isInteger(schema.maxLength) && value.length > schema.maxLength) {
      errors.push(`${path}: longer than maxLength`);
    }
    if (schema.pattern && !new RegExp(schema.pattern, "u").test(value)) {
      errors.push(`${path}: does not match pattern`);
    }
    if (schema.format === "date-time" && Number.isNaN(Date.parse(value))) {
      errors.push(`${path}: invalid date-time`);
    }
  }
  if (typeof value === "number" && Number.isFinite(value)) {
    if (typeof schema.minimum === "number" && value < schema.minimum) {
      errors.push(`${path}: below minimum`);
    }
    if (typeof schema.maximum === "number" && value > schema.maximum) {
      errors.push(`${path}: above maximum`);
    }
  }
  if (Array.isArray(value)) {
    if (Number.isInteger(schema.minItems) && value.length < schema.minItems) {
      errors.push(`${path}: fewer than minItems`);
    }
    if (Number.isInteger(schema.maxItems) && value.length > schema.maxItems) {
      errors.push(`${path}: more than maxItems`);
    }
    if (schema.uniqueItems === true) {
      const keys = value.map(canonicalJson);
      if (new Set(keys).size !== keys.length) errors.push(`${path}: items are not unique`);
    }
    if (schema.items) {
      value.forEach((item, index) => validateNode(item, schema.items, root, `${path}[${index}]`, errors));
    }
  }
  if (value !== null && typeof value === "object" && !Array.isArray(value)) {
    for (const key of schema.required ?? []) {
      if (!Object.hasOwn(value, key)) errors.push(`${path}: missing required property ${key}`);
    }
    if (schema.additionalProperties === false && schema.properties) {
      for (const key of Object.keys(value)) {
        if (!Object.hasOwn(schema.properties, key)) errors.push(`${path}: unknown property ${key}`);
      }
    }
    for (const [key, childSchema] of Object.entries(schema.properties ?? {})) {
      if (Object.hasOwn(value, key)) validateNode(value[key], childSchema, root, `${path}.${key}`, errors);
    }
  }
  for (const child of schema.allOf ?? []) validateNode(value, child, root, path, errors);
  if (schema.if) {
    const conditionErrors = [];
    validateNode(value, schema.if, root, path, conditionErrors);
    if (conditionErrors.length === 0 && schema.then) validateNode(value, schema.then, root, path, errors);
    if (conditionErrors.length > 0 && schema.else) validateNode(value, schema.else, root, path, errors);
  }
};

export const validateJsonSchema = (value, schema) => {
  const errors = [];
  validateNode(value, schema, schema, "$", errors);
  return Object.freeze({ valid: errors.length === 0, errors: Object.freeze(errors.slice(0, 100)) });
};

export const assertJsonSchema = (value, schema) => {
  const result = validateJsonSchema(value, schema);
  if (!result.valid) {
    const error = new Error(`role output does not match its schema: ${result.errors.join("; ")}`);
    error.code = "ROLE_OUTPUT_SCHEMA_INVALID";
    error.validationErrors = result.errors;
    throw error;
  }
  return value;
};
