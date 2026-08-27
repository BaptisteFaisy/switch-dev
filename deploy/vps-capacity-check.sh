#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' '=== host ==='
free -m || true
df -h / /srv 2>/dev/null || df -h /
printf '%s\n' '=== cgroup ==='
for f in /sys/fs/cgroup/memory.max /sys/fs/cgroup/memory.current /sys/fs/cgroup/pids.max /sys/fs/cgroup/pids.current; do
  [[ -r "$f" ]] && printf '%s: %s\n' "$f" "$(cat "$f")"
done
printf '%s\n' '=== recommendation ==='
echo 'Keep connected sessions at 30 and active agents at 8 until load tests prove otherwise.'
