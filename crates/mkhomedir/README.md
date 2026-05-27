# Make Home Directory - Prestart Hook

Creates home directory for running user in the container if missing

**What it does**

* Collect $HOME value for the current user from the system /etc/passwd
* Update /etc/passwd in the container for current user if $HOME is different from system one.
* Create $HOME directory in the container if missing

## Usage as a Podman hook

Add a prestart hook entry similar to:

```json
{
  "version": "1.0.0",
  "hook": {
    "path": "/opt/hooks/mkhomedir"
  },
  "when": {
    "always": true
  },
  "stages": ["createRuntime"]
}
```
