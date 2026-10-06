# PMIx - Precreate Hook

Add PMIx environment variables and bind mounts to container config, based on SLURM and PMIx entries in container environment.

**What it does**

* Reads the **container config JSON** from `stdin` and emits the updated config to `stdout`. 
* Looks for SLURM_\*, and PMIX_\* environment variables from container config.
* Adds environment variables and bind mounts related to PMIx to container config if needed.
* Pretty-prints output and exits non-zero on validation/parse errors (errors go to `stderr`). 

## Usage as a Podman hook

Add a `precreate` hook entry similar to:

```json
{
  "version": "1.0.0",
  "hook": {
    "path": "/opt/hooks/pmix_hook"
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
<LOG_ROOT>/precreate-hooks-<effective-uid>/pmix_hook.log
```

The directory is private to the hook's effective host UID (`0700`), and the append-only log is
created with mode `0600`. Records contain a UTC timestamp, hook name, UID, PID, exit status,
category, and escaped error message. They intentionally omit the OCI configuration and environment.

`<LOG_ROOT>` is `$XDG_RUNTIME_DIR` if available, otherwise the hook falls back on `/tmp`.

For a rootless invocation, inspect the log with:

```console
tail -n 20 "<LOG_ROOT>/precreate-hooks-$(id -u)/pmix_hook.log"
```

The file has no application-level rotation and may be removed by normal `/tmp` cleanup. This
mechanism is intended only until Podman propagates precreate-hook diagnostics to its caller.

### Exit statuses

`pmix_hook` reuses the shared precreate-hook categories and does not define hook-specific codes.

| Status | Category | Meaning for `pmix_hook` |
| ---: | --- | --- |
| 65 | `EX_DATAERR` | Malformed OCI input or invalid `process.user.uid` |
| 69 | `EX_UNAVAILABLE` | `getent` or the requested account lookup is unavailable |
| 70 | `EX_SOFTWARE` | Invalid `getent` output or unexpected internal failure |
| 74 | `EX_IOERR` | Input or output I/O failure |
