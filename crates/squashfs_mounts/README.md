# SquashFS Mounts - createContainer Hook

Mounts SquashFS files from the host in the container.

**What it does**

On createContainer stage:
* Reads the **container state JSON** from `stdin`.
* Reads SquashFS mounts from com.hooks.squashfs_mounts annotation.
* Reads PARALLAX_MP_SQUASHFUSE_CMD environment variable, if missing uses "squashfuse_ll" as default.
* Create mount point directory tree for each mount, when missing.
* Mounts source SquashFS files at target mount points.

Mounts are removed at container stop.


**Requirements**

* Requires com.hooks.squashfs_mounts annotation with a specific string format:

```console
com.hooks.squashfs_mounts = "$SRC_1:$DST_1;$SRC_2:$DST_2;...;$SRC_N:$DST_N"
```


## Usage as a Podman hook

Add a `createContainer` hook entry similar to:

```json
{
  "version": "1.0.0",
  "hook": {
    "path": "/opt/hooks/squashfs_mounts"
  },
  "when": {
    "always": true
  },
  "stages": ["createContainer"]
}
```

### Exit statuses

`squashfs_mounts` reuses the shared precreate-hook categories and does not define hook-specific codes.

| Status | Category | Meaning for `squashfs_mounts` |
| ---: | --- | --- |
| 65 | `EX_DATAERR` | Malformed input from OCI or annotation |
| 69 | `EX_UNAVAILABLE` | Missing mount source or target  |
| 74 | `EX_IOERR` | Input or output I/O failure |
