bats_require_minimum_version 1.5.0

setup() {
  repo="$(git rev-parse --show-toplevel)"
  bin="$repo/target/release/sethomevar"
  hook_log="/tmp/precreate-hooks-$(id -u)/sethomevar.log"
}

log_size() {
  if [ -f "$hook_log" ]; then
    wc -c <"$hook_log"
  else
    printf '0\n'
  fi
}

@test "sethomevar writes categorized failures to its diagnostic log" {
  before="$(log_size)"

  run --separate-stderr bash -lc \
    "printf '%s\n' '{\"process\":{\"user\":{}}}' | \"$bin\""

  [ "$status" -eq 65 ]
  [[ "$stderr" == *"process.user.uid"* ]]
  [ -f "$hook_log" ]

  run tail -c "+$((before + 1))" "$hook_log"
  [ "$status" -eq 0 ]
  [[ "$output" == *"status=65 category=EX_DATAERR"* ]]
  [[ "$output" == *"process.user.uid"* ]]
}

@test "sethomevar success changes stdout without adding an error record" {
  uid="$(id -u)"
  expected_home="$(getent passwd "$uid" | cut -d: -f6)"
  before="$(log_size)"

  run --separate-stderr bash -lc \
    "printf '%s\n' '{\"process\":{\"user\":{\"uid\":$uid},\"env\":[]}}' | \"$bin\""

  [ "$status" -eq 0 ]
  [ -z "$stderr" ]
  actual_home="$(printf '%s' "$output" | jq -r '.process.env[] | select(startswith("HOME=")) | sub("^HOME="; "")')"
  [ "$actual_home" = "$expected_home" ]
  after="$(log_size)"
  [ "$after" -eq "$before" ]
}
