bats_require_minimum_version 1.5.0

setup() {
  repo="$(git rev-parse --show-toplevel)"
  bin="$repo/target/release/pce_hook"
  
  export repo bin
}


@test "PCE hook operates with ContainerEdits" {
  fixture="$repo/test/fixtures/container-config-sample.json"
  fixture_output="$repo/test/fixtures/container-config-sample-output-pce.json"
  PCE_INPUT="$repo/test/fixtures/pce-input-sample.json"

  run --separate-stderr bash -lc \
    "cat \"$fixture\" | PCE_INPUT=\"$PCE_INPUT\" \"$bin\" 2>/dev/null"
  [ "$status" -eq 0 ]

  # here we check we got what we expect
  expected="$(jq -cS . "$fixture_output")"
  actual="$(printf '%s' "$output" | jq -cS .)"
  [ "$actual" = "$expected" ]
}


@test "PCE hook modify only env" {
  fixture="$repo/test/fixtures/container-config-sample.json"
  fixture_output="$repo/test/fixtures/pce-container-config-sample-output-only-env.json"
  PCE_INPUT="$repo/test/fixtures/pce-input-sample-only-env.json"

  run --separate-stderr bash -lc \
    "cat \"$fixture\" | PCE_INPUT=\"$PCE_INPUT\" \"$bin\" 2>/dev/null"
  [ "$status" -eq 0 ]

  # here we check we got what we expect
  expected="$(jq -cS . "$fixture_output")"
  actual="$(printf '%s' "$output" | jq -cS .)"
  [ "$actual" = "$expected" ]
}

@test "PCE hook modify only mount" {
  fixture="$repo/test/fixtures/container-config-sample.json"
  fixture_output="$repo/test/fixtures/pce-container-config-sample-output-only-mount.json"
  PCE_INPUT="$repo/test/fixtures/pce-input-sample-only-mount.json"

  run --separate-stderr bash -lc \
    "cat \"$fixture\" | PCE_INPUT=\"$PCE_INPUT\" \"$bin\" 2>/dev/null"
  [ "$status" -eq 0 ]

  # here we check we got what we expect
  expected="$(jq -cS . "$fixture_output")"
  actual="$(printf '%s' "$output" | jq -cS .)"
  [ "$actual" = "$expected" ]
}

@test "PCE hook no change on empty" {
  fixture="$repo/test/fixtures/container-config-sample.json"
  fixture_output="$repo/test/fixtures/container-config-sample.json"
  PCE_INPUT="$repo/test/fixtures/pce-input-sample-empty.json"

  run --separate-stderr bash -lc \
    "cat \"$fixture\" | PCE_INPUT=\"$PCE_INPUT\" \"$bin\" 2>/dev/null"
  [ "$status" -eq 0 ]

  # here we check we got what we expect
  expected="$(jq -cS . "$fixture_output")"
  actual="$(printf '%s' "$output" | jq -cS .)"
  [ "$actual" = "$expected" ]
}

@test "PCE hook manages malformed input json" {
  fixture="$repo/test/fixtures/container-config-sample.json"
  fixture_output="$repo/test/fixtures/container-config-sample.json"
  PCE_INPUT="$repo/test/fixtures/pce-input-sample-malformed.json"
  hook_log="/tmp/precreate-hooks-$(id -u)/pce_hook.log"
  hook_log_size=0
  if [ -f "$hook_log" ]; then
    hook_log_size="$(wc -c <"$hook_log")"
  fi

  run --separate-stderr bash -lc \
    "cat \"$fixture\" | PCE_INPUT=\"$PCE_INPUT\" \"$bin\""
  [ "$status" -eq 78 ]

  # catch the invalid json message
  grep -qi 'invalid json' <<<"$stderr"
  [ -f "$hook_log" ]
  run tail -c "+$((hook_log_size + 1))" "$hook_log"
  [ "$status" -eq 0 ]
  [[ "$output" == *"status=78 category=EX_CONFIG"* ]]
  [[ "$output" == *"Invalid JSON"* ]]
}

@test "PCE hook manages invalid type on env" {
  fixture="$repo/test/fixtures/container-config-sample.json"
  fixture_output="$repo/test/fixtures/container-config-sample.json"
  PCE_INPUT="$repo/test/fixtures/pce-input-sample-int-on-env.json"
  hook_log="/tmp/precreate-hooks-$(id -u)/pce_hook.log"
  hook_log_size=0
  if [ -f "$hook_log" ]; then
    hook_log_size="$(wc -c <"$hook_log")"
  fi

  run --separate-stderr bash -lc \
    "cat \"$fixture\" | PCE_INPUT=\"$PCE_INPUT\" \"$bin\""
  [ "$status" -eq 78 ]

  # catch the invalid type message
  grep -qi 'invalid type' <<<"$stderr"
  [ -f "$hook_log" ]
  run tail -c "+$((hook_log_size + 1))" "$hook_log"
  [ "$status" -eq 0 ]
  [[ "$output" == *"status=78 category=EX_CONFIG"* ]]
  [[ "$output" == *"invalid type"* ]]
}
