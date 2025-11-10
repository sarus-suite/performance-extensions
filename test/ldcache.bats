#!/usr/bin/env bats

# Some hardcoded paths... TODO: fix this
: "${HOOKS_DIR:=/users/felipecr/workspace/sarus-hooks-rs/hooks.d}"
: "${IMAGE:=ubuntu:22.04}"


setup() {
  # Pre-get image
  podman pull "$IMAGE" >/dev/null
}


@test "ldcache check if updated cache when enabled" {
  # Get time with hook DISABLED
  run podman --hooks-dir="$HOOKS_DIR" run --rm \
    --annotation ldcache.enable=false \
    "$IMAGE" bash -lc 'stat -c "%Y %s" /etc/ld.so.cache'
  
  [ "$status" -eq 0 ]

  disabled_epoch="$(echo "$output" | awk "{print \$1}")"
  disabled_size="$(echo "$output"  | awk "{print \$2}")"
  echo "disabled: mtime=$disabled_epoch size=$disabled_size"

  # So cache might not update if there is not new lib detected
  # Build a temp image that includes a new library under /usr/local/lib
  TMP_IMGDIR="$(mktemp -d)"
  cat >"$TMP_IMGDIR/Dockerfile" <<'EOF'
FROM ubuntu:22.04
# Ensure a change is required at runtime
RUN rm -f /etc/ld.so.cache
EOF
  podman build -q -t ldcache-test:latest "$TMP_IMGDIR" >/dev/null
  rm -rf "$TMP_IMGDIR"

  # Get time with hook ENABLED and inject extracted lib
  run podman --hooks-dir="$HOOKS_DIR"\
	  run --rm \
	  --annotation ldcache.enable=true \
    ldcache-test:latest bash -lc 'stat -c "%Y %s" /etc/ld.so.cache'

  {
    printf '%s\n' "$output"
    printf '%s\n' "$stderr"
  } >&3

  [ "$status" -eq 0 ]


  enabled_epoch="$(echo "$output" | awk "{print \$1}")"
  enabled_size="$(echo "$output"  | awk "{print \$2}")"

  echo "enabled:  mtime=$enabled_epoch size=$enabled_size"

  if [[ "$enabled_epoch" -le "$disabled_epoch" ]]; then

  	  echo "Expected enabled mtime ($enabled_epoch) > disabled mtime ($disabled_epoch)"
	  echo "Sizes (disabled -> enabled): $disabled_size -> $enabled_size"

	  false
  fi
}

