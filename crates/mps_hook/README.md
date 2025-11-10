# NVIDIA MPS Bootstrap Hook

Ensure an **NVIDIA MPS** server is running for the current UID before your container starts.

**What it does**

* Tries to start `nvidia-cuda-mps-control -d`. If missing, exits `127` with a clear message. 
* Checks if `nvidia-cuda-mps-server` is already running **for this UID**; if yes, exits **0** (noop). Retries once if needed. 
* Fails cleanly with diagnostics if the server won’t come up. 

## Usage as a Podman hook

Run at **prestart** and gate with an annotation:

```json
{
  "version": "1.0.0",
  "hook": {
    "path": "/opt/hooks/mps_hook"
  },
  "when": {
    "always": false,
    "annotations": { "mps.enable": "^true$" }
  },
  "stages": ["prestart"]
}
```

> Tip: Set `mps.enable=true` on jobs that need to share GPU devices via MPS.

## Behavior & exit codes

* **0**: MPS server is already running for this UID, or started successfully. 
* **1**: Unexpected failure (e.g., could not start or re-check failed). 
* **127**: `nvidia-cuda-mps-control` not found in `PATH`. 

