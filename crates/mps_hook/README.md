# NVIDIA MPS Bootstrap Hook

Ensure an **NVIDIA MPS** server is running for the current UID before your container starts.

**What it does**

* Starts or contacts `nvidia-cuda-mps-control -d`. If missing, exits `127` with a clear message.
* Asks the control daemon to start an MPS server for the current UID. Retries once if needed.
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

* **0**: MPS server for this UID was accepted by the control daemon.
* **1**: Unexpected failure (e.g., could not start or contact the control daemon).
* **127**: `nvidia-cuda-mps-control` not found in `PATH`. 
