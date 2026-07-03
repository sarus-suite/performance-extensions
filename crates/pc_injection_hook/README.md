# Precreate Injection Hook

Precreate hook that plans library injection from the container rootfs and rewrites the OCI config
to add bind mounts to inject host libs.

## Architecture Overview

This hook is architected as a small compiler for OCI specs.

Its lifecycle in main.rs is a five-stage pipeline:

* Read the incoming OCI config JSON from stdin.
* Load hook inputs from the config plus environment variables.
* Discover what libraries the container already exposes.
* Plan a set of safe config edits.
* Apply those edits and emit a rewritten OCI config to stdout.

The core data model is:
* HookInputs is the input contract
* Library keeps the semantic unit of logic: path, parsed linker name, real name, and ABI version
* ConfigEdits is the planned output: mounts, LD\_LIBRARY\_PATH additions, extra mounts, extra env, and warnings

For each input library, the planning layer makes one decision: overwrite an existing container library path, or inject through a directory and extend LD\_LIBRARY\_PATH
Always deciding replacement if ABI mayor is respected, otherwise it does directory placement.

## Notes

* When the plan introduces new lib injection paths, the hook also updates `LD_LIBRARY_PATH` because a
  prestart `ldconfig -r <rootfs>` run does not see runtime-only bind mounts. New lib injection are
  exposed through a host-side staging directory mounted at `/run/pc-injection/<library>`.


## Hook args

The hook can also read repeated `args` entries from the OCI hook config. This keeps multi-line
hook definitions readable and avoids packing repeated values into semicolon-delimited env vars.

Supported args:

* `--ldconfig=/path/to/ldconfig`
* `--lib=/host/path/to/libfoo.so.1.2.3`
* `--dependency-lib=/host/path/to/libbar.so.1.0.0`
* `--file=/absolute/host/path`
* `--env=KEY=VALUE`
* `--mount=/source:/destination:bind,rw,nosuid,noexec,nodev,private`

Example:

```json
{
  "version": "1.0.0",
  "hook": {
    "path": "/opt/hooks/pc_injection_hook",
    "args": [
      "pc_injection_hook",
      "--ldconfig=/sbin/ldconfig",
      "--lib=/host/lib/libmpi.so.12.0.1",
      "--lib=/host/lib/libxyz.so.0.0.0",
      "--dependency-lib=/host/lib/libpcitest.so.1.0.0",
      "--file=/etc/awesome_assets/abc.driver",
      "--env=MPIR_CVAR_CH4_OFI_MULTI_NIC_STRIPING_THRESHOLD=100000000",
      "--env=ANOTHER_VARIABLE=42",
      "--mount=/var/spool/slurmd:/var/spool/slurmd:bind,rw,nosuid,noexec,nodev,private",
      "--mount=/var/lib/hugetlbfs:/var/lib/hugetlbfs:bind:rbind,rw,nosuid,nodev,private",
      "--mount=/tmp:/tmp:bind,rw,nosuid,noexec,nodev,private"
    ]
  },
  "when": {
    "annotations": {
      "com.hook.test.enabled": "^true$"
    }
  },
  "stages": ["precreate"]
}
```

## Optional hook env vars

* `INJECTION_EXTRA_ENV`: semicolon-separated `KEY=VALUE` entries.
* `INJECTION_EXTRA_MOUNTS`: semicolon-separated mount entries in
  `source:destination:type:option1,option2,...` format.

Example:

```text
INJECTION_EXTRA_ENV=MPIR_CVAR_CH4_OFI_MULTI_NIC_STRIPING_THRESHOLD=100000000;FOO=bar
INJECTION_EXTRA_MOUNTS=/var/spool/slurmd:/var/spool/slurmd:none:x-create=dir,bind,rw,nosuid,noexec,nodev,private;/var/lib/hugetlbfs:/var/lib/hugetlbfs:bind:rbind,rw,nosuid,nodev,private
```
