import { readFile, realpath } from "node:fs/promises";
import path from "node:path";

import { deepFreeze, sha256 } from "./canonical.mjs";

export const ROLE_IDS = Object.freeze([
  "scout",
  "implementer",
  "test_writer",
  "mutation_operator",
  "adversary",
  "quality_reviewer",
  "integrator",
]);

export const WRITE_SCOPE_ROLE_IDS = Object.freeze(["implementer", "test_writer"]);

export const ROLE_ARCHETYPES = deepFreeze({
  scout: {
    archetypeId: "T1",
    archetype: "chat-scout",
    executionKind: "chat",
    writeAccess: "read",
    outputContract: "scope_map",
  },
  implementer: {
    archetypeId: "T2",
    archetype: "chat-writer",
    executionKind: "chat",
    writeAccess: "write-scope",
    outputContract: "patch_manifest",
  },
  test_writer: {
    archetypeId: "T2",
    archetype: "chat-writer",
    executionKind: "chat",
    writeAccess: "write-scope",
    outputContract: "patch_manifest",
  },
  mutation_operator: {
    archetypeId: "T5",
    archetype: "chat-batch",
    executionKind: "chat",
    writeAccess: "read",
    outputContract: "mutants_matrix",
  },
  adversary: {
    archetypeId: "T3",
    archetype: "chat-auditor",
    executionKind: "chat",
    writeAccess: "read",
    outputContract: "proof_or_none",
  },
  quality_reviewer: {
    archetypeId: "T4",
    archetype: "chat-reviewer",
    executionKind: "chat",
    writeAccess: "read",
    outputContract: "review_report",
  },
  integrator: {
    archetypeId: "CAS",
    archetype: "controlled-integration",
    executionKind: "controlled-code",
    writeAccess: "integrate",
    outputContract: "integration_record",
  },
});

export const DEFAULT_ROLES_ROOT = path.resolve(import.meta.dirname, "../agent_roles");

const ROLE_VERSION = /^\d+\.\d+$/u;
const MODEL = /^gpt-[a-z0-9.-]+$/u;
const TOOL = /^[a-z][a-z0-9_-]{0,63}$/u;
const MANIFEST_SCHEMA = "../../_schemas/v1/role-manifest.schema.json";
const MANIFEST_KEYS = Object.freeze([
  "$schema",
  "schema_version",
  "role_id",
  "role_version",
  "execution_kind",
  "archetype_id",
  "archetype",
  "model",
  "reasoning",
  "service_tier",
  "write_access",
  "tools_allowlist",
  "capsule",
  "out_schema",
  "output_contract",
  "require_review",
  "second_review_rules",
]);

const isPlainObject = (value) => value !== null
  && typeof value === "object"
  && !Array.isArray(value)
  && (Object.getPrototypeOf(value) === Object.prototype
    || Object.getPrototypeOf(value) === null);

const requirePlainObject = (value, label) => {
  if (!isPlainObject(value)) throw new TypeError(`${label} must be an object`);
  return value;
};

const requireExactKeys = (value, expected, label) => {
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  const missing = wanted.filter((key) => !actual.includes(key));
  const extra = actual.filter((key) => !wanted.includes(key));
  if (missing.length > 0 || extra.length > 0) {
    throw new TypeError(
      `${label} has invalid fields (missing: ${missing.join(", ") || "none"}; extra: ${extra.join(", ") || "none"})`,
    );
  }
};

const requireString = (value, label, { pattern, maximum = 512 } = {}) => {
  if (typeof value !== "string" || value.length === 0 || value.length > maximum) {
    throw new TypeError(`${label} must be a non-empty string of at most ${maximum} characters`);
  }
  if (pattern && !pattern.test(value)) throw new TypeError(`${label} has an invalid format`);
  return value;
};

const requireEnum = (value, allowed, label) => {
  if (!allowed.includes(value)) {
    throw new TypeError(`${label} must be one of: ${allowed.join(", ")}`);
  }
  return value;
};

const requireBoolean = (value, label) => {
  if (typeof value !== "boolean") throw new TypeError(`${label} must be a boolean`);
  return value;
};

const requireRootRelativeReference = (value, label, extension) => {
  requireString(value, label, { maximum: 256 });
  if (value.includes("\\") || value.startsWith("/") || /^[a-z]:/iu.test(value)) {
    throw new TypeError(`${label} must use a repository-relative POSIX path`);
  }
  const segments = value.split("/");
  if (segments.some((segment) => segment === "" || segment === "." || segment === "..")) {
    throw new TypeError(`${label} must not contain empty, current, or parent segments`);
  }
  if (!value.endsWith(extension)) throw new TypeError(`${label} must end with ${extension}`);
  return value;
};

