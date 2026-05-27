# Set HOME variable - Precreate Hook

Update container environment replacing HOME variable for running user with the one from the host system.

**What it does**

* Reads the **container config JSON** from `stdin` and emits the updated config to `stdout`. 
* Reads running user uid from container config.
* Find HOME host value via getpwuid_r
* Replace HOME entry in container config env
* Pretty-prints output and exits non-zero on validation/parse errors (errors go to `stderr`). 

## Usage as a Podman hook

Add a `precreate` hook entry similar to:

```json
{
  "version": "1.0.0",
  "hook": {
    "path": "/opt/hooks/sethomevar"
  },
  "when": {
    "always": true
  },
  "stages": ["precreate"]
}
```
