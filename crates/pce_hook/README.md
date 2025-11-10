# Precreate Container Edits Hook

Make simple, declarative edits to an OCI container config at **createContainer** time, controllable via annotation.

**What it does**

* Reads the **container config JSON** from `stdin` and emits the updated config to `stdout`. 
* Applies edits (env + mounts) from a JSON file pointed by `PCE_INPUT`.
* If `PCE_INPUT` is unset, the hooks does a no-op passthrough. 
* Pretty-prints output and exits non-zero on validation/parse errors (errors go to `stderr`). 

## Input configuration file

Minimal, CDI-like shape.

```json
{
  "precreate": "0.1.0",
  "containerEdits": [
    {
      "env": ["FOO=VALID_SPEC", "BAR=BARVALUE1"],
      "mounts": [
        {"hostPath":"/bin/vendorBin","containerPath":"/bin/vendorBin"},
        {"hostPath":"/usr/lib/libVendor.so.0","containerPath":"/usr/lib/libVendor.so.0"},
        {
          "hostPath":"tmpfs",
          "containerPath":"/tmp/data",
          "type":"tmpfs",
          "options":["nosuid","strictatime","mode=755","size=65536k"]
        }
      ]
    }
  ]
}
```

**Rules**

* `env`: array of strings, each `KEY=VALUE`. Non-string entries or missing `=` error. 
* `mounts`: array of objects with `hostPath` and `containerPath`; optional `type`, `options`. 

## Usage as a Podman hook

Add a createContainer hook entry similar to:

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
  },
  "stages": ["createContainer"]
}
```