const requireUniqueTools = (value) => {
  if (!Array.isArray(value) || value.length === 0 || value.length > 32) {
    throw new TypeError("role manifest tools_allowlist must contain between 1 and 32 tools");
  }
  const tools = value.map((tool, index) => requireString(
    tool,
    `role manifest tools_allowlist[${index}]`,
    { pattern: TOOL, maximum: 64 },
  ));
  if (new Set(tools).size !== tools.length) {
    throw new TypeError("role manifest tools_allowlist must not contain duplicates");
  }
  return Object.freeze([...tools]);
};

/**
 * Validate the versioned role boundary. Unknown fields and role/archetype
 * mismatches are rejected instead of being silently ignored.
 */
export const validateRoleManifest = (candidate, { expectedRoleId } = {}) => {
  const manifest = requirePlainObject(candidate, "role manifest");
  requireExactKeys(manifest, MANIFEST_KEYS, "role manifest");

  if (manifest.$schema !== MANIFEST_SCHEMA) {
    throw new TypeError(`role manifest $schema must be ${MANIFEST_SCHEMA}`);
  }
  if (manifest.schema_version !== "switch-agent-role/v1") {
    throw new TypeError("role manifest schema_version must be switch-agent-role/v1");
  }

  const roleId = requireEnum(manifest.role_id, ROLE_IDS, "role manifest role_id");
  if (expectedRoleId !== undefined && roleId !== expectedRoleId) {
    throw new TypeError(`role manifest role_id must be ${expectedRoleId}`);
  }
  const expected = ROLE_ARCHETYPES[roleId];
  const roleVersion = requireString(manifest.role_version, "role manifest role_version", {
    pattern: ROLE_VERSION,
    maximum: 16,
  });

  for (const [field, expectedValue] of [
    ["execution_kind", expected.executionKind],
    ["archetype_id", expected.archetypeId],
    ["archetype", expected.archetype],
    ["write_access", expected.writeAccess],
    ["output_contract", expected.outputContract],
  ]) {
    if (manifest[field] !== expectedValue) {
      throw new TypeError(`role manifest ${field} must be ${expectedValue} for ${roleId}`);
    }
  }

  const model = requireString(manifest.model, "role manifest model", {
    pattern: MODEL,
    maximum: 64,
  });
  const reasoning = requireEnum(
    manifest.reasoning,
    ["none", "low", "medium", "high"],
    "role manifest reasoning",
  );
  const serviceTier = requireEnum(
    manifest.service_tier,
    ["default", "priority"],
    "role manifest service_tier",
  );
  const toolsAllowlist = requireUniqueTools(manifest.tools_allowlist);
  const capsule = requireRootRelativeReference(manifest.capsule, "role manifest capsule", ".md");
  const outSchema = requireRootRelativeReference(
    manifest.out_schema,
    "role manifest out_schema",
    ".json",
  );
  const requireReview = requireBoolean(manifest.require_review, "role manifest require_review");
  const secondReviewRules = manifest.second_review_rules === null
    ? null
    : requireString(manifest.second_review_rules, "role manifest second_review_rules", {
      maximum: 128,
    });

  if (roleId === "integrator" && manifest.execution_kind !== "controlled-code") {
    throw new TypeError("integrator must be controlled code, never a free-running chat");
  }
  if (WRITE_SCOPE_ROLE_IDS.includes(roleId) && !requireReview) {
    throw new TypeError(`${roleId} must require an independent quality review`);
  }
  if (WRITE_SCOPE_ROLE_IDS.includes(roleId)
    && secondReviewRules !== "code-quality-review/v1") {
    throw new TypeError(`${roleId} must use code-quality-review/v1 second-review rules`);
  }

  return deepFreeze({
    $schema: manifest.$schema,
    schema_version: manifest.schema_version,
    role_id: roleId,
    role_version: roleVersion,
    execution_kind: manifest.execution_kind,
    archetype_id: manifest.archetype_id,
    archetype: manifest.archetype,
    model,
    reasoning,
    service_tier: serviceTier,
    write_access: manifest.write_access,
    tools_allowlist: toolsAllowlist,
    capsule,
    out_schema: outSchema,
    output_contract: manifest.output_contract,
    require_review: requireReview,
    second_review_rules: secondReviewRules,
  });
};

