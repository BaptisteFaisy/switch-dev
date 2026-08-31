import { canonicalJson, deepFreeze, sha256 } from "./canonical.mjs";
import {
  ROLE_IDS,
  WRITE_SCOPE_ROLE_IDS,
  getRoleDefinition,
  loadRoleRegistry,
} from "./roles.mjs";

export const FACTORY_SCHEMA_VERSION = "switch-agent-factory/v1";
export const RUN_SPEC_SCHEMA_VERSION = "switch-agent-run-spec/v1";
export const MAX_LOGICAL_AGENTS = 10_000;
export const MAX_COORDINATORS = 32;
export const DEFAULT_MAX_CONTEXT_BYTES = 16_384;

const ABSOLUTE_MAX_CONTEXT_BYTES = 65_536;
const IDENTIFIER = /^[a-z0-9][a-z0-9_-]{0,127}$/u;
const COMMIT = /^[a-f0-9]{7,64}$/u;

const isPlainObject = (value) => value !== null
  && typeof value === "object"
  && !Array.isArray(value)
  && (Object.getPrototypeOf(value) === Object.prototype
    || Object.getPrototypeOf(value) === null);

const requirePlainObject = (value, label) => {
  if (!isPlainObject(value)) throw new TypeError(`${label} must be an object`);
  return value;
};

const requireFields = (value, { required, allowed = required }, label) => {
  const keys = Object.keys(value);
  const missing = required.filter((key) => !keys.includes(key));
  const extra = keys.filter((key) => !allowed.includes(key));
  if (missing.length > 0 || extra.length > 0) {
    throw new TypeError(
      `${label} has invalid fields (missing: ${missing.join(", ") || "none"}; extra: ${extra.join(", ") || "none"})`,
    );
  }
};

const requireString = (value, label, { minimum = 1, maximum = 4_096, pattern } = {}) => {
  if (typeof value !== "string" || value.length < minimum || value.length > maximum) {
    throw new TypeError(
      `${label} must be a string between ${minimum} and ${maximum} characters`,
    );
  }
  if (value.normalize("NFC") !== value) throw new TypeError(`${label} must be NFC-normalized`);
  if (value.includes("\0")) throw new TypeError(`${label} must not contain a NUL byte`);
  if (pattern && !pattern.test(value)) throw new TypeError(`${label} has an invalid format`);
  return value;
};

const requireInteger = (value, label, minimum, maximum) => {
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    throw new TypeError(`${label} must be an integer between ${minimum} and ${maximum}`);
  }
  return value;
};

const normalizeStringList = (
  value,
  label,
  { minimum = 0, maximum = 64, itemMaximum = 1_024 } = {},
) => {
  if (!Array.isArray(value) || value.length < minimum || value.length > maximum) {
    throw new TypeError(`${label} must contain between ${minimum} and ${maximum} entries`);
  }
  const normalized = value.map((item, index) => requireString(item, `${label}[${index}]`, {
    maximum: itemMaximum,
  }));
  if (new Set(normalized).size !== normalized.length) {
    throw new TypeError(`${label} must not contain duplicates`);
  }
  return Object.freeze(normalized);
};

const normalizeRunContext = (candidate) => {
  const context = requirePlainObject(candidate, "run context");
  requireFields(context, {
    required: ["objective", "acceptanceCriteria"],
    allowed: ["objective", "acceptanceCriteria", "constraints", "references"],
  }, "run context");
  return deepFreeze({
    objective: requireString(context.objective, "run context objective"),
    acceptanceCriteria: normalizeStringList(
      context.acceptanceCriteria,
      "run context acceptanceCriteria",
      { minimum: 1 },
    ),
    constraints: normalizeStringList(context.constraints ?? [], "run context constraints"),
    references: normalizeStringList(context.references ?? [], "run context references"),
  });
};

