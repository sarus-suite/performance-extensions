# Ubuntu 24.04 Vagrant test VM
Provisions an Ubuntu 24.04 guest with Podman plus the runtime pieces we need to test OCI hooks

## What

- install only the prerequisites from Ubuntu 24.04 packages
- install Podman itself from the static bundle used by the host-tools flow
- configure rootless Podman for the `vagrant` user
- sync this repository into the guest for iterative hook testing

## Usage

From this directory:

```bash
./prepare-cloud-image.sh
vagrant destroy -f
vagrant up
vagrant ssh
```

If you change files on the host and want to refresh the VM:

```bash
vagrant rsync
```

Once on the VM (via vagrant ssh):

```bash
cd /workspace/performance-extensions
cargo build --release
bats test
```

To focus on the Podman integration test:

```bash
cd /workspace/performance-extensions
cargo build --release
bats test/pce-podman.bats
```