const parseJson = (source, label) => {
  try {
    return JSON.parse(source);
  } catch (error) {
    throw new TypeError(`${label} is not valid JSON: ${error.message}`);
  }
};

const isInside = (root, target) => {
  const relative = path.relative(root, target);
  return relative === "" || (!relative.startsWith(`..${path.sep}`)
    && relative !== ".."
    && !path.isAbsolute(relative));
};

const resolveExistingReference = async (root, reference, label) => {
  const lexicalTarget = path.resolve(root, ...reference.split("/"));
  if (!isInside(root, lexicalTarget)) throw new TypeError(`${label} escapes the roles root`);
  const actualTarget = await realpath(lexicalTarget);
  if (!isInside(root, actualTarget)) throw new TypeError(`${label} resolves outside the roles root`);
  return actualTarget;
};

const validateOutputSchema = (schema, label) => {
  requirePlainObject(schema, label);
  if (schema.$schema !== "https://json-schema.org/draft/2020-12/schema") {
    throw new TypeError(`${label} must use JSON Schema draft 2020-12`);
  }
  requireString(schema.$id, `${label} $id`, { maximum: 256 });
  if (schema.type !== "object" || schema.additionalProperties !== false) {
    throw new TypeError(`${label} must define a strict object boundary`);
  }
};

/** Load and content-address all seven immutable v1 role capsules. */
export const loadRoleRegistry = async ({ rolesRoot = DEFAULT_ROLES_ROOT } = {}) => {
  const root = await realpath(path.resolve(rolesRoot));
  const loaded = await Promise.all(ROLE_IDS.map(async (roleId) => {
    const manifestPath = path.join(root, roleId, "v1", "manifest.json");
    const manifestSource = await readFile(manifestPath, "utf8");
    const manifest = validateRoleManifest(parseJson(manifestSource, `${roleId} manifest`), {
      expectedRoleId: roleId,
    });
    const [capsulePath, outputSchemaPath] = await Promise.all([
      resolveExistingReference(root, manifest.capsule, `${roleId} capsule`),
      resolveExistingReference(root, manifest.out_schema, `${roleId} output schema`),
    ]);
    const [capsuleSource, outputSchemaSource] = await Promise.all([
      readFile(capsulePath, "utf8"),
      readFile(outputSchemaPath, "utf8"),
    ]);
    if (capsuleSource.trim().length === 0 || Buffer.byteLength(capsuleSource, "utf8") > 16_384) {
      throw new TypeError(`${roleId} capsule must contain between 1 and 16384 UTF-8 bytes`);
    }
    if (capsuleSource.includes("\0")) throw new TypeError(`${roleId} capsule contains a NUL byte`);
    const outputSchema = parseJson(outputSchemaSource, `${roleId} output schema`);
    validateOutputSchema(outputSchema, `${roleId} output schema`);

    const manifestSha256 = sha256(manifest);
    const capsuleSha256 = sha256(capsuleSource);
    const outputSchemaSha256 = sha256(outputSchema);
    return deepFreeze({
      ...manifest,
      capsule_source: capsuleSource,
      output_schema: outputSchema,
      manifest_sha256: manifestSha256,
      capsule_sha256: capsuleSha256,
      output_schema_sha256: outputSchemaSha256,
      identity_sha256: sha256({
        manifestSha256,
        capsuleSha256,
        outputSchemaSha256,
      }),
    });
  }));

  const roles = Object.fromEntries(loaded.map((role) => [role.role_id, role]));
  const registrySha256 = sha256(ROLE_IDS.map((roleId) => ({
    role_id: roleId,
    role_version: roles[roleId].role_version,
    identity_sha256: roles[roleId].identity_sha256,
  })));
  return deepFreeze({
    schema_version: "switch-agent-role-registry/v1",
    registry_sha256: registrySha256,
    roles,
  });
};

export const getRoleDefinition = (registry, roleId, roleVersion = "1.0") => {
  requirePlainObject(registry, "role registry");
  const normalizedRoleId = requireEnum(roleId, ROLE_IDS, "role id");
  const role = registry.roles?.[normalizedRoleId];
  if (!role) throw new TypeError(`role registry does not contain ${normalizedRoleId}`);
  if (role.role_version !== roleVersion) {
    throw new TypeError(
      `role ${normalizedRoleId} version ${roleVersion} is unavailable (loaded: ${role.role_version})`,
    );
  }
  return role;
};

export const roleInternals = Object.freeze({
  MANIFEST_KEYS,
  MANIFEST_SCHEMA,
  isInside,
});
