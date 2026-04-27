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
* Appends the required mounts to the OCI config and writes the updated JSON to `stdout`.
* Adds `LD_LIBRARY_PATH` when the plan injects a runtime library directory instead of
  overwriting an existing container library path.

## Notes

* This hook pairs with `ldcache_hook` when it overwrites existing library paths that are
  already present in the container cache!
* When the plan introduces new lib injection paths, the hook also updates `LD_LIBRARY_PATH` because a
  prestart `ldconfig -r <rootfs>` run does not see runtime-only bind mounts. New lib injection are
  exposed through a host-side staging directory mounted at `/run/pc-injection/<library>`.
