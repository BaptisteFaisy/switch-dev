#!/usr/bin/env bash
set -u

readonly container_name="codex-switch-terminal-prepapp"
readonly memory_limit="9g"
readonly memory_limit_bytes="$((9 * 1024 * 1024 * 1024))"
readonly memory_swap_total="9g"
readonly memory_reservation="512m"
readonly memory_min_bytes="$((512 * 1024 * 1024))"
readonly memory_high_bytes="$((8 * 1024 * 1024 * 1024))"
readonly memory_swap_bytes="0"
readonly pids_limit="-1"

while true; do
  if [[ "$(docker inspect --format '{{.State.Running}}' "$container_name" 2>/dev/null)" != "true" ]]; then
    sleep 10
    continue
  fi

  current_memory=""
  current_swap=""
  current_pids=""
  read -r current_memory current_swap current_pids < <(
    docker inspect --format \
      '{{.HostConfig.Memory}} {{.HostConfig.MemorySwap}} {{.HostConfig.PidsLimit}}' \
      "$container_name" 2>/dev/null
  )
  if [[ "$current_memory" != "$memory_limit_bytes" \
      || "$current_swap" != "$memory_limit_bytes" \
      || "$current_pids" != "$pids_limit" ]]; then
    docker update \
      --memory "$memory_limit" \
      --memory-reservation "$memory_reservation" \
      --memory-swap "$memory_swap_total" \
      --pids-limit "$pids_limit" \
      "$container_name" >/dev/null 2>&1 || true
  fi

  container_pid="$(docker inspect --format '{{.State.Pid}}' "$container_name" 2>/dev/null)"
  container_cgroup="$(awk -F: '$1 == "0" { print $3; exit }' "/proc/$container_pid/cgroup" 2>/dev/null)"
  memory_min_path="/sys/fs/cgroup${container_cgroup}/memory.min"
  memory_high_path="/sys/fs/cgroup${container_cgroup}/memory.high"
  memory_swap_path="/sys/fs/cgroup${container_cgroup}/memory.swap.max"
  if [[ -n "$container_cgroup" && -w "$memory_min_path" ]]; then
    echo "$memory_min_bytes" >"$memory_min_path" 2>/dev/null || true
  fi
  if [[ -n "$container_cgroup" && -w "$memory_high_path" ]]; then
    # Pression douce a 8 Gio, sans tuer les chats ; plafond dur Docker a 9 Gio.
    echo "$memory_high_bytes" >"$memory_high_path" 2>/dev/null || true
  fi
  if [[ -n "$container_cgroup" && -w "$memory_swap_path" ]]; then
    echo "$memory_swap_bytes" >"$memory_swap_path" 2>/dev/null || true
  fi

  server_pid=""
  if [[ -n "$container_cgroup" && -r "/sys/fs/cgroup${container_cgroup}/cgroup.procs" ]]; then
    while read -r candidate_pid; do
      if [[ "$(cat "/proc/$candidate_pid/comm" 2>/dev/null)" == "cst-server" ]]; then
        server_pid="$candidate_pid"
        break
      fi
    done <"/sys/fs/cgroup${container_cgroup}/cgroup.procs"
  fi
  if [[ -n "$server_pid" ]]; then
    renice -n -10 -p "$server_pid" >/dev/null 2>&1 || true
  fi

  container_root="/proc/$container_pid/root"
  install \
    -m 0755 -o 10001 -g 10001 \
    "$container_root/srv/cst/bin/cst-node-memory-limited" \
    "$container_root/home/cst/.local/bin/node" >/dev/null 2>&1 || {
      sleep 10
      continue
    }

  # L'image lance deja son propre watchdog sous tini. Un second `docker exec`
  # toutes les deux secondes ne faisait qu'echouer et saturer le journal Docker.
  sleep 10
done
