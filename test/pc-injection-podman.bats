#!/usr/bin/env bats
bats_require_minimum_version 1.5.0
source /usr/local/lib/bats/bats-support/load.bash
source /usr/local/lib/bats/bats-assert/load.bash

make_pc_injection_hook_dir() {
  local primary_lib="$1"
  local dependency_lib="$2"
  local ldconfig_path="$3"
  local hooks_dir
  hooks_dir="$(mktemp -d)"

  local repo bin
  repo="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
  bin="$repo/target/release/pc_injection_hook"

  if [[ ! -x "$bin" ]]; then
    echo "pc_injection_hook binary not found at $bin." >&2
    rm -rf "$hooks_dir"
    return 1
  fi

  cat >"$hooks_dir/pc-injection.json" <<EOF
{
  "version": "1.0.0",
  "hook": {
    "path": "$bin",
    "env": [
      "LDCONFIG_PATH=$ldconfig_path",
      "INJECTION_PRIMARY_LIBS=$primary_lib",
      "INJECTION_DEPENDENCY_LIBS=$dependency_lib",
      "INJECTION_COMPATIBILITY=major"
    ]
  },
  "when": {
    "annotations": {
      "pc-injection.enable": "^true$"
    }
  },
  "stages": ["precreate"]
}
EOF

  printf '%s\n' "$hooks_dir"
}

make_pc_injection_hook_dir_with_extras() {
  local primary_lib="$1"
  local dependency_lib="$2"
  local ldconfig_path="$3"
  local extra_mounts="$4"
  local extra_env="$5"
  local hooks_dir
  hooks_dir="$(mktemp -d)"

  local repo bin
  repo="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
  bin="$repo/target/release/pc_injection_hook"

  if [[ ! -x "$bin" ]]; then
    echo "pc_injection_hook binary not found at $bin." >&2
    rm -rf "$hooks_dir"
    return 1
  fi

  cat >"$hooks_dir/pc-injection.json" <<EOF
{
  "version": "1.0.0",
  "hook": {
    "path": "$bin",
    "env": [
      "LDCONFIG_PATH=$ldconfig_path",
      "INJECTION_PRIMARY_LIBS=$primary_lib",
      "INJECTION_DEPENDENCY_LIBS=$dependency_lib",
      "INJECTION_EXTRA_MOUNTS=$extra_mounts",
      "INJECTION_EXTRA_ENV=$extra_env",
      "INJECTION_COMPATIBILITY=major"
    ]
  },
  "when": {
    "annotations": {
      "pc-injection.enable": "^true$"
    }
  },
  "stages": ["precreate"]
}
EOF

  printf '%s\n' "$hooks_dir"
}

@test "pc_injection_hook adds fallback LD_LIBRARY_PATH mounts in Podman" {
  : "${IMAGE:=ubuntu:24.04}"

  podman pull "$IMAGE" >/dev/null

  run command -v gcc
  assert_success
  gcc_path="$output"

  run command -v ldconfig
  assert_success
  ldconfig_path="$output"

  run bash -lc '
    while read -r line; do
      case "$line" in
        *"libz.so.1 "*)
          set -- $line
          printf "%s\n" "${!#}"
          exit 0
          ;;
      esac
    done < <(ldconfig -p)
    exit 1
  '
  assert_success
  assert_output --partial "/"
  primary_lib="$output"

  workdir="$(mktemp -d)"
  src="$workdir/libpcitest.c"
  dependency_lib="$workdir/libpcitest.so.1.0.0"

  cat >"$src" <<'EOF'
int pcitest_value(void) { return 42; }
EOF

  run "$gcc_path" -shared -fPIC -Wl,-soname,libpcitest.so.1 -o "$dependency_lib" "$src"
  assert_success

  hooks_dir="$(make_pc_injection_hook_dir "$primary_lib" "$dependency_lib" "$ldconfig_path")"
  [ -n "$hooks_dir" ]

  run podman --hooks-dir="$hooks_dir" run --rm \
    --annotation pc-injection.enable=false \
    "$IMAGE" bash -lc '
      [ -z "${LD_LIBRARY_PATH:-}" ] &&
      [ ! -e /run/pc-injection/libpcitest.so.1.0.0 ]
    '
  assert_success

  run podman --hooks-dir="$hooks_dir" run --rm \
    --annotation pc-injection.enable=true \
    "$IMAGE" bash -lc '
      printf "LD_LIBRARY_PATH=%s\n" "${LD_LIBRARY_PATH:-}"
      test -d /run/pc-injection/libpcitest.so.1.0.0
      test -L /run/pc-injection/libpcitest.so.1.0.0/libpcitest.so.1
      test -L /run/pc-injection/libpcitest.so.1.0.0/libpcitest.so.1.0.0
    '

  {
    printf '%s\n' "$output"
    printf '%s\n' "$stderr"
  } >&3

  assert_success
  assert_output --partial "LD_LIBRARY_PATH=/run/pc-injection/libpcitest.so.1.0.0"

  rm -rf "$workdir" "$hooks_dir"
}

@test "pc_injection_hook adds extra env and mount in Podman" {
  : "${IMAGE:=ubuntu:24.04}"

  podman pull "$IMAGE" >/dev/null

  run command -v ldconfig
  assert_success
  ldconfig_path="$output"

  run bash -lc '
    while read -r line; do
      case "$line" in
        *"libz.so.1 "*)
          set -- $line
          printf "%s\n" "${!#}"
          exit 0
          ;;
      esac
    done < <(ldconfig -p)
    exit 1
  '
  assert_success
  assert_output --partial "/"
  primary_lib="$output"

  workdir="$(mktemp -d)"
  extra_mount_src="$workdir/slurmd"
  mkdir -p "$extra_mount_src"
  printf 'from-host\n' >"$extra_mount_src/marker"

  extra_mounts="$extra_mount_src:/var/spool/slurmd:bind:bind,rw,nosuid,noexec,nodev,private"
  extra_env="MPIR_CVAR_CH4_OFI_MULTI_NIC_STRIPING_THRESHOLD=100000000"

  hooks_dir="$(
    make_pc_injection_hook_dir_with_extras \
      "$primary_lib" \
      "$primary_lib" \
      "$ldconfig_path" \
      "$extra_mounts" \
      "$extra_env"
  )"
  [ -n "$hooks_dir" ]

  run podman --hooks-dir="$hooks_dir" run --rm \
    --annotation pc-injection.enable=true \
    "$IMAGE" bash -lc '
      [ "$MPIR_CVAR_CH4_OFI_MULTI_NIC_STRIPING_THRESHOLD" = "100000000" ] &&
      [ -f /var/spool/slurmd/marker ]
    '

  {
    printf '%s\n' "$output"
    printf '%s\n' "$stderr"
  } >&3

  assert_success

  rm -rf "$workdir" "$hooks_dir"
}
