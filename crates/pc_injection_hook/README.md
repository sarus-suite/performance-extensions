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

## Optional hook env vars

* `INJECTION_EXTRA_ENV`: semicolon-separated `KEY=VALUE` entries.
* `INJECTION_EXTRA_MOUNTS`: semicolon-separated mount entries in
  `source:destination:type:option1,option2,...` format.

Example:

```text
INJECTION_EXTRA_ENV=MPIR_CVAR_CH4_OFI_MULTI_NIC_STRIPING_THRESHOLD=100000000;FOO=bar
INJECTION_EXTRA_MOUNTS=/var/spool/slurmd:/var/spool/slurmd:none:x-create=dir,bind,rw,nosuid,noexec,nodev,private;/var/lib/hugetlbfs:/var/lib/hugetlbfs:bind:rbind,rw,nosuid,nodev,private
```
