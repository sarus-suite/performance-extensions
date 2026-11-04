# SSH precreate hook

`ssh_precreate_hook` prepares per-user SSH access for a container.  At the OCI
`precreate` stage it reads the OCI configuration from standard input, validates
the public key supplied by the required `ssh.authorized_key` annotation, and
bind-mounts it as `/etc/ssh/hpc-dev-authorized_keys`. It also installs the embedded
`container-ssh-proxy` helper into the host user's `~/.local/bin` on first use.

The hook writes the modified OCI configuration to standard output. It needs
`ssh-keygen` on the host. Its state directory is restricted to the effective
user (`0700`), and generated `authorized_keys` files are `0600`. The hook never
receives or stores the user's private key. No hook environment variables are
required.

## OCI hook configuration

The repository includes the Sarus Suite installation template at
[`hooks.d/70-ssh-precreate.json`](../../hooks.d/70-ssh-precreate.json). The
Sarus Suite installer replaces `@@SARUS_SUITE_HOOK_BIN@@` with the hook binary
directory for the selected system or user installation.

For a standalone installation, use the equivalent configuration with the
installed hook's absolute path:

```json
{
  "version": "1.0.0",
  "hook": {
    "path": "/usr/local/libexec/ssh_precreate_hook",
    "args": ["/usr/local/libexec/ssh_precreate_hook"]
  },
  "when": { "annotations": { "ssh.authorized_key": "^.+$" } },
  "stages": ["precreate"]
}
```

The container image must configure `sshd` to use
`/etc/ssh/hpc-dev-authorized_keys` for the target account.

When `ssh.authorized_key` selects the hook, it also injects these read-only files:

| Container path | Mode | Purpose |
| --- | --- | --- |
| `/etc/ssh/sshd_config.podman` | `0644` | Passwordless `sshd -i` configuration. |
| `/usr/local/bin/hpc-dev-sshd` | `0755` | Prepares a container-local SSH server host key and runs the bundled `sshd` in inetd mode (`sshd -i`). |
| `/usr/local/libexec/hpc-sshd` | `0755` | Architecture-specific `sshd`, `sshd-auth`, and `sshd-session` bundle. |

The executable mounts deliberately omit `noexec`; all mounts remain read-only,
`nosuid`, and `nodev`. The hook package must contain a bundle matching its target
architecture. The supplied binaries require glibc 2.34 or newer. The image must
also provide `/bin/sh`, `id`, `mkdir`, and `chmod`; it does not need its own
OpenSSH server or `ssh-keygen`. The hook never starts `sshd` automatically. The
launcher creates its server host key with the bundled `ssh-keygen` and runs the
bundled `sshd -t` automatically before starting the bundled `sshd -i`. Use
`hpc-dev-sshd --prepare` only to perform that initialization and validation
without starting an SSH session.

The three server executables are embedded in the hook at build time and staged
below `/run/user/<host-uid>/sarus-hook` in a content-addressed directory. This keeps a
matching OpenSSH trio together and prevents a hook upgrade from changing the
bundle seen by an already-running container.

## Host proxy installation

The hook embeds `assets/container-ssh-proxy` and, by default, installs or updates
it at `~/.local/bin/container-ssh-proxy`. The target home is resolved for the
mapped host UID. The hook only creates real directories owned by its effective
user, installs through an atomic rename, and refuses to overwrite an existing
file that is not marked as managed by performance-extensions.

Control this behavior with the `ssh.proxy.install` annotation:

| Value | Behavior |
| --- | --- |
| `auto` | Default. Install or update when safe; warn and continue if installation fails. |
| `required` | Fail container creation if the helper cannot be installed. |
| `never` | Do not install the helper. Use an administrator- or user-provided copy. |

Automatic installation requires rootless Podman and a home directory visible
both where the hook runs and on the login host used by the SSH `ProxyCommand`.
The proxy requires Python 3.6 or newer, Linux `/proc`, and access to the Podman
installation that started the container.

## Connecting from a workstation

An OpenSSH client entry can run the installed proxy through the cluster login
host:

```sshconfig
Host REMOTE_CONTAINER
    HostName REMOTE_CONTAINER
    User YOUR_USERNAME
    IdentityFile ~/.ssh/sarus-container
    IdentitiesOnly yes
    HostKeyAlias REMOTE_CONTAINER
    ProxyCommand ssh -T LOGIN_HOST 'exec "$HOME/.local/bin/container-ssh-proxy" connect'
```

Replace `YOUR_USERNAME` with the account name inside the container and
`LOGIN_HOST` with the SSH host or alias used to reach the remote system.
`REMOTE_CONTAINER` is a local SSH alias and does not need to resolve in DNS
because `ProxyCommand` supplies the connection transport.

The private key configured by `IdentityFile` must correspond to the public key
supplied through `ssh.authorized_key`. Host-key verification should remain
enabled for production use; if containers are ephemeral, inject or preserve a
stable host key rather than globally disabling `StrictHostKeyChecking`.

Run `scripts/build-ssh-precreate-hook.sh` from the repository root for a complete
native build. It uses the pinned Debian 12 builder to create or reuse the matching
glibc OpenSSH assets, then builds the static musl hook in the regular devcontainer.
The lower-level `scripts/build-hpc-sshd.sh` command is intended to run inside the
glibc builder. Generated server executables and their provenance manifest are
kept under `assets/` but are deliberately excluded from Git.

## Required authorized key

Set the required `ssh.authorized_key` annotation to one complete, single-line
OpenSSH public key. The user is responsible for retaining the corresponding
private key and configuring it in the workstation's `IdentityFile`:

```toml
[annotations]
"ssh.authorized_key" = "ssh-ed25519 AAAA... YOUR_USERNAME@WORKSTATION"
```

The equivalent direct Podman invocation is:

```bash
podman run \
  --annotation "ssh.authorized_key=$(cat "$HOME/.ssh/id_ed25519.pub")" \
  …
```

The hook validates the key with `ssh-keygen` and mounts a content-addressed
authorized-keys file. Different public keys therefore remain isolated across
concurrently running containers for the same user.

## Failure behavior

The supplied hook registration does not invoke the hook when
`ssh.authorized_key` is absent or empty. Once invoked, the hook fails if the key
is invalid; it also rejects a missing key when invoked directly or through an
alternate registration. A non-blocking kernel lock makes concurrent preparation
for the same host user fail immediately. The kernel releases that lock whenever
the owning process exits, including after a crash or forced termination. The
persistent `.lock` file is harmless and requires no cleanup. All hook errors are
also recorded through the shared precreate diagnostics facility. This lock
protects only precreate-time shared-state preparation; it does not limit SSH
connections. Each `container-ssh-proxy connect` invocation starts an independent
`sshd -i` session after hook preparation has completed.

The hook translates namespace UID 0 through `/proc/self/uid_map` before
selecting the host-UID directory. This prevents different users' rootless
namespaces from selecting the same state directory.
