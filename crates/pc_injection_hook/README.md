# pc_injection_hook

Precreate hook that plans library injection from the container rootfs and rewrites the OCI config
to add bind mounts to inject host libs.

## What it does

* Reads the OCI runtime config from `stdin`.
* Finds the container rootfs from `root.path`.
* Discovers container libraries with `ldconfig -r <rootfs> -p`.
* Plans bind mounts for:
  * primary libraries from `INJECTION_PRIMARY_LIBS`
  * optional dependency libraries from `INJECTION_DEPENDENCY_LIBS`
  * optional extra files from `INJECTION_EXTRA_FILES`
  * optional extra OCI mounts from `INJECTION_EXTRA_MOUNTS`
* Appends the required mounts to the OCI config and writes the updated JSON to `stdout`.
* Adds `LD_LIBRARY_PATH` when the plan injects a runtime library directory instead of
  overwriting an existing container library path.
* Merges optional environment variables from `INJECTION_EXTRA_ENV` into `process.env`.

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
