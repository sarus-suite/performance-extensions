bats_require_minimum_version 1.5.0

setup() {
  repo="$(git rev-parse --show-toplevel)"
  bin="$repo/target/release/pce_hook"
  
  export repo bin
}


@test "PCE hook modify input container config" {
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

  run --separate-stderr bash -lc \
    "cat \"$fixture\" | PCE_INPUT=\"$PCE_INPUT\" \"$bin\""
  [ "$status" -ne 0 ]

#  {
#    printf '%s\n' "$output"
#    printf '%s\n' "$stderr"
#  } >&3

  # catch the invalid json message
  grep -qi 'invalid json' <<<"$stderr"
}

@test "PCE hook manages invalid type on env" {
  fixture="$repo/test/fixtures/container-config-sample.json"
  fixture_output="$repo/test/fixtures/container-config-sample.json"
  PCE_INPUT="$repo/test/fixtures/pce-input-sample-int-on-env.json"

  run --separate-stderr bash -lc \
    "cat \"$fixture\" | PCE_INPUT=\"$PCE_INPUT\" \"$bin\""
  [ "$status" -ne 0 ]

#  {
#    printf '%s\n' "$output"
#    printf '%s\n' "$stderr"
#  } >&3

  # catch the invalid type message
  grep -qi 'invalid type' <<<"$stderr"
}
