import { canonicalJson, sha256 } from "./canonical.mjs";
import { assertJsonSchema } from "./json-schema.mjs";
import { SwitchClientError } from "./switch-client.mjs";
import { setTimeout as delay } from "node:timers/promises";

const TERMINAL_SWITCH_STATUSES = new Set(["completed", "failed", "cancelled"]);

const stripJsonFence = (text) => {
  const trimmed = text.trim();
  const match = trimmed.match(/^```(?:json)?\s*([\s\S]*?)\s*```$/iu);
  return match ? match[1].trim() : trimmed;
};

export const extractFinalText = (snapshot) => {
  const candidates = Array.isArray(snapshot?.parts)
    ? snapshot.parts
      .filter((part) => part?.kind === "text"
        && typeof part.text === "string"
        && part.text.trim() !== "")
      .map((part) => part.text)
    : [];
  return candidates.length === 0 ? null : candidates.map((text) => text.trim()).join("\n\n").trim();
};

const requireMaterial = (run, agent) => {
  const material = run.definition?.roleMaterials?.[agent.roleId];
  const context = run.definition?.contextCatalog?.[agent.contextSha256];
  if (!material || !context
    || material.roleVersion !== agent.roleVersion
    || material.identitySha256 !== agent.roleIdentitySha256
    || sha256(material.capsuleSource) !== agent.capsuleSha256
    || sha256(material.outputSchema) !== agent.outputSchemaSha256
    || sha256(context) !== agent.contextSha256) {
    const error = new Error(`immutable role material mismatch for ${agent.id}`);
    error.code = "ROLE_MATERIAL_MISMATCH";
    throw error;
  }
  return { material, context };
};

export const buildReadonlyChatRequest = (run, agent) => {
  if (run.executor?.kind !== "switch_readonly"
    || run.executor?.mode !== "ask"
    || run.executor?.realChatDispatchEnabled !== true
    || agent.executionKind !== "chat"
    || agent.writeAccess !== "read") {
    const error = new Error(`agent ${agent.id} is not eligible for read-only Switch dispatch`);
    error.code = "SWITCH_READONLY_ROLE_REFUSED";
    throw error;
  }
  const { material, context } = requireMaterial(run, agent);
  const assignment = {
    schema: "switch-mass-subagents/readonly-assignment/v1",
    runId: run.id,
    agentId: agent.id,
    logicalKey: agent.logicalKey,
    roleId: agent.roleId,
    roleVersion: agent.roleVersion,
    baseCommit: agent.baseCommit,
    scope: agent.scope,
    context,
  };
  const prompt = [
    "You are a read-only Switch development subagent.",
    "Do not edit files, execute write-capable tools, send messages, or change external state.",
    "Inspect only the bounded assignment below. Return exactly one JSON value matching the supplied schema, with no prose or Markdown fence.",
    "",
    "ROLE CAPSULE:",
    material.capsuleSource.trim(),
    "",
    "ASSIGNMENT:",
    canonicalJson(assignment),
    "",
    "REQUIRED OUTPUT JSON SCHEMA:",
    canonicalJson(material.outputSchema),
  ].join("\n");
  if (Buffer.byteLength(prompt, "utf8") > 131_072) {
    const error = new Error(`read-only prompt for ${agent.id} exceeds 131072 UTF-8 bytes`);
    error.code = "SWITCH_PROMPT_TOO_LARGE";
    throw error;
  }
  return Object.freeze({
    accountId: run.definition.accountId,
    sessionId: null,
    prompt,
    imageAttachments: [],
    projectDir: run.projectDir,
    mode: "ask",
    toolScope: "none",
    model: material.model,
    reasoningEffort: material.reasoning,
    appConnectors: [],
    appWriteApproved: false,
    agentTools: [],
    agentSkills: [],
    questionTool: false,
    proofTool: false,
    sourceChatKey: agent.sourceChatKey,
  });
};

const safeError = (error) => ({
  code: typeof error?.code === "string" ? error.code : "SWITCH_DISPATCH_ERROR",
  status: Number.isInteger(error?.status) ? error.status : null,
  messageSha256: sha256(String(error?.message ?? "unknown error")),
});

export class SwitchReadonlyExecutor {
  constructor({ switchClient, store, reconciliationDelaysMs = [0, 250, 1_000] }) {
    if (!switchClient?.chatDispatchEnabled || !store) {
      throw new TypeError("an enabled Switch client and store are required");
    }
    this.switchClient = switchClient;
    this.store = store;
    if (!Array.isArray(reconciliationDelaysMs)
      || reconciliationDelaysMs.length < 1
      || reconciliationDelaysMs.length > 5
      || reconciliationDelaysMs.some((value) => !Number.isInteger(value)
        || value < 0
        || value > 10_000)) {
      throw new TypeError("reconciliationDelaysMs must contain one to five bounded delays");
    }
    this.reconciliationDelaysMs = Object.freeze([...reconciliationDelaysMs]);
  }

