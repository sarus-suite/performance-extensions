# ldCache Refresh Hook (`ldcache_hook`)

Refresh the container’s dynamic loader cache at **prestart**, controllable via annotation.

**What it does**

* Reads **OCI state JSON** from `stdin` to find the bundle path. Resolves the container rootfs from `config.json`. 
* Runs `ldconfig -v -r <rootfs>` (override with `LDCONFIG_PATH`). 
* Prints a short **/etc/ld.so.cache** summary (existence, size, mtime) to `stderr`. 
* Exits with the wrapped command’s status; `127` in case of error.

## Usage as a Podman hook

Add a **prestart** hook entry:

```json
{
  "version": "1.0.0",
  "hook": {
    "path": "/opt/hooks/ldcache_hook",
    "env": ["LDCONFIG_PATH=/sbin/ldconfig"]
  },
  "when": {
    "always": false,
    "annotations": { "ldcache.enable": "^true$" }
  },
  "stages": ["prestart"]
}
```

## Behavior & exit codes

* **0**: `ldconfig` ran and returned success.
* **Non-zero**: `ldconfig` returned failure; details on `stderr`.
* **127**: `ldconfig` not found in `PATH`. 

