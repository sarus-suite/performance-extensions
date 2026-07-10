# Precreate Injection Hook

Precreate hook that rewrites the OCI config to inject host libraries, files, mounts, and env vars.

## Configuration

Use CLI hook args as the primary configuration mechanism. Environment variables are still supported
as a legacy fallback.

If the same setting is provided by both CLI args and env vars, the CLI value wins for that setting.

## CLI Args

Add args entries as needed in the OCI hook `args` array. `--lib`, `--dependency-lib`, `--file`,
`--env`, and `--mount` may all be given more than once.

* `--ldconfig=/path/to/ldconfig`
  Specify the ldconfig binary on host to use by the hook, needed for manipulating the ld cache from the container bundle.
* `--lib=/host/path/to/libfoo.so.1.2.3`
  Use for primary libraries you want the container to use. If the container already exposes the
  same SONAME with the same major ABI, the hook mounts over every matching container path.
  Otherwise it injects the
  library through `/run/pc-injection` and updates `LD_LIBRARY_PATH`. Primary libraries must include
  at least a major ABI version. When fallback injection is used, symlink sources are resolved to
  the real library file.
* `--dependency-lib=/host/path/to/libbar.so.1.0.0`
  Use for supporting libraries that should be added without replacing container library paths. These
  are always injected through `/run/pc-injection` and `LD_LIBRARY_PATH`. A major ABI version is not
  required. Symlink sources are resolved to the real library file.
* `--file=/absolute/host/path`
  Use for non-library files that must appear at the same absolute path inside the container. The
  host file is bind-mounted to that same absolute path. Sources must be regular files, not
  symlinks.
* `--env=KEY=VALUE`
  Use to add runtime environment variables needed by the injected stack.
* `--mount=/source:/destination:option1,option2,...`
  Use for extra bind mounts required by the injected stack. CLI mounts are always bind mounts.
  Symlink sources are resolved to their canonical host path.
  Supported options: `bind`, `rbind`, `ro`, `rw`, `nosuid`, `suid`, `nodev`, `dev`, `noexec`,
  `exec`, `private`, `rprivate`, `slave`, `rslave`, `shared`, `rshared`, `x-create=dir`.

Note: At least one `--lib` or `--dependency-lib` is required.

Example with multiple resource injection entries:

```json
{
  "version": "1.0.0",
  "hook": {
    "path": "/opt/hooks/pc_injection_hook",
    "args": [
      "pc_injection_hook",
      "--lib=/host/lib/libmpi.so.12.0.1",
      "--lib=/host/lib/libucx.so.0.0.0",
      "--dependency-lib=/host/lib/libfabric.so.1.22.0",
      "--dependency-lib=/host/lib/libmega.so.66.99.0",
      "--file=/etc/awesome_assets/abc.driver",
      "--file=/etc/awesome_assets/xyz.driver",
      "--env=MPIR_CVAR_CH4_OFI_MULTI_NIC_STRIPING_THRESHOLD=100000000",
      "--env=THE_ANSWER=42",
      "--mount=/var/spool/slurmd:/var/spool/slurmd:rw,nosuid,noexec,nodev,private",
      "--mount=/tmp:/tmp:rw,nosuid,noexec,nodev,private"
    ]
  },
  "stages": ["precreate"]
}
```

## Legacy Env Vars

Use these only when CLI args are not practical.

* `LDCONFIG_PATH=/path/to/ldconfig`
* `INJECTION_PRIMARY_LIBS=/a/libfoo.so.1:/b/libbar.so.2`
* `INJECTION_DEPENDENCY_LIBS=/a/libdep.so.1:/b/libdep2.so.3`
* `INJECTION_EXTRA_FILES=/etc/awesome_assets/abc.driver:/etc/awesome_assets/xyz.driver`
* `INJECTION_EXTRA_ENV=KEY=VALUE;OTHER_KEY=OTHER_VALUE`
* `INJECTION_EXTRA_MOUNTS=/source:/destination:bind,rw,nosuid,noexec,nodev,private`

Notes:

* `INJECTION_PRIMARY_LIBS`, `INJECTION_DEPENDENCY_LIBS`, and `INJECTION_EXTRA_FILES` use the OS
  path-list format (`:` on Linux).
* `INJECTION_EXTRA_ENV` uses semicolon-separated `KEY=VALUE` entries.
* `INJECTION_EXTRA_MOUNTS` uses `source:destination:type:option1,option2,...`.
* Only bind-style extra mounts are supported. `type` may be `bind`, `none`, or empty.
