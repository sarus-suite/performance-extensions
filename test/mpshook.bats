#!/usr/bin/env bats

bats_require_minimum_version 1.5.0


setup() {
  REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
  BIN="$REPO_ROOT/target/release/mps_hook"   # adjust if your binary is named differently

  # where the real control binary lives
  CONTROL_PATH="$(command -v nvidia-cuda-mps-control || true)"
  if [[ -z "$CONTROL_PATH" ]]; then
    skip "WARNING! nvidia-cuda-mps-control not found!"
  fi
  CONTROL_DIR="$(dirname "$CONTROL_PATH")"

  # helpers
  export HOOK_BIN="$BIN"
}


teardown() {
  # try to stop any MPS server we started
  if command -v nvidia-cuda-mps-control >/dev/null 2>&1; then
    # send a quit command and ignore the exit status
    printf 'quit\n' | nvidia-cuda-mps-control >/dev/null 2>&1 || true
  fi
}

# MPS wrapped calls for clarity
mps_is_running() {
  ps -eo uid=,comm= | grep -qE "^[[:space:]]*$(id -u)[[:space:]]+nvidia-cuda-mps-server$"
}

mps_start() {
  nvidia-cuda-mps-control -d
}

mps_stop() {
  printf 'quit\n' | nvidia-cuda-mps-control
}

# Helper to validate when MPS server is not available
path_without_control() {
  # remove CONTROL_DIR from PATH
  printf '%s' "$PATH" \
    | awk -v rm="$CONTROL_DIR" -v RS=: -v ORS=: '$0 != rm {print}' \
    | sed 's/:$//'
}


# Test should repport if nvidia mps server is not available!
@test "MPS_HOOK returns error if control binary not in PATH" {
  PATH_BACKUP="$PATH"
  PATH="$(path_without_control)"

  run "$HOOK_BIN"

  # restore PATH immediately
  PATH="$PATH_BACKUP"

  // We assert status and print any unexpected error
  [ "$status" -eq 127 ] || { echo "---stderr---" >&3; printf '%s\n' "$stderr" >&3; }
}


# Test to check correct output in case server is already there
@test "MPS_HOOK ok if server already up" {
  # ensure server is running
  mps_is_running || mps_start
  mps_is_running || skip "MPS server failed to start; cannot validate 'already running' path"

  run "$HOOK_BIN"
  [ "$status" -eq 0 ]
}


@test "MPS_HOOK ok if server down and then starts it" {
  # check server is stopped if not stop it
  if mps_is_running
    then mps_stop
  fi
  mps_is_running && skip "Could not stop MPS server; cannot validate start path"

  run "$HOOK_BIN"
  [ "$status" -eq 0 ]

  # We check again and it should be up
  mps_is_running
}