const normalizeAssignmentContext = (candidate, label) => {
  const context = requirePlainObject(candidate, label);
  requireFields(context, {
    required: ["task"],
    allowed: ["task", "acceptanceCriteria", "constraints", "references"],
  }, label);
  return deepFreeze({
    task: requireString(context.task, `${label} task`),
    acceptanceCriteria: normalizeStringList(
      context.acceptanceCriteria ?? [],
      `${label} acceptanceCriteria`,
    ),
    constraints: normalizeStringList(context.constraints ?? [], `${label} constraints`),
    references: normalizeStringList(context.references ?? [], `${label} references`),
  });
};

const validatePathPart = (value, label) => {
  requireString(value, label, { maximum: 512 });
  if (value.startsWith("/") || value.startsWith("./") || value.includes("\\")
    || /^[a-z]:/iu.test(value)) {
    throw new TypeError(`${label} must be a repository-relative POSIX path`);
  }
  const segments = value.split("/");
  for (const segment of segments) {
    if (segment === "" || segment === "." || segment === "..") {
      throw new TypeError(`${label} contains an invalid path segment`);
    }
    if (/[\u0000-\u001f*?\[\]{}!#:]/u.test(segment)
      || segment.trim() !== segment
      || segment.endsWith(".")) {
      throw new TypeError(`${label} contains an unsafe or ambiguous path segment`);
    }
  }
  return value;
};

/**
 * Accepted selectors are exact files, `directory/**` trees, or
 * `file#symbol` symbols. More expressive globs are deliberately rejected so
 * overlap is decidable before any writer is admitted.
 */
export const parseScopeSelector = (candidate, label = "scope selector") => {
  const selector = requireString(candidate, label, { maximum: 640 });
  if (selector === "**") return deepFreeze({ selector, kind: "tree", path: "", symbol: null });

  const hashParts = selector.split("#");
  if (hashParts.length > 2) throw new TypeError(`${label} contains more than one symbol marker`);
  const [rawPath, symbol] = hashParts;
  const isTree = rawPath.endsWith("/**");
  if (isTree && symbol !== undefined) {
    throw new TypeError(`${label} cannot combine a tree and a symbol`);
  }
  const filePath = isTree ? rawPath.slice(0, -3) : rawPath;
  validatePathPart(filePath, label);
  if (/[*?\[\]{}!]/u.test(filePath)) throw new TypeError(`${label} contains an unsupported glob`);

  if (symbol !== undefined) {
    requireString(symbol, `${label} symbol`, {
      maximum: 128,
      pattern: /^[\p{L}_$][\p{L}\p{N}_$:.<>-]{0,127}$/u,
    });
    return deepFreeze({ selector, kind: "symbol", path: filePath, symbol });
  }
  return deepFreeze({
    selector,
    kind: isTree ? "tree" : "file",
    path: filePath,
    symbol: null,
  });
};

const treeContains = (treePath, targetPath) => treePath === ""
  || targetPath === treePath
  || targetPath.startsWith(`${treePath}/`);

export const scopeSelectorsOverlap = (leftCandidate, rightCandidate) => {
  const left = typeof leftCandidate === "string" ? parseScopeSelector(leftCandidate) : leftCandidate;
  const right = typeof rightCandidate === "string" ? parseScopeSelector(rightCandidate) : rightCandidate;
  if (left.kind === "tree") return treeContains(left.path, right.path);
  if (right.kind === "tree") return treeContains(right.path, left.path);
  if (left.path !== right.path) return false;
  if (left.kind === "file" || right.kind === "file") return true;
  return left.symbol === right.symbol;
};

const normalizeScopeSet = (candidate, label) => {
  if (!Array.isArray(candidate) || candidate.length > 64) {
    throw new TypeError(`${label} must be an array of at most 64 selectors`);
  }
  const parsed = candidate.map((selector, index) => parseScopeSelector(
    selector,
    `${label}[${index}]`,
  ));
  const selectors = parsed.map(({ selector }) => selector);
  if (new Set(selectors).size !== selectors.length) {
    throw new TypeError(`${label} must not contain duplicate selectors`);
  }
  for (let left = 0; left < parsed.length; left += 1) {
    for (let right = left + 1; right < parsed.length; right += 1) {
      if (scopeSelectorsOverlap(parsed[left], parsed[right])) {
        throw new TypeError(
          `${label} contains redundant overlapping selectors ${selectors[left]} and ${selectors[right]}`,
        );
      }
    }
  }
  return Object.freeze(selectors);
};

class StringMinHeap {
  constructor() {
    this.values = [];
  }

  push(value) {
    let index = this.values.push(value) - 1;
    while (index > 0) {
      const parent = Math.floor((index - 1) / 2);
      if (this.values[parent].localeCompare(value) <= 0) break;
      this.values[index] = this.values[parent];
      index = parent;
    }
    this.values[index] = value;
  }

  pop() {
    const first = this.values[0];
    const tail = this.values.pop();
    if (this.values.length === 0) return first;
    let index = 0;
    while (true) {
      const left = (index * 2) + 1;
      if (left >= this.values.length) break;
      const right = left + 1;
      const child = right < this.values.length
        && this.values[right].localeCompare(this.values[left]) < 0
        ? right
        : left;
      if (this.values[child].localeCompare(tail) >= 0) break;
      this.values[index] = this.values[child];
      index = child;
    }
    this.values[index] = tail;
    return first;
  }

  get size() {
    return this.values.length;
  }
}

const buildBatchGraph = (batches) => {
  if (!Array.isArray(batches) || batches.length === 0 || batches.length > MAX_LOGICAL_AGENTS) {
    throw new TypeError(`batches must contain between 1 and ${MAX_LOGICAL_AGENTS} batches`);
  }
  const byId = new Map();
  for (const [index, batch] of batches.entries()) {
    requirePlainObject(batch, `batches[${index}]`);
    const id = requireString(batch.id, `batches[${index}].id`, {
      maximum: 128,
      pattern: IDENTIFIER,
    });
    if (byId.has(id)) throw new TypeError(`duplicate batch id ${id}`);
    if (!Array.isArray(batch.dependsOn) || batch.dependsOn.length > MAX_LOGICAL_AGENTS) {
      throw new TypeError(`batch ${id} dependsOn must be a bounded array`);
    }
    const dependsOn = batch.dependsOn.map((dependency, dependencyIndex) => requireString(
      dependency,
      `batch ${id} dependsOn[${dependencyIndex}]`,
      { maximum: 128, pattern: IDENTIFIER },
    ));
    if (new Set(dependsOn).size !== dependsOn.length) {
      throw new TypeError(`batch ${id} contains duplicate dependencies`);
    }
    if (dependsOn.includes(id)) throw new TypeError(`batch ${id} cannot depend on itself`);
    byId.set(id, { id, dependsOn: Object.freeze([...dependsOn].sort()), source: batch });
  }

  const dependents = new Map([...byId.keys()].map((id) => [id, []]));
  const indegree = new Map();
  for (const batch of byId.values()) {
    indegree.set(batch.id, batch.dependsOn.length);
    for (const dependency of batch.dependsOn) {
      if (!byId.has(dependency)) {
        throw new TypeError(`batch ${batch.id} depends on unknown batch ${dependency}`);
      }
      dependents.get(dependency).push(batch.id);
    }
  }
  for (const values of dependents.values()) values.sort();

  const ready = new StringMinHeap();
  for (const [id, degree] of indegree) if (degree === 0) ready.push(id);
  const orderedIds = [];
  while (ready.size > 0) {
    const id = ready.pop();
    orderedIds.push(id);
    for (const dependent of dependents.get(id)) {
      const remaining = indegree.get(dependent) - 1;
      indegree.set(dependent, remaining);
      if (remaining === 0) ready.push(dependent);
    }
  }
  if (orderedIds.length !== batches.length) {
    const cyclic = [...indegree.entries()]
      .filter(([, degree]) => degree > 0)
      .map(([id]) => id)
      .sort();
    throw new TypeError(`batch DAG contains a cycle involving: ${cyclic.join(", ")}`);
  }

  const orderById = new Map(orderedIds.map((id, index) => [id, index]));
  const waveById = new Map();
  const wordCount = Math.ceil(orderedIds.length / 32);
  const ancestorsById = new Map();
  for (const id of orderedIds) {
    const batch = byId.get(id);
    const wave = batch.dependsOn.length === 0
      ? 0
      : Math.max(...batch.dependsOn.map((dependency) => waveById.get(dependency))) + 1;
    waveById.set(id, wave);
    const ancestors = new Uint32Array(wordCount);
    for (const dependency of batch.dependsOn) {
      const dependencyAncestors = ancestorsById.get(dependency);
      for (let word = 0; word < wordCount; word += 1) {
        ancestors[word] |= dependencyAncestors[word];
      }
      const dependencyOrder = orderById.get(dependency);
      ancestors[Math.floor(dependencyOrder / 32)] |= 1 << (dependencyOrder % 32);
    }
    ancestorsById.set(id, ancestors);
  }

  const isAncestor = (possibleAncestor, possibleDescendant) => {
    const ancestorOrder = orderById.get(possibleAncestor);
    const bits = ancestorsById.get(possibleDescendant);
    if (ancestorOrder === undefined || !bits) return false;
    return (bits[Math.floor(ancestorOrder / 32)] & (1 << (ancestorOrder % 32))) !== 0;
  };
  const isOrdered = (left, right) => left !== right
    && (isAncestor(left, right) || isAncestor(right, left));

  return {
    byId,
    orderedIds,
    orderById,
    waveById,
    isOrdered,
  };
};

export const validateBatchGraph = (batches) => {
  const graph = buildBatchGraph(batches);
  return deepFreeze(graph.orderedIds.map((id) => ({
    id,
    dependsOn: graph.byId.get(id).dependsOn,
    order: graph.orderById.get(id),
    wave: graph.waveById.get(id),
  })));
};

const properAncestorPaths = (filePath) => {
  if (filePath === "") return [];
  const parts = filePath.split("/");
  const ancestors = [""];
  for (let length = 1; length < parts.length; length += 1) {
    ancestors.push(parts.slice(0, length).join("/"));
  }
  return ancestors;
};

/** Reject overlapping write-scope agents unless their batches are DAG-ordered. */
export const validateWriterScopes = (agents, batches) => {
  if (!Array.isArray(agents)) throw new TypeError("agents must be an array");
  const graph = buildBatchGraph(batches);
  const entries = [];
  for (const [agentIndex, agent] of agents.entries()) {
    requirePlainObject(agent, `agents[${agentIndex}]`);
    if (!WRITE_SCOPE_ROLE_IDS.includes(agent.roleId)) continue;
    if (!graph.byId.has(agent.batchId)) {
      throw new TypeError(`writer agent ${agent.logicalKey} references unknown batch ${agent.batchId}`);
    }
    if (!Array.isArray(agent.scope) || agent.scope.length === 0) {
      throw new TypeError(`writer agent ${agent.logicalKey} must own at least one scope`);
    }
    for (const selector of agent.scope) {
      entries.push({
        ...parseScopeSelector(selector, `writer agent ${agent.logicalKey} scope`),
        batchId: agent.batchId,
        owner: agent.logicalKey,
      });
    }
  }
  entries.sort((left, right) => left.path.split("/").length - right.path.split("/").length
    || left.path.localeCompare(right.path)
    || left.kind.localeCompare(right.kind)
    || (left.symbol ?? "").localeCompare(right.symbol ?? "")
    || left.owner.localeCompare(right.owner));

  const allByPath = new Map();
  const treesByPath = new Map();
  const append = (map, key, entry) => {
    const values = map.get(key) ?? [];
    values.push(entry);
    map.set(key, values);
  };
  const conflictWith = (entry, candidates) => candidates.find((candidate) => candidate.owner !== entry.owner
    && !graph.isOrdered(entry.batchId, candidate.batchId));

  for (const entry of entries) {
    let conflict;
    for (const ancestor of properAncestorPaths(entry.path)) {
      conflict = conflictWith(entry, treesByPath.get(ancestor) ?? []);
      if (conflict) break;
    }
    if (!conflict) {
      // Distinct symbol leases still patch the same physical file, so two
      // writers on that file are serialized even when symbol names differ.
      conflict = conflictWith(entry, allByPath.get(entry.path) ?? []);
    }
    if (conflict) {
      throw new TypeError(
        `writer scopes overlap without a DAG dependency: ${conflict.owner} (${conflict.selector}) and ${entry.owner} (${entry.selector})`,
      );
    }

    append(allByPath, entry.path, entry);
    if (entry.kind === "tree") append(treesByPath, entry.path, entry);
  }
  return true;
};

const normalizeAssignment = (candidate, { batchId, registry }) => {
  const assignment = requirePlainObject(candidate, `batch ${batchId} assignment`);
  requireFields(assignment, {
    required: ["id", "roleId", "roleVersion", "count", "context"],
    allowed: ["id", "roleId", "roleVersion", "count", "scope", "scopeSets", "context"],
  }, `batch ${batchId} assignment`);
  const id = requireString(assignment.id, `batch ${batchId} assignment id`, {
    maximum: 128,
    pattern: IDENTIFIER,
  });
  if (!ROLE_IDS.includes(assignment.roleId)) {
    throw new TypeError(`batch ${batchId} assignment ${id} has an unknown role`);
  }
  const role = getRoleDefinition(registry, assignment.roleId, assignment.roleVersion);
  const count = requireInteger(
    assignment.count,
    `batch ${batchId} assignment ${id} count`,
    1,
    MAX_LOGICAL_AGENTS,
  );
  const hasScope = Object.hasOwn(assignment, "scope");
  const hasScopeSets = Object.hasOwn(assignment, "scopeSets");
  if (hasScope === hasScopeSets) {
    throw new TypeError(`batch ${batchId} assignment ${id} must define exactly one of scope or scopeSets`);
  }
  let scopeSets;
  if (hasScope) {
    const commonScope = normalizeScopeSet(
      assignment.scope,
      `batch ${batchId} assignment ${id} scope`,
    );
    scopeSets = Array.from({ length: count }, () => commonScope);
  } else {
    if (!Array.isArray(assignment.scopeSets) || assignment.scopeSets.length !== count) {
      throw new TypeError(
        `batch ${batchId} assignment ${id} scopeSets must contain exactly ${count} entries`,
      );
    }
    scopeSets = assignment.scopeSets.map((scopeSet, index) => normalizeScopeSet(
      scopeSet,
      `batch ${batchId} assignment ${id} scopeSets[${index}]`,
    ));
  }
  if (WRITE_SCOPE_ROLE_IDS.includes(role.role_id)) {
    if (hasScope && count > 1) {
      throw new TypeError(
        `writer assignment ${batchId}/${id} must provide one disjoint scopeSets entry per agent`,
      );
    }
    if (scopeSets.some((scope) => scope.length === 0)) {
      throw new TypeError(`writer assignment ${batchId}/${id} contains an empty scope`);
    }
  }
  if (role.role_id === "integrator") {
    if (count !== 1 || scopeSets.some((scope) => scope.length !== 0)) {
      throw new TypeError(
        `integrator assignment ${batchId}/${id} must be one controlled CAS lane with no file scope`,
      );
    }
  }

  return deepFreeze({
    id,
    role,
    count,
    scopeSets,
    context: normalizeAssignmentContext(
      assignment.context,
      `batch ${batchId} assignment ${id} context`,
    ),
  });
};

const validateRegistry = (registry) => {
  requirePlainObject(registry, "role registry");
  if (registry.schema_version !== "switch-agent-role-registry/v1"
    || !/^[a-f0-9]{64}$/u.test(registry.registry_sha256 ?? "")) {
    throw new TypeError("role registry has an invalid schema version or digest");
  }
  for (const roleId of ROLE_IDS) getRoleDefinition(registry, roleId, "1.0");
  return registry;
};

export class AgentFactory {
  constructor({ registry, maxContextBytes = DEFAULT_MAX_CONTEXT_BYTES }) {
    this.registry = validateRegistry(registry);
    this.maxContextBytes = requireInteger(
      maxContextBytes,
      "maxContextBytes",
      1,
      ABSOLUTE_MAX_CONTEXT_BYTES,
    );
  }

  expand(candidate) {
    const specification = requirePlainObject(candidate, "run specification");
    requireFields(specification, {
      required: ["schemaVersion", "runId", "seed", "baseCommit", "context", "batches"],
    }, "run specification");
    if (specification.schemaVersion !== RUN_SPEC_SCHEMA_VERSION) {
      throw new TypeError(`run specification schemaVersion must be ${RUN_SPEC_SCHEMA_VERSION}`);
    }
    const runId = requireString(specification.runId, "run specification runId", {
      maximum: 128,
      pattern: IDENTIFIER,
    });
    const seed = requireInteger(
      specification.seed,
      "run specification seed",
      0,
      Number.MAX_SAFE_INTEGER,
    );
    const baseCommit = requireString(specification.baseCommit, "run specification baseCommit", {
      maximum: 64,
      pattern: COMMIT,
    });
    const runContext = normalizeRunContext(specification.context);

    if (!Array.isArray(specification.batches)
      || specification.batches.length < 1
      || specification.batches.length > MAX_COORDINATORS) {
      throw new TypeError(`batches must contain between 1 and ${MAX_COORDINATORS} coordinator cells`);
    }
    const rawGraph = buildBatchGraph(specification.batches);
    const normalizedById = new Map();
    let logicalAgentCount = 0;
    let integratorLaneCount = 0;
    for (const batchId of rawGraph.orderedIds) {
      const source = rawGraph.byId.get(batchId).source;
      requireFields(source, {
        required: ["id", "dependsOn", "assignments"],
      }, `batch ${batchId}`);
      if (!Array.isArray(source.assignments)
        || source.assignments.length === 0
        || source.assignments.length > MAX_LOGICAL_AGENTS) {
        throw new TypeError(`batch ${batchId} assignments must be a non-empty bounded array`);
      }
      const assignments = source.assignments.map((assignment) => normalizeAssignment(assignment, {
        batchId,
        registry: this.registry,
      }));
      assignments.sort((left, right) => left.id.localeCompare(right.id));
      if (new Set(assignments.map(({ id }) => id)).size !== assignments.length) {
        throw new TypeError(`batch ${batchId} contains duplicate assignment ids`);
      }
      const integrators = assignments.filter(({ role }) => role.role_id === "integrator");
      integratorLaneCount += integrators.length;
      if (integratorLaneCount > 1) {
        throw new TypeError("a run cannot contain more than one controlled CAS lane for its branch");
      }
      logicalAgentCount += assignments.reduce((sum, assignment) => sum + assignment.count, 0);
      if (logicalAgentCount > MAX_LOGICAL_AGENTS) {
        throw new TypeError(`run expands beyond ${MAX_LOGICAL_AGENTS} logical agents`);
      }
      normalizedById.set(batchId, { source, assignments });
    }

    const agents = [];
    const batches = [];
    const seenIds = new Set();
    const seenLogicalKeys = new Set();
    for (const batchId of rawGraph.orderedIds) {
      const normalized = normalizedById.get(batchId);
      const parentDigest = sha256({
        factory: FACTORY_SCHEMA_VERSION,
        runId,
        seed,
        batchId,
        registrySha256: this.registry.registry_sha256,
      });
      const parentId = `coord_${parentDigest.slice(0, 32)}`;
      const agentIds = [];
      for (const assignment of normalized.assignments) {
        const inheritedContext = deepFreeze({
          run: runContext,
          task: assignment.context,
        });
        const contextBytes = Buffer.byteLength(canonicalJson(inheritedContext), "utf8");
        if (contextBytes > this.maxContextBytes) {
          throw new TypeError(
            `bounded context for ${batchId}/${assignment.id} is ${contextBytes} bytes; maximum is ${this.maxContextBytes}`,
          );
        }
        const contextSha256 = sha256(inheritedContext);
        for (let index = 0; index < assignment.count; index += 1) {
          const scope = assignment.scopeSets[index];
          const logicalIdentityDigest = sha256({
            factory: FACTORY_SCHEMA_VERSION,
            seed,
            baseCommit,
            batchId,
            assignmentId: assignment.id,
            roleIdentitySha256: assignment.role.identity_sha256,
            instance: index + 1,
            scope,
            contextSha256,
          });
          const logicalKey = `logical_${logicalIdentityDigest}`;
          const operationalIdentityDigest = sha256({
            factory: FACTORY_SCHEMA_VERSION,
            runId,
            logicalKey,
          });
          const id = `agt_${operationalIdentityDigest.slice(0, 32)}`;
          if (seenIds.has(id) || seenLogicalKeys.has(logicalKey)) {
            throw new Error(`deterministic identity collision in ${batchId}/${assignment.id}`);
          }
          seenIds.add(id);
          seenLogicalKeys.add(logicalKey);
          agentIds.push(id);
          agents.push({
            id,
            logicalKey,
            idempotencyKey: `agent-factory/v1:${operationalIdentityDigest}`,
            ordinal: agents.length + 1,
            runId,
            parentId,
            batchId,
            roleId: assignment.role.role_id,
            roleVersion: assignment.role.role_version,
            roleIdentitySha256: assignment.role.identity_sha256,
            archetypeId: assignment.role.archetype_id,
            archetype: assignment.role.archetype,
            executionKind: assignment.role.execution_kind,
            writeAccess: assignment.role.write_access,
            baseCommit,
            scope,
            context: inheritedContext,
            contextBytes,
            contextSha256,
            capsule: assignment.role.capsule,
            capsuleSha256: assignment.role.capsule_sha256,
            outputSchema: assignment.role.out_schema,
            outputSchemaSha256: assignment.role.output_schema_sha256,
            status: "queued",
            attemptCount: 0,
          });
        }
      }
      batches.push({
        id: batchId,
        dependsOn: rawGraph.byId.get(batchId).dependsOn,
        order: rawGraph.orderById.get(batchId),
        wave: rawGraph.waveById.get(batchId),
        parentId,
        agentIds,
      });
    }

    validateWriterScopes(agents, batches);
    const planBody = {
      schemaVersion: FACTORY_SCHEMA_VERSION,
      runId,
      seed,
      baseCommit,
      registrySha256: this.registry.registry_sha256,
      logicalAgentCount,
      batches,
      agents,
    };
    const logicalKeysByAgentId = new Map(agents.map((agent) => [agent.id, agent.logicalKey]));
    const replayPlan = {
      schemaVersion: FACTORY_SCHEMA_VERSION,
      seed,
      baseCommit,
      registrySha256: this.registry.registry_sha256,
      logicalAgentCount,
      batches: batches.map(({ parentId: _parentId, agentIds, ...batch }) => ({
        ...batch,
        agentLogicalKeys: agentIds.map((agentId) => logicalKeysByAgentId.get(agentId)),
      })),
      agents: agents.map(({
        id: _id,
        runId: _runId,
        parentId: _parentId,
        idempotencyKey: _idempotencyKey,
        ...agent
      }) => agent),
    };
    const plan = { ...planBody, planSha256: sha256(replayPlan) };
    return deepFreeze({ ...plan, instancePlanSha256: sha256(plan) });
  }
}

export const createAgentPlan = async (specification, options = {}) => {
  const registry = options.registry ?? await loadRoleRegistry({ rolesRoot: options.rolesRoot });
  return new AgentFactory({
    registry,
    maxContextBytes: options.maxContextBytes ?? DEFAULT_MAX_CONTEXT_BYTES,
  }).expand(specification);
};

export const factoryInternals = Object.freeze({
  buildBatchGraph,
  normalizeScopeSet,
  treeContains,
});