  async dispatch(run, agent) {
    const request = buildReadonlyChatRequest(run, agent);
    try {
      const snapshot = await this.switchClient.startChatTurn(request);
      if (snapshot.sourceChatKey !== agent.sourceChatKey) {
        return { kind: "attention", reason: "start_source_chat_key_mismatch" };
      }
      return this.materializeSnapshot(run, agent, snapshot, { reconciled: false });
    } catch (error) {
      const ambiguous = error instanceof SwitchClientError
        && ["SWITCH_TIMEOUT", "SWITCH_NETWORK_ERROR", "SWITCH_ABORTED"].includes(error.code);
      if (error instanceof SwitchClientError && error.status === 429) {
        return { kind: "retry", reason: "switch_capacity" };
      }
      if (!ambiguous) {
        return { kind: "attention", reason: "start_rejected", error: safeError(error) };
      }
      try {
        for (const waitMs of this.reconciliationDelaysMs) {
          if (waitMs > 0) await delay(waitMs);
          const snapshot = await this.switchClient.findActiveChatTurnBySourceChatKey(
            agent.sourceChatKey,
            { accountId: run.definition.accountId },
          );
          if (snapshot) {
            return this.materializeSnapshot(run, agent, snapshot, { reconciled: true });
          }
        }
        return { kind: "attention", reason: "start_outcome_unknown", error: safeError(error) };
      } catch (reconciliationError) {
        return {
          kind: "attention",
          reason: "start_reconciliation_failed",
          error: safeError(reconciliationError),
        };
      }
    }
  }

  async poll(run, agent) {
    try {
      const snapshot = await this.switchClient.getChatTurn(agent.switchTurnId);
      if (String(snapshot.id) !== String(agent.switchTurnId)
        || snapshot.sourceChatKey !== agent.sourceChatKey) {
        return { kind: "attention", reason: "poll_identity_mismatch" };
      }
      return this.materializeSnapshot(run, agent, snapshot, { reconciled: false });
    } catch (error) {
      return { kind: "poll_error", error: safeError(error) };
    }
  }

  async activeTurnCount() {
    return (await this.switchClient.listActiveChatTurns()).length;
  }

  async reconcile(run, agent) {
    try {
      const snapshot = await this.switchClient.findActiveChatTurnBySourceChatKey(
        agent.sourceChatKey,
        { accountId: run.definition.accountId },
      );
      if (!snapshot) return { kind: "not_found" };
      return this.materializeSnapshot(run, agent, snapshot, { reconciled: true });
    } catch (error) {
      return { kind: "poll_error", error: safeError(error) };
    }
  }

  async materializeSnapshot(run, agent, snapshot, { reconciled }) {
    if (!TERMINAL_SWITCH_STATUSES.has(snapshot.status)) {
      return { kind: "running", snapshot, reconciled };
    }
    if (snapshot.status !== "completed") {
      const artifact = await this.store.putArtifact({
        schema: "switch-mass-subagents/switch-terminal/v1",
        runId: run.id,
        agentId: agent.id,
        sourceChatKey: agent.sourceChatKey,
        turnId: snapshot.id,
        status: snapshot.status,
        errorSha256: snapshot.error ? sha256(snapshot.error) : null,
      });
      return { kind: snapshot.status, snapshot, artifactSha256: artifact.sha256, reconciled };
    }

    const finalText = extractFinalText(snapshot);
    if (finalText === null) {
      return { kind: "attention", reason: "completed_without_text" };
    }
    let output;
    try {
      output = JSON.parse(stripJsonFence(finalText));
      const { material } = requireMaterial(run, agent);
      assertJsonSchema(output, material.outputSchema);
    } catch (error) {
      const artifact = await this.store.putArtifact({
        schema: "switch-mass-subagents/invalid-switch-output/v1",
        runId: run.id,
        agentId: agent.id,
        sourceChatKey: agent.sourceChatKey,
        turnId: snapshot.id,
        outputTextSha256: sha256(finalText),
        validation: safeError(error),
      });
      return {
        kind: "attention",
        reason: "role_output_schema_invalid",
        artifactSha256: artifact.sha256,
      };
    }
    const artifactPayload = {
      schema: "switch-mass-subagents/switch-output/v1",
      runId: run.id,
      agentId: agent.id,
      logicalKey: agent.logicalKey,
      roleIdentitySha256: agent.roleIdentitySha256,
      outputSchemaSha256: agent.outputSchemaSha256,
      sourceChatKey: agent.sourceChatKey,
      turnId: snapshot.id,
      status: snapshot.status,
      output,
    };
    const artifact = await this.store.putArtifact(artifactPayload);
    return {
      kind: "submitted",
      snapshot,
      artifactSha256: artifact.sha256,
      outputSha256: sha256(output),
      reconciled,
    };
  }
}

export const readonlyExecutorInternals = Object.freeze({
  requireMaterial,
  safeError,
  stripJsonFence,
});
