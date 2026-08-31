# Switch mass-subagents sidecar

This directory implements the safe Phase 0 / Phase A foundation from
`PLAN-10000-MASS-SUBAGENTS.md` for **Switch development only**.

Implemented in this delivery:

- a versioned role registry and deterministic logical-agent factory;
- a bounded scheduler that maps many logical microtasks onto at most the
  configured Switch chat capacity;
- a write-ahead event log with crash recovery, atomic projections and
  content-addressed artifacts;
- a deterministic fake provider for success, `429`, timeout, duplicate and
  out-of-order scenarios;
- the cumulative L1-L8 verification contract and immutable evidence hashes;
- a versioned Switch HTTP client for probes and explicitly approved read-only
  chat turns (`POST/GET/DELETE /api/chat/turns`);
- crash-safe reservation/execution/commit dispatch outside the run-store lock,
  bounded by the configured chat capacity;
- strict JSON Schema validation and content-addressed storage of real read-only
  role outputs;
- development-target checks and reproducible Phase 0 / A foundation artifacts.

The real development executor is fail-closed by default. When it is enabled,
it accepts only roles whose immutable manifest says `executionKind=chat` and
`writeAccess=read`. Every request is forced to Switch `mode=ask` and
`toolScope=none`, with empty app, skill and tool lists and
`appWriteApproved=false`. Switch then issues no model-tool capability and
injects no Switch MCP server. The provider command also ignores inherited MCP
configuration and exposes only read-only built-ins. This no-tool path is
currently restricted to Codex and Claude accounts. A run must additionally send:

```json
{
  "executor": "switch_readonly",
  "realDispatchApproval": "switch-development-readonly/v1",
  "idempotencyKey": "caller-owned-request-001",
  "accountId": "<development account id>",
  "maxChatTurns": 1
}
```

`idempotencyKey` makes a byte-equivalent creation retry return the existing run;
reuse with a different request is refused. `maxChatTurns` must exactly match
the expanded logical-agent count. Only one
live real run is admitted by the pilot. A lost POST response is reconciled by
`accountId + sourceChatKey`; if it remains ambiguous, the agent stops in
`needs_attention` and the POST is never replayed blindly.

At startup, real mode requires `/healthz.chatTurnToolScopes` to contain
`"none"`. An older Switch runtime therefore fails closed instead of silently
ignoring the new request field. Ordinary Switch chat turns are included in the
same capacity budget, transient `429` responses are retried, persistent polling
errors are bounded, and shutdown waits for the active scheduler tick.

Still not enabled:

- executing generated code;
- writing to a shared workspace;
- integrating a patch or advancing a Git reference;
- promoting schema-valid read-only output to L1-L8 verified evidence;
- any production/VPS target.

Those remain blocked until the corresponding gates in the architecture plan
have reproducible evidence. The verifier deliberately reports
`foundation_passed` with `gateQualified: false`: a missing proof is never
promoted to a green Phase 0/A gate.

## Local verification

Run these commands from the repository root.

PowerShell:

```powershell
$env:SWITCH_ENV = 'development'
.\scripts\agents\verify-phase.ps1 -Phase 0 -Seed 42 -FoundationOnly
.\scripts\agents\verify-phase.ps1 -Phase A -Seed 42 -Scale 10 -FoundationOnly
```

POSIX shell:

```sh
SWITCH_ENV=development ./scripts/agents/verify-phase.sh 0 --seed 42 --foundation-only
SWITCH_ENV=development ./scripts/agents/verify-phase.sh A --seed 42 --scale 10 --foundation-only
```

The optional live authenticated read snapshot additionally reads
`SWITCH_DEV_BASE_URL` and `SWITCH_ADMIN_TOKEN` from the process environment.
It is not the full `P0-INT-001` launch/pilot/workspace contract. Tokens are
never written to logs or artifacts.

The supported active-development launch path is the native PowerShell wrapper,
which stages a content-addressed read-only release under the current user's
`LOCALAPPDATA`, keeps token/data ACLs private, and binds to loopback port 18084:

```powershell
pwsh -File .\scripts\start-mass-subagents-development.ps1 `
  -DeploymentRoot E:\AppsData\SwitchDevelopment
```

The wrapper accepts only the fixed port 18084, holds both its single-instance
mutex and Switch's shared deployment mutex during staging, and refuses to run a
sidecar or launcher tree that differs from Git `HEAD`. This makes the supported
host topology one sidecar process and one persistent run store.

This starts the deterministic fake executor only. Add `-EnableSwitchReadonly`
solely when the development Switch runtime containing `toolScope=none` is
already active and its local development credential is available. Stop through
the authenticated graceful endpoint:

```powershell
pwsh -File .\scripts\stop-mass-subagents-development.ps1 `
  -DeploymentRoot E:\AppsData\SwitchDevelopment
```

The Dockerfile is a reference packaging path. The active Switch development
tree does not qualify or launch it through Compose. If a separate Compose
overlay is used from the source snapshot, `MASS_SUBAGENTS_ADMIN_TOKEN` must be a
dedicated sidecar token distinct from `CST_ADMIN_TOKEN`.

To expose the real read-only path, all of the following are required in the
sidecar process environment:

```text
SWITCH_ENV=development
SWITCH_BASE_URL=http://127.0.0.1:18082
SWITCH_ADMIN_TOKEN=<development-only token>
MASS_SUBAGENTS_SWITCH_READONLY_ENABLED=true
MASS_SUBAGENTS_MAX_REAL_AGENTS=32
```

Startup refuses real mode unless Switch development reports ready and not
draining. A positive `/healthz.capacity` further caps the configured scheduler
capacity. Defaults also bound the poll interval (1 s), retained runs (100), and
each artifact (4 MiB). `POST /v1/admin/shutdown` performs an authenticated,
graceful shutdown; no process-kill command is part of the workflow.

The 10,000-agent qualification is a logical-agent smoke test with the
deterministic fake provider; it performs no network or paid-provider call:

```powershell
$env:SWITCH_ENV = 'development'
$env:MASS_SUBAGENTS_REAL_ENABLED = 'false'
npm --prefix .\sidecar\mass-subagents run smoke:10000 -- --repeats 1
```

Real chat dispatch remains deliberately capped at 32 read-only logical agents
per admitted pilot run. Scaling real provider calls to 10,000, enabling writes,
or promoting outputs to L1-L8 evidence requires later gates and explicit
authorization.
