# SSH precreate hook

`ssh_precreate_hook` prepares per-user SSH access for a container.  At the OCI
`precreate` stage it reads the OCI configuration from standard input, creates or
reuses an Ed25519 identity below `/tmp/sarus-hook-<effective-uid>`, derives its
public key with host `ssh-keygen`, and bind-mounts the resulting `authorized_keys`
file at `/etc/ssh/hpc-dev-authorized_keys`.

The hook writes the modified OCI configuration to standard output. It needs
`ssh-keygen` on the host; its state directory and files are restricted to the
effective user (`0700` and `0600`, respectively). No hook environment variables
are required.

## OCI hook configuration

```json
{
  "version": "1.0.0",
  "hook": {
    "path": "/usr/local/libexec/ssh_precreate_hook",
    "args": ["/usr/local/libexec/ssh_precreate_hook"]
  },
  "when": { "always": true },
  "stages": ["precreate"]
}
```

The container image must configure `sshd` to use
`/etc/ssh/hpc-dev-authorized_keys` for the target account.

## Failure behavior

The hook deliberately fails rather than rotating a key or waiting on another
precreate invocation. A stale `/tmp/sarus-hook-<effective-uid>/.lock` must be
removed by the owner before retrying. Errors are also recorded through the shared
precreate diagnostics facility.
