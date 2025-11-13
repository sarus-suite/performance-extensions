# Precreate Container Edits Hook

Make simple, declarative edits to an OCI container config at **createContainer** time, controllable via annotation.

**What it does**

* Reads the **container config JSON** from `stdin` and emits the updated config to `stdout`. 
* Applies edits (env + mounts) from a JSON file pointed by `PCE_INPUT`.
* If `PCE_INPUT` is unset, the hooks does a no-op passthrough. 
* Pretty-prints output and exits non-zero on validation/parse errors (errors go to `stderr`). 

## Input configuration file

Minimal, CDI-like shape

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

### Controlling when the hook runs

The hook is configured following the standard OCI hook, control of when it runs can be controlled with the `when` block in the hook JSON config. The fields in the `when` block are evaluated as a logical AND and must match for the hook to run.

One way to control execution is by annotation mathing. So only will execute when a specific annotation is present for the container to hook to run (this can be controlled by the EDF). Example

```json
  "when": {
    "annotations": { "pce.enable": "^true$" }
  }
```
In this case the value is matched using regex, so it only matched literal `true`. Note that any regex expression can be used here.

For having the hook to always run
```json
  "when": {
    "always": true
  }
```
For detailed explanation see the documentation [oci-hooks](https://github.com/containers/common/blob/main/pkg/hooks/docs/oci-hooks.5.md#100-hook-schema)
