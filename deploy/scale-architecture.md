# VPS scale architecture

## Current safe profile

The Azure VPS currently has 4 vCPU and 16 GiB RAM. The production container is
limited to 2.5 CPU, 6 GiB RAM, no swap, and 2048 PIDs. Therefore the supported
profile is **30 connected sessions with 8 active agents**, while new work waits
in the server's admission queue. This is not a promise that eight heavy builds
will fit; load tests must validate the workload mix before increasing limits.

Copy `deploy/vps-scale-profile.env.example` into the VPS runtime environment,
review it, and apply it only during a controlled drain. Do not set logical
capacity values to 400: those values do not add CPU or memory.

## Isolation rules

- Every run gets its own workspace/worktree and run identifier.
- Worktrees stay on persistent SSD; `/dev/shm` is reserved for small temporary
  files because tmpfs consumes RAM and disappears on restart.
- Worker processes inherit bounded heap/build parallelism.
- Jobs need a timeout, cancellation path, bounded retry count, and cleanup.
- A worker must never use another run's workspace path.

## Target service split

The first production step is a scheduler/worker split without moving the public
API. The API owns authentication, WebSockets, and durable state transitions.
The scheduler owns admission and leases. Workers execute jobs and report
heartbeats. Redis is used for short-lived locks and queue notifications.

PostgreSQL is required for durable sessions/runs/events before multiple API
replicas or multiple worker nodes are enabled. It can run locally initially,
but it must use a separate persistent volume and must be backed up with
`pg_dump`. Do not migrate production state by copying live database files.

## PostgreSQL rollout

1. Provision PostgreSQL with a persistent volume and a private-only listener.
2. Create a dedicated database and least-privilege role.
3. Apply versioned schema migrations.
4. Dual-write non-critical run metadata and compare counts/checksums.
5. Drain new work, take a verified backup, migrate the remaining metadata, and
   enable PostgreSQL reads behind a feature flag.
6. Keep the file/Redis fallback until at least one successful restore drill.

Suggested initial schema entities: `users`, `sessions`, `runs`, `workspaces`,
`run_events`, `worker_leases`, and `job_attempts`. Add uniqueness on run IDs and
workspace leases, and indexes on status plus updated time.

## Backups and cleanup

- `backup-vps-state.sh` creates a checksummed state archive and optionally a
  PostgreSQL custom dump.
- `restore-vps-state.sh` refuses to run without `CST_CONFIRM_RESTORE=YES` and
  creates a pre-restore archive.
- `cleanup-vps-workspaces.sh` is dry-run by default; review it before `--apply`.
- Keep at least 14 days of backups, and copy backups off the VPS for disaster
  recovery. A backup kept only on the same disk is not a complete backup.

## Metrics and alerts

Monitor host and container CPU throttling, memory.current/max, OOM kills, PID
usage, disk utilisation, queue depth, oldest queued job, active workers,
heartbeat age, run duration, error rate, Redis availability, PostgreSQL
availability, and WebSocket disconnects. Alert before disk reaches 80% and
memory reaches 85%.

## Scaling path

- Current VPS: 30 sessions / 8 active agents, after load testing.
- Next step: add a second worker node and keep API/state on the primary.
- Heavy production target: 16 vCPU / 64 GiB RAM or several worker nodes.
- Store large artifacts in object storage rather than the root disk.
- Use blue/green releases and drain-aware rollback for every runtime update.
