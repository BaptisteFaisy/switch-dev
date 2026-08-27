#!/usr/bin/env bash
set -euo pipefail

root="${1:?workspace root is required}"
target_dir="${2:-/tmp/cst-vps-server-zigbuild}"
commit="${3:-local-vps-zigbuild}"
tool_root="${XDG_CACHE_HOME:-$HOME/.cache}/cst-vps-zigbuild"

if [[ -f "$HOME/.cargo/env" ]]; then
  # shellcheck source=/dev/null
  source "$HOME/.cargo/env"
fi
command -v cargo >/dev/null
command -v curl >/dev/null
command -v python3 >/dev/null
command -v sha256sum >/dev/null

mkdir -p "$tool_root"

zig_metadata="$(
  curl --proto '=https' --tlsv1.2 -fsSL https://ziglang.org/download/index.json |
    python3 -c '
import json
import sys

data = json.load(sys.stdin)
versions = [
    (key, value)
    for key, value in data.items()
    if key != "master" and isinstance(value, dict) and "x86_64-linux" in value
]
if not versions:
    raise SystemExit("no stable Zig x86_64-linux release found")
version, release = max(
    versions,
    key=lambda item: tuple(int(part) for part in item[0].split(".")),
)
archive = release["x86_64-linux"]
print(version, archive["tarball"], archive["shasum"])
'
)"
read -r zig_version zig_url zig_sha <<<"$zig_metadata"

zig_archive="$tool_root/zig-$zig_version.tar.xz"
zig_dir="$tool_root/zig-x86_64-linux-$zig_version"
if [[ ! -x "$zig_dir/zig" ]]; then
  curl --proto '=https' --tlsv1.2 -fL "$zig_url" -o "$zig_archive"
  printf '%s  %s\n' "$zig_sha" "$zig_archive" | sha256sum --check -
  tar -xJf "$zig_archive" -C "$tool_root"
fi

zigbuild_root="$tool_root/cargo-zigbuild"
if [[ ! -x "$zigbuild_root/bin/cargo-zigbuild" ]]; then
  cargo install cargo-zigbuild --locked --root "$zigbuild_root"
fi

export PATH="$zig_dir:$zigbuild_root/bin:$PATH"
export CARGO_TARGET_DIR="$target_dir"
export CST_GIT_COMMIT="$commit"

cd "$root"
cargo zigbuild \
  --locked \
  --manifest-path src-tauri/Cargo.toml \
  --profile server \
  --bin cst-server \
  --target x86_64-unknown-linux-gnu.2.36

binary="$target_dir/x86_64-unknown-linux-gnu/server/cst-server"
test -x "$binary"
max_glibc="$(
  objdump -T "$binary" |
    grep -o 'GLIBC_[0-9.]*' |
    sort -V |
    tail -1
)"
if [[ "$(printf '%s\n%s\n' "$max_glibc" GLIBC_2.36 | sort -V | tail -1)" != "GLIBC_2.36" ]]; then
  printf 'incompatible glibc requirement: %s\n' "$max_glibc" >&2
  exit 1
fi

printf '%s %s\n' "$binary" "$max_glibc"
