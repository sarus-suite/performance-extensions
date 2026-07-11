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

  if [[ -n "$dependency_lib" ]]; then
    cat >"$hooks_dir/pc-injection.json" <<EOF
{
  "version": "1.0.0",
  "hook": {
    "path": "$bin",
    "env": [
      "LDCONFIG_PATH=$ldconfig_path",
      "INJECTION_PRIMARY_LIBS=$primary_lib",
      "INJECTION_DEPENDENCY_LIBS=$dependency_lib"
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
  else
    cat >"$hooks_dir/pc-injection.json" <<EOF
{
  "version": "1.0.0",
  "hook": {
    "path": "$bin",
    "env": [
      "LDCONFIG_PATH=$ldconfig_path",
      "INJECTION_PRIMARY_LIBS=$primary_lib"
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
  fi

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

  if [[ -n "$dependency_lib" ]]; then
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
      "INJECTION_EXTRA_ENV=$extra_env"
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
  else
    cat >"$hooks_dir/pc-injection.json" <<EOF
{
  "version": "1.0.0",
  "hook": {
    "path": "$bin",
    "env": [
      "LDCONFIG_PATH=$ldconfig_path",
      "INJECTION_PRIMARY_LIBS=$primary_lib",
      "INJECTION_EXTRA_MOUNTS=$extra_mounts",
      "INJECTION_EXTRA_ENV=$extra_env"
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
  fi

  printf '%s\n' "$hooks_dir"
}

make_pc_injection_hook_dir_args() {
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

  if [[ -n "$dependency_lib" ]]; then
    cat >"$hooks_dir/pc-injection.json" <<EOF
{
  "version": "1.0.0",
  "hook": {
    "path": "$bin",
    "args": [
      "pc_injection_hook",
      "--ldconfig=$ldconfig_path",
      "--lib=$primary_lib",
      "--dependency-lib=$dependency_lib"
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
  else
    cat >"$hooks_dir/pc-injection.json" <<EOF
{
  "version": "1.0.0",
  "hook": {
    "path": "$bin",
    "args": [
      "pc_injection_hook",
      "--ldconfig=$ldconfig_path",
      "--lib=$primary_lib"
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
  fi

  printf '%s\n' "$hooks_dir"
}

make_pc_injection_hook_dir_args_with_extras() {
  local primary_lib="$1"
  local dependency_lib="$2"
  local ldconfig_path="$3"
  local extra_mount="$4"
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

  if [[ -n "$dependency_lib" ]]; then
    cat >"$hooks_dir/pc-injection.json" <<EOF
{
  "version": "1.0.0",
  "hook": {
    "path": "$bin",
    "args": [
      "pc_injection_hook",
      "--ldconfig=$ldconfig_path",
      "--lib=$primary_lib",
      "--dependency-lib=$dependency_lib",
      "--mount=$extra_mount",
      "--env=$extra_env"
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
  else
    cat >"$hooks_dir/pc-injection.json" <<EOF
{
  "version": "1.0.0",
  "hook": {
    "path": "$bin",
    "args": [
      "pc_injection_hook",
      "--ldconfig=$ldconfig_path",
      "--lib=$primary_lib",
      "--mount=$extra_mount",
      "--env=$extra_env"
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
  fi

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
      test -f /run/pc-injection/libpcitest.so.1.0.0/libpcitest.so.1.0.0
    '

  {
    printf '%s\n' "$output"
    printf '%s\n' "$stderr"
  } >&3

  assert_success
  assert_output --partial "LD_LIBRARY_PATH=/run/pc-injection/libpcitest.so.1.0.0"

  rm -rf "$workdir" "$hooks_dir"
}

@test "pc_injection_hook accepts args-based library injection in Podman" {
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
  src="$workdir/libpciargs.c"
  dependency_lib="$workdir/libpciargs.so.1.0.0"

  cat >"$src" <<'EOF'
int pciargs_value(void) { return 42; }
EOF

  run "$gcc_path" -shared -fPIC -Wl,-soname,libpciargs.so.1 -o "$dependency_lib" "$src"
  assert_success

  hooks_dir="$(make_pc_injection_hook_dir_args "$primary_lib" "$dependency_lib" "$ldconfig_path")"
  [ -n "$hooks_dir" ]

  run podman --hooks-dir="$hooks_dir" run --rm \
    --annotation pc-injection.enable=true \
    "$IMAGE" bash -lc '
      printf "LD_LIBRARY_PATH=%s\n" "${LD_LIBRARY_PATH:-}"
      test -d /run/pc-injection/libpciargs.so.1.0.0
      test -L /run/pc-injection/libpciargs.so.1.0.0/libpciargs.so.1
      test -f /run/pc-injection/libpciargs.so.1.0.0/libpciargs.so.1.0.0
    '

  {
    printf '%s\n' "$output"
    printf '%s\n' "$stderr"
  } >&3

  assert_success
  assert_output --partial "LD_LIBRARY_PATH=/run/pc-injection/libpciargs.so.1.0.0"

  rm -rf "$workdir" "$hooks_dir"
}

@test "pc_injection_hook accepts args-based unversioned primary overwrite in Podman" {
  : "${IMAGE:=ubuntu:24.04}"

  podman pull "$IMAGE" >/dev/null

  run command -v gcc
  assert_success
  gcc_path="$output"

  run command -v ldconfig
  assert_success
  ldconfig_path="$output"

  workdir="$(mktemp -d)"
  host_src="$workdir/libpciflag-host.c"
  host_lib="$workdir/libpciflag.so"
  image_src="$workdir/libpciflag-image.c"
  image_lib="$workdir/libpciflag-image.so"
  containerfile="$workdir/Containerfile"
  hooks_dir="$(mktemp -d)"
  image_tag="pc-injection-unversioned-primary-$$:latest"

  cat >"$host_src" <<'EOF'
const char *pciflag_marker(void) { return "host-marker"; }
EOF

  cat >"$image_src" <<'EOF'
const char *pciflag_marker(void) { return "container-marker"; }
EOF

  run "$gcc_path" -shared -fPIC -Wl,-soname,libpciflag.so -o "$host_lib" "$host_src"
  assert_success

  run "$gcc_path" -shared -fPIC -Wl,-soname,libpciflag.so -o "$image_lib" "$image_src"
  assert_success

  cat >"$containerfile" <<EOF
FROM $IMAGE
COPY $(basename "$image_lib") /usr/local/lib/libpciflag.so
RUN ldconfig
EOF

  run podman build -t "$image_tag" -f "$containerfile" "$workdir"
  assert_success

  repo="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
  bin="$repo/target/release/pc_injection_hook"
  [ -x "$bin" ]

  cat >"$hooks_dir/pc-injection.json" <<EOF
{
  "version": "1.0.0",
  "hook": {
    "path": "$bin",
    "args": [
      "pc_injection_hook",
      "--ldconfig=$ldconfig_path",
      "--lib=$host_lib",
      "--allow-unversioned-primary-overwrite"
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

  run podman --hooks-dir="$hooks_dir" run --rm \
    --annotation pc-injection.enable=true \
    "$image_tag" bash -lc '
      printf "LD_LIBRARY_PATH=%s\n" "${LD_LIBRARY_PATH:-}"
      printf "hook-target=%s\n" /usr/local/lib/libpciflag.so
      grep -aoE "host-marker|container-marker" /usr/local/lib/libpciflag.so || true
      [ -z "${LD_LIBRARY_PATH:-}" ] &&
      [ ! -e /run/pc-injection/libpciflag.so ] &&
      grep -aq "host-marker" /usr/local/lib/libpciflag.so &&
      ! grep -aq "container-marker" /usr/local/lib/libpciflag.so
    '

  assert_success
  assert_output --partial "LD_LIBRARY_PATH="
  assert_output --partial "hook-target=/usr/local/lib/libpciflag.so"
  assert_output --partial "host-marker"

  podman image rm -f "$image_tag" >/dev/null 2>&1 || true
  rm -rf "$workdir" "$hooks_dir"
}

@test "pc_injection_hook rejects unversioned primary without flag in Podman" {
  : "${IMAGE:=ubuntu:24.04}"

  podman pull "$IMAGE" >/dev/null

  run command -v gcc
  assert_success
  gcc_path="$output"

  run command -v ldconfig
  assert_success
  ldconfig_path="$output"

  workdir="$(mktemp -d)"
  host_src="$workdir/libpciflag-host.c"
  host_lib="$workdir/libpciflag.so"
  hooks_dir="$(mktemp -d)"
  hook_stderr="$workdir/hook.stderr"

  cat >"$host_src" <<'EOF'
const char *pciflag_marker(void) { return "host-marker"; }
EOF

  run "$gcc_path" -shared -fPIC -Wl,-soname,libpciflag.so -o "$host_lib" "$host_src"
  assert_success

  repo="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
  bin="$repo/target/release/pc_injection_hook"
  [ -x "$bin" ]

  cat >"$hooks_dir/pc-injection.json" <<EOF
{
  "version": "1.0.0",
  "hook": {
    "path": "$bin",
    "args": [
      "pc_injection_hook",
      "--ldconfig=$ldconfig_path",
      "--lib=$host_lib"
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

  run podman --hooks-dir="$hooks_dir" run --rm \
    --runtime=crun \
    --annotation pc-injection.enable=true \
    --annotation run.oci.hooks.stderr="$hook_stderr" \
    "$IMAGE" true

  assert_failure
  if [ -f "$hook_stderr" ]; then
    run grep -F "must contain at least a major ABI number" "$hook_stderr"
    assert_success
  else
    assert_output --partial "precreate hook"
  fi

  rm -rf "$workdir" "$hooks_dir"
}

@test "pc_injection_hook falls back for unversioned primary without same-name container lib in Podman" {
  : "${IMAGE:=ubuntu:24.04}"

  podman pull "$IMAGE" >/dev/null

  run command -v gcc
  assert_success
  gcc_path="$output"

  run command -v ldconfig
  assert_success
  ldconfig_path="$output"

  workdir="$(mktemp -d)"
  host_src="$workdir/libpciflag-host.c"
  host_lib="$workdir/libpciflag.so"
  hooks_dir="$(mktemp -d)"

  cat >"$host_src" <<'EOF'
const char *pciflag_marker(void) { return "host-marker"; }
EOF

  run "$gcc_path" -shared -fPIC -Wl,-soname,libpciflag.so -o "$host_lib" "$host_src"
  assert_success

  repo="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
  bin="$repo/target/release/pc_injection_hook"
  [ -x "$bin" ]

  cat >"$hooks_dir/pc-injection.json" <<EOF
{
  "version": "1.0.0",
  "hook": {
    "path": "$bin",
    "args": [
      "pc_injection_hook",
      "--ldconfig=$ldconfig_path",
      "--lib=$host_lib",
      "--allow-unversioned-primary-overwrite"
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

  run podman --hooks-dir="$hooks_dir" run --rm \
    --annotation pc-injection.enable=true \
    "$IMAGE" bash -lc '
      printf "LD_LIBRARY_PATH=%s\n" "${LD_LIBRARY_PATH:-}"
      printf "fallback-target=%s\n" /run/pc-injection/libpciflag.so/libpciflag.so
      grep -aoE "host-marker|container-marker" /run/pc-injection/libpciflag.so/libpciflag.so || true
      [ "${LD_LIBRARY_PATH:-}" = "/run/pc-injection/libpciflag.so" ] &&
      [ -d /run/pc-injection/libpciflag.so ] &&
      [ -f /run/pc-injection/libpciflag.so/libpciflag.so ] &&
      grep -aq "host-marker" /run/pc-injection/libpciflag.so/libpciflag.so
    '

  assert_success
  assert_output --partial "LD_LIBRARY_PATH=/run/pc-injection/libpciflag.so"
  assert_output --partial "fallback-target=/run/pc-injection/libpciflag.so/libpciflag.so"
  assert_output --partial "host-marker"

  rm -rf "$workdir" "$hooks_dir"
}

@test "pc_injection_hook stages symlinked dependency as real file plus alias in Podman" {
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
  src="$workdir/libpcisymlink.c"
  dependency_real="$workdir/libpcisymlink.so.1.0.0"
  dependency_link="$workdir/libpcisymlink.so.1"

  cat >"$src" <<'EOF'
int pcisymlink_value(void) { return 42; }
EOF

  run "$gcc_path" -shared -fPIC -Wl,-soname,libpcisymlink.so.1 -o "$dependency_real" "$src"
  assert_success
  ln -s "$(basename "$dependency_real")" "$dependency_link"

  hooks_dir="$(make_pc_injection_hook_dir "$primary_lib" "$dependency_link" "$ldconfig_path")"
  [ -n "$hooks_dir" ]

  run podman --hooks-dir="$hooks_dir" run --rm \
    --annotation pc-injection.enable=true \
    "$IMAGE" bash -lc '
      test "$LD_LIBRARY_PATH" = "/run/pc-injection/libpcisymlink.so.1" &&
      test -L /run/pc-injection/libpcisymlink.so.1/libpcisymlink.so.1 &&
      test -f /run/pc-injection/libpcisymlink.so.1/libpcisymlink.so.1.0.0 &&
      test "$(readlink /run/pc-injection/libpcisymlink.so.1/libpcisymlink.so.1)" = "libpcisymlink.so.1.0.0"
    '

  {
    printf '%s\n' "$output"
    printf '%s\n' "$stderr"
  } >&3

  assert_success

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
      "" \
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

@test "pc_injection_hook accepts args-based extra env and mount in Podman" {
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
  printf 'from-host-args\n' >"$extra_mount_src/marker"

  extra_mount="$extra_mount_src:/var/spool/slurmd:bind,rw,nosuid,noexec,nodev,private"
  extra_env="MPIR_CVAR_CH4_OFI_MULTI_NIC_STRIPING_THRESHOLD=100000000"

  hooks_dir="$(
    make_pc_injection_hook_dir_args_with_extras \
      "$primary_lib" \
      "" \
      "$ldconfig_path" \
      "$extra_mount" \
      "$extra_env"
  )"
  [ -n "$hooks_dir" ]

  run podman --hooks-dir="$hooks_dir" run --rm \
    --annotation pc-injection.enable=true \
    "$IMAGE" bash -lc '
      [ "$MPIR_CVAR_CH4_OFI_MULTI_NIC_STRIPING_THRESHOLD" = "100000000" ] &&
      [ -f /var/spool/slurmd/marker ] &&
      [ "$(cat /var/spool/slurmd/marker)" = "from-host-args" ]
    '

  {
    printf '%s\n' "$output"
    printf '%s\n' "$stderr"
  } >&3

  assert_success

  rm -rf "$workdir" "$hooks_dir"
}
