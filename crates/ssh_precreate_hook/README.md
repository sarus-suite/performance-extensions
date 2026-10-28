# SSH precreate hook

`ssh_precreate_hook` prepares per-user SSH access for a container.  At the OCI
`precreate` stage it reads the OCI configuration from standard input, creates or
reuses an Ed25519 identity below `/tmp/sarus-hook-<host-uid>`, derives its
public key with host `ssh-keygen`, and bind-mounts the resulting `authorized_keys`
file at `/etc/ssh/hpc-dev-authorized_keys`.

The hook writes the modified OCI configuration to standard output. It needs
`ssh-keygen` on the host. Its state directory is restricted to the effective
user (`0700`) and its private-key material is `0600`. No hook environment
variables are required.

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

When `ssh.enable=true` selects the hook, it also injects these read-only files:

| Container path | Mode | Purpose |
| --- | --- | --- |
| `/etc/ssh/sshd_config.podman` | `0644` | Passwordless `sshd -i` configuration. |
| `/usr/local/bin/hpc-dev-sshd` | `0755` | Optional launcher that prepares a container-local host key and runs `sshd -i`. |
| `/usr/local/libexec/hpc-sshd` | `0755` | Architecture-specific `sshd`, `sshd-auth`, and `sshd-session` bundle. |

The executable mounts deliberately omit `noexec`; all mounts remain read-only,
`nosuid`, and `nodev`. The hook package must contain a bundle matching its target
architecture. The supplied binaries require glibc 2.34 or newer. The image must
also provide `/bin/sh`, `id`, `mkdir`, `chmod`, and `ssh-keygen`; it does not need
its own OpenSSH server. The hook never starts `sshd` automatically. The launcher
creates its host key and runs the bundled `sshd -t` automatically before starting
the bundled `sshd -i`. Use `hpc-dev-sshd --prepare` only to perform that
initialization and validation without starting an SSH session.

The three server executables are embedded in the hook at build time and staged
below `/tmp/sarus-hook-<host-uid>` in a content-addressed directory. This keeps a
matching OpenSSH trio together and prevents a hook upgrade from changing the
bundle seen by an already-running container.

Run `scripts/build-ssh-precreate-hook.sh` from the repository root for a complete
native build. It uses the pinned Debian 12 builder to create or reuse the matching
glibc OpenSSH assets, then builds the static musl hook in the regular devcontainer.
The lower-level `scripts/build-hpc-sshd.sh` command is intended to run inside the
glibc builder. Generated server executables and their provenance manifest are
kept under `assets/` but are deliberately excluded from Git.

## Supplying an authorized key

By default, the hook creates and reuses a per-user Ed25519 identity. To use an
existing public key instead, set the `ssh.authorized_key` annotation to one
complete, single-line OpenSSH public key:

```bash
podman run \
  --annotation ssh.enable=true \
  --annotation "ssh.authorized_key=$(<\"$HOME/.ssh/id_ed25519.pub\")" \
  …
```

The hook validates the key with `ssh-keygen` and mounts a content-addressed
authorized-keys file. Different public keys therefore remain isolated across
concurrently running containers for the same user.

## Failure behavior

The hook deliberately fails rather than rotating a key or waiting on another
precreate invocation. A stale `/tmp/sarus-hook-<host-uid>/.lock` must be
removed by the owner before retrying. Errors are also recorded through the shared
precreate diagnostics facility.

For rootless containers, the hook translates namespace UID 0 through
`/proc/self/uid_map` before selecting the host-UID directory. This prevents
different users' rootless namespaces from sharing `/tmp/sarus-hook-0`.
