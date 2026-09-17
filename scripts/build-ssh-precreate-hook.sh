#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
WORKSPACE_NAME="$(basename "${ROOT_DIR}")"

case "$(uname -m)" in
  x86_64|amd64)
    target_arch=amd64
    target_triple=x86_64-unknown-linux-musl
    ;;
  aarch64|arm64)
    target_arch=arm64
    target_triple=aarch64-unknown-linux-musl
    ;;
  *)
    printf 'error: unsupported native build architecture: %s\n' "$(uname -m)" >&2
    exit 1
    ;;
esac

command -v devcontainer >/dev/null 2>&1 || {
  printf 'error: the devcontainer CLI is required\n' >&2
  exit 1
}

(
  cd /
  devcontainer up \
    --remove-existing-container \
    --workspace-folder "${ROOT_DIR}" \
    --config "${ROOT_DIR}/.devcontainer/glibc/devcontainer.json" >/dev/null
  devcontainer exec \
    --workspace-folder "${ROOT_DIR}" \
    --config "${ROOT_DIR}/.devcontainer/glibc/devcontainer.json" \
    bash -lc "cd /workspaces/${WORKSPACE_NAME} && TARGET_ARCH=${target_arch} ./scripts/build-hpc-sshd.sh"

  devcontainer up \
    --remove-existing-container \
    --workspace-folder "${ROOT_DIR}" \
    --config "${ROOT_DIR}/.devcontainer/devcontainer.json" >/dev/null
  devcontainer exec \
    --workspace-folder "${ROOT_DIR}" \
    --config "${ROOT_DIR}/.devcontainer/devcontainer.json" \
    bash -lc "cd /workspaces/${WORKSPACE_NAME} && cargo build --release --target ${target_triple} -p ssh_precreate_hook"
)

printf 'ssh_precreate_hook ready at %s\n' \
  "${ROOT_DIR}/target/${target_triple}/release/ssh_precreate_hook"
