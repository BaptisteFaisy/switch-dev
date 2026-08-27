#!/usr/bin/env bash
set -euo pipefail

readonly CGROUP_MEMORY_CURRENT="/sys/fs/cgroup/memory.current"
readonly CGROUP_MEMORY_MAX="/sys/fs/cgroup/memory.max"
readonly LOCK_FILE="/tmp/cst-memory-watchdog.lock"
readonly PRESSURE_MARKER="/tmp/cst-memory-pressure-drain-until"

positive_integer() {
  local value="$1" fallback="$2"
  if [[ "$value" =~ ^[1-9][0-9]*$ ]]; then
    printf '%s' "$value"
  else
    printf '%s' "$fallback"
  fi
}

nonnegative_integer() {
  local value="$1" fallback="$2"
  if [[ "$value" =~ ^[0-9]+$ ]]; then
    printf '%s' "$value"
  else
    printf '%s' "$fallback"
  fi
}

server_port="${CST_BIND:-127.0.0.1:8080}"
server_port="${server_port##*:}"
server_port="$(positive_integer "$server_port" 8080)"
readonly ADMIN_DRAIN_URL="${CST_MEMORY_WATCHDOG_URL:-http://127.0.0.1:${server_port}/api/admin/drain}"

readonly REQUIRED_CONTAINER_HEADROOM_KIB=$((
  $(positive_integer "${CST_CHAT_MIN_CONTAINER_HEADROOM_MIB:-}" 768) * 1024
))
readonly REQUIRED_HOST_HEADROOM_KIB=$((
  $(positive_integer "${CST_CHAT_MIN_HOST_AVAILABLE_MIB:-}" 1024) * 1024
))
readonly CPU_LOAD_PERCENT_PER_CORE="$(
  nonnegative_integer "${CST_MEMORY_WATCHDOG_CPU_PERCENT:-}" 0
)"
readonly DRAIN_LEASE_SECONDS="$(
  positive_integer "${CST_MEMORY_WATCHDOG_DRAIN_SECONDS:-}" 120
)"
readonly DRAIN_REFRESH_SECONDS=$((DRAIN_LEASE_SECONDS / 2))
readonly POLL_SECONDS="$(positive_integer "${CST_MEMORY_WATCHDOG_POLL_SECONDS:-}" 5)"

exec 9>"$LOCK_FILE"
flock -n 9 || exit 0

resource_pressure_detected() {
  local current_bytes maximum_bytes container_headroom_kib host_headroom_kib
  local cpu_count load_one load_percent

  [[ "${CST_MEMORY_WATCHDOG_FORCE:-0}" == "1" ]] && return 0
  [[ -r "$CGROUP_MEMORY_CURRENT" && -r "$CGROUP_MEMORY_MAX" ]] || return 1

  current_bytes="$(<"$CGROUP_MEMORY_CURRENT")"
  maximum_bytes="$(<"$CGROUP_MEMORY_MAX")"
  host_headroom_kib="$(awk '/^MemAvailable:/ { print $2; exit }' /proc/meminfo)"

  if [[ "$maximum_bytes" == "max" ]]; then
    container_headroom_kib=$REQUIRED_CONTAINER_HEADROOM_KIB
  else
    container_headroom_kib=$(((maximum_bytes - current_bytes) / 1024))
  fi

  if ((
    container_headroom_kib < REQUIRED_CONTAINER_HEADROOM_KIB ||
    host_headroom_kib < REQUIRED_HOST_HEADROOM_KIB
  )); then
    return 0
  fi

  # Le cgroup limite deja le CPU. /proc/loadavg inclut les travaux en attente :
  # une compilation normale peut donc depasser le nombre de coeurs sans que le
  # noeud manque de ressources pour accepter un nouveau terminal ou chat.
  # Le seuil CPU reste disponible en opt-in ; 0 le desactive.
  if ((CPU_LOAD_PERCENT_PER_CORE == 0)); then
    return 1
  fi

  cpu_count="$(nproc)"
  read -r load_one _ </proc/loadavg
  load_percent="$(awk -v load="$load_one" -v cpus="$cpu_count" 'BEGIN { print int((load * 100) / cpus) }')"
  ((load_percent >= CPU_LOAD_PERCENT_PER_CORE))
}

pause_new_admission() {
  local now previous_deadline=0 next_deadline
  now="$(date +%s)"
  if [[ -r "$PRESSURE_MARKER" ]]; then
    read -r previous_deadline <"$PRESSURE_MARKER" || previous_deadline=0
  fi
  if ((previous_deadline - now > DRAIN_REFRESH_SECONDS)); then
    return
  fi
  [[ -n "${CST_ADMIN_TOKEN:-}" ]] || return

  if curl --fail --silent --show-error \
    --max-time 5 \
    --header "Authorization: Bearer $CST_ADMIN_TOKEN" \
    --header 'Content-Type: application/json' \
    --data "{\"draining\":true,\"ttlSeconds\":$DRAIN_LEASE_SECONDS}" \
    "$ADMIN_DRAIN_URL" >/dev/null; then
    next_deadline=$((now + DRAIN_LEASE_SECONDS))
    printf '%s\n' "$next_deadline" >"$PRESSURE_MARKER"
    printf '%(%Y-%m-%dT%H:%M:%SZ)T resource pressure: admission paused for %ss; active workloads preserved\n' \
      -1 "$DRAIN_LEASE_SECONDS"
  fi
}

expire_local_marker() {
  local now deadline=0
  [[ -r "$PRESSURE_MARKER" ]] || return 0
  now="$(date +%s)"
  read -r deadline <"$PRESSURE_MARKER" || deadline=0
  if ((deadline <= now)); then
    rm -f -- "$PRESSURE_MARKER"
  fi
}

while true; do
  if resource_pressure_detected; then
    pause_new_admission
  else
    # Le bail serveur expire naturellement. Ne jamais annuler ici un drain de
    # deploiement qui ne nous appartient pas.
    expire_local_marker
  fi
  [[ "${CST_MEMORY_WATCHDOG_ONCE:-0}" == "1" ]] && exit 0
  sleep "$POLL_SECONDS"
done
