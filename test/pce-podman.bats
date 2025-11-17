#!/usr/bin/env bats
bats_require_minimum_version 1.5.0

# Create a temporary hooks dir with a pce_hook OCI config
#   $1 = absolute path to PCE_INPUT JSON on the host
make_pce_hook_dir() {
  local pce_input="$1"
  local hooks_dir
  hooks_dir="$(mktemp -d)"

  local repo bin
  repo="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
  bin="$repo/target/release/pce_hook"

  if [[ ! -x "$bin" ]]; then
    echo "pce_hook binary not found at $bin." >&2
    rm -rf "$hooks_dir"
    return 1
  fi

  cat >"$hooks_dir/pce.json" <<EOF
{
  "version": "1.0.0",
  "hook": {
    "path": "$bin",
    "env": [ "PCE_INPUT=$pce_input" ]
  },
  "when": {
    "annotations": {
      "pce.enabled": "true"
    }
  },
  "stages": ["precreate"]
}
EOF

  printf '%s\n' "$hooks_dir"
}

@test "PCE hook adds env and mount to Podman" {
  : "${IMAGE:=ubuntu:22.04}"

  podman pull "$IMAGE" >/dev/null

  workdir="$(mktemp -d)"
  mount_src="$workdir/host-mount"
  mkdir -p "$mount_src"
  echo "marker-from-pce" >"$mount_src/marker"

  pce_input="$workdir/pce-input.json"
  cat >"$pce_input" <<EOF
{
  "precreate": "0.1.0",
  "containerEdits": [
    {
      "env": [
        "PCE_TEST_FOO=BAR",
        "PCE_TEST_MARKER=present"
      ],
      "mounts": [
        {
          "containerPath": "/pce-from-hook",
          "hostPath": "$mount_src",
          "type": "bind",
          "options" : ["rw", "rbind"]
        }
      ]
    }
  ]
}
EOF

  hooks_dir="$(make_pce_hook_dir "$pce_input")"
  [ -n "$hooks_dir" ]

  # Run with hook DISABLED: env + mount should NOT be present
  run podman --hooks-dir="$hooks_dir" run --rm \
    --annotation pce.enabled=false \
    "$IMAGE" bash -lc '
      [ -z "${PCE_TEST_FOO:-}" ] &&
      [ -z "${PCE_TEST_MARKER:-}" ] &&
      [ ! -d /pce-from-hook ]'

   [ "$status" -eq 0 ]

  # Run with hook ENABLED: env + mount should be present
  run podman --hooks-dir="$hooks_dir" run --rm --annotation pce.enabled="true" \
    "$IMAGE" bash -lc '
      [ "$PCE_TEST_FOO" = "BAR" ] &&
      [ "$PCE_TEST_MARKER" = "present" ] &&
      [ -d /pce-from-hook ] &&
      [ -f /pce-from-hook/marker ]
    '

  {
    printf '%s\n' $stdout
    printf '%s\n' $stderr
  } >&3

  [ "$status" -eq 0 ]

  rm -rf "$workdir" "$hooks_dir"
}

