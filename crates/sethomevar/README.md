# Set HOME variable - Precreate Hook

Update container environment replacing HOME variable for running user with the one from the host system.

**What it does**

* Reads the **container config JSON** from `stdin` and emits the updated config to `stdout`. 
* Reads running user uid from container config.
* Finds the host HOME value through `getent passwd`
* Replace HOME entry in container config env
* Pretty-prints output and exits non-zero on validation/parse errors (errors go to `stderr`). 

## Usage as a Podman hook

Add a `precreate` hook entry similar to:

```json
{
  "version": "1.0.0",
  "hook": {
    "path": "/opt/hooks/sethomevar"
  },
  "when": {
    "always": true
  },
  "stages": ["precreate"]
}
```

## Error diagnostics

Podman currently discards `stderr` from hooks in its non-standard `precreate` stage. As a temporary
mitigation, failures are written both to `stderr` and to:

```text
/tmp/precreate-hooks-<effective-uid>/sethomevar.log
```

The directory is private to the hook's effective host UID (`0700`), and the append-only log is
created with mode `0600`. Records contain a UTC timestamp, hook name, UID, PID, exit status,
category, and escaped error message. They intentionally omit the OCI configuration and environment.

For a rootless invocation, inspect the log with:

```console
tail -n 20 "/tmp/precreate-hooks-$(id -u)/sethomevar.log"
```

The file has no application-level rotation and may be removed by normal `/tmp` cleanup. This
mechanism is intended only until Podman propagates precreate-hook diagnostics to its caller.

### Exit statuses

`sethomevar` reuses the shared precreate-hook categories and does not define hook-specific codes.

| Status | Category | Meaning for `sethomevar` |
| ---: | --- | --- |
| 65 | `EX_DATAERR` | Malformed OCI input or invalid `process.user.uid` |
| 69 | `EX_UNAVAILABLE` | `getent` or the requested account lookup is unavailable |
| 70 | `EX_SOFTWARE` | Invalid `getent` output or unexpected internal failure |
| 74 | `EX_IOERR` | Input or output I/O failure |
