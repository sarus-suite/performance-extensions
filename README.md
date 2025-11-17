# Performance extensions. HPC features for Podman

Extensions that turn Podman into an HPC-ready runtime with Sarus-suite.
Annotation-driven. Fully static binaries. Works on any Linux node.



## Why it matters

* **Accelerators on demand:** Enable accelerated libraries, MPS, extra mounts, env tweaks — per container — via annotations.
* **Cluster-friendly:** Static builds = zero runtime deps. Drop the binaries onto heterogeneous systems.
* **Least surprise:** Control hook execution from the Sarus EDF via annotation conditions match. No image changes needed.



## Hooks

* *Precreate Container Edits* [(pce_hook)](https://github.com/sarus-suite/performance-extensions/tree/main/crates/pce_hook)
  Reads container config from `stdin`, applies env + mount edits from `PCE_INPUT`, writes updated config to `stdout`. Use at `createContainer`.

* *Refresh loader cache* [(ldcache_hook)](https://github.com/sarus-suite/performance-extensions/tree/main/crates/ldcache_hook)
  On `prestart`, runs `ldconfig -v -r <rootfs>` (override with `LDCONFIG_PATH`).

* *NVIDIA MPS bootstrap* [(mps_hook)](https://github.com/sarus-suite/performance-extensions/tree/main/crates/mps_hook)
  Starts `nvidia-cuda-mps-control -d`, checks per-UID server, returns helpful exit codes.



## Build

**Fast:**

```bash
cargo build --release
```

**Portable (recommended):** static `musl` builds via devcontainer
```bash
devcontainer up --workspace-folder .
devcontainer exec --workspace-folder . cargo build --release
```
> Needs "devcontainer cli"



## Configure (OCI hook schema)

**Example: ldconfig at prestart**

```json
{
  "version": "1.0.0",
  "hook": {
    "path": "/opt/hooks/ldcache_hook",
    "env": ["LDCONFIG_PATH=/sbin/ldconfig"]
  },
  "when": { "annotations": { "ldcache.enable": "true" } },
  "stages": ["prestart"]
}
```

**Example: PCE at precreate stage**

```json
{
  "version": "1.0.0",
  "hook": {
    "path": "/opt/hooks/pce_hook",
    "env": ["PCE_INPUT=/etc/hooks/pce-input.json"]
  },
  "when": {
    "always": false,
    "annotations": { "pce.enable": "^true$" }
  },  "stages": ["precreate"]
}
```



## Tests

```bash
bats test
```

