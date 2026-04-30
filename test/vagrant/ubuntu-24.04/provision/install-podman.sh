#!/usr/bin/env bash
set -euo pipefail

REPO_MOUNT="${1:-/workspace/performance-extensions}"
VM_USER="${VM_USER:-vagrant}"
CACHE_DIR="${CACHE_DIR:-/var/cache/performance-extensions}"
DOWNLOAD_DIR="${CACHE_DIR}/downloads"
BUILD_DIR="${CACHE_DIR}/build"
PODMAN_STATIC_VERSION="${PODMAN_STATIC_VERSION:-latest}"
BATS_SUPPORT_REF="${BATS_SUPPORT_REF:-v0.3.0}"
BATS_ASSERT_REF="${BATS_ASSERT_REF:-v2.1.0}"

log() {
  printf '[vagrant-podman] %s\n' "$*"
}

require_root() {
  if [ "${EUID}" -ne 0 ]; then
    echo "this provisioner must run as root" >&2
    exit 1
  fi
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required command: $1" >&2
    exit 1
  }
}

detect_arch() {
  case "$(uname -m)" in
    aarch64|arm64)
      printf 'arm64\n'
      ;;
    x86_64|amd64)
      printf 'amd64\n'
      ;;
    *)
      echo "unsupported guest architecture: $(uname -m)" >&2
      exit 1
      ;;
  esac
}

podman_static_url() {
  local arch="$1"

  if [ -n "${PODMAN_STATIC_URL:-}" ]; then
    printf '%s\n' "${PODMAN_STATIC_URL}"
    return
  fi

  if [ "${PODMAN_STATIC_VERSION}" = "latest" ]; then
    printf 'https://github.com/mgoltzsche/podman-static/releases/latest/download/podman-linux-%s.tar.gz\n' "${arch}"
  else
    printf 'https://github.com/mgoltzsche/podman-static/releases/download/%s/podman-linux-%s.tar.gz\n' "${PODMAN_STATIC_VERSION}" "${arch}"
  fi
}

download_if_missing() {
  local url="$1"
  local dest="$2"

  if [ ! -f "${dest}" ]; then
    log "downloading ${url}"
    curl -fsSL "${url}" -o "${dest}"
  fi
}

append_subid_range() {
  local file="$1"
  local user="$2"
  local range_start="$3"
  local range_size="$4"

  if ! grep -q "^${user}:" "${file}"; then
    printf '%s:%s:%s\n' "${user}" "${range_start}" "${range_size}" >> "${file}"
  fi
}

configure_rootless_user() {
  local user="$1"
  local uid
  local gid
  local home

  uid="$(id -u "${user}")"
  gid="$(id -g "${user}")"
  home="$(getent passwd "${user}" | cut -d: -f6)"

  append_subid_range /etc/subuid "${user}" 100000 65536
  append_subid_range /etc/subgid "${user}" 100000 65536

  install -d -m 0700 -o "${uid}" -g "${gid}" "/run/user/${uid}"
  install -d -m 0755 -o "${uid}" -g "${gid}" "${home}/.config"
  install -d -m 0755 -o "${uid}" -g "${gid}" "${home}/.config/containers"

  cat > "${home}/.config/containers/containers.conf" <<EOF
[engine]
cgroup_manager = "cgroupfs"
events_logger = "file"
EOF
  chown -R "${uid}:${gid}" "${home}/.config"
}

install_packages() {
  export DEBIAN_FRONTEND=noninteractive

  apt-get update
  apt-get install -y \
    bats \
    build-essential \
    cargo \
    ca-certificates \
    curl \
    git \
    iptables \
    jq \
    make \
    pkg-config \
    rustc \
    tar \
    uidmap \
    util-linux
}

install_bats_library() {
  local repo="$1"
  local ref="$2"
  local destination_name="$3"
  local clone_dir="${BUILD_DIR}/${destination_name}-${ref}"
  local destination_dir="/usr/local/lib/bats/${destination_name}"

  if [ -f "${destination_dir}/load.bash" ]; then
    log "reusing ${destination_name} ${ref}"
    return
  fi

  rm -rf "${clone_dir}" "${destination_dir}"
  git clone --depth 1 --branch "${ref}" "https://github.com/bats-core/${repo}.git" "${clone_dir}"
  mkdir -p "$(dirname "${destination_dir}")" "${destination_dir}"
  cp -R "${clone_dir}/." "${destination_dir}/"
}

remove_distro_podman() {
  export DEBIAN_FRONTEND=noninteractive

  if dpkg -s podman >/dev/null 2>&1; then
    log "removing distro podman packages to avoid mixing binaries"
    apt-get remove -y podman podman-docker || true
  fi
}

install_podman_static() {
  local arch="$1"
  local url="$2"
  local tarball="${DOWNLOAD_DIR}/podman-linux-${arch}.tar.gz"
  local unpack_dir="${BUILD_DIR}/podman-linux-${arch}"
  local bundle_root="${unpack_dir}/podman-linux-${arch}"

  mkdir -p "${DOWNLOAD_DIR}" "${BUILD_DIR}"
  download_if_missing "${url}" "${tarball}"

  rm -rf "${unpack_dir}"
  mkdir -p "${unpack_dir}"
  tar -xzf "${tarball}" -C "${unpack_dir}"

  log "installing static podman bundle into /usr/local and /etc"
  cp -R "${bundle_root}/usr/." /usr/
  cp -R "${bundle_root}/etc/." /etc/

  test -x /usr/local/bin/podman
}

configure_apparmor() {
  local profile="/etc/apparmor.d/podman"

  if [ -f "${profile}" ]; then
    sed -Ei 's!^profile podman /usr/bin/podman !profile podman /usr/{bin,local/bin}/podman !' "${profile}"
    if command -v apparmor_parser >/dev/null 2>&1; then
      apparmor_parser -r "${profile}" || true
    fi
  fi
}

verify_install() {
  local user="$1"
  local uid

  uid="$(id -u "${user}")"

  PATH="/usr/local/bin:${PATH}" podman --version
  runuser -l "${user}" -c "export PATH=/usr/local/bin:\$PATH XDG_RUNTIME_DIR=/run/user/${uid}; podman info --format '{{.Host.OCIRuntime.Name}}'"
}

write_summary() {
  local summary_file="/etc/motd.d/performance-extensions-podman"

  mkdir -p "$(dirname "${summary_file}")"
  cat > "${summary_file}" <<EOF
Performance Extensions test VM
- repo mount: ${REPO_MOUNT}
- podman: $(PATH="/usr/local/bin:${PATH}" podman --version)
- run tests as: cd ${REPO_MOUNT} && cargo build --release && bats test
EOF
}

main() {
  local arch
  local static_url

  require_root
  require_cmd curl
  require_cmd tar

  arch="$(detect_arch)"
  static_url="$(podman_static_url "${arch}")"

  log "installing guest dependencies from Ubuntu packages"
  install_packages
  remove_distro_podman

  log "installing bats support libraries"
  install_bats_library bats-support "${BATS_SUPPORT_REF}" "bats-support"
  install_bats_library bats-assert "${BATS_ASSERT_REF}" "bats-assert"

  log "installing podman static bundle for ${arch}"
  install_podman_static "${arch}" "${static_url}"
  configure_apparmor

  log "configuring rootless podman for ${VM_USER}"
  configure_rootless_user "${VM_USER}"

  log "verifying podman for root and ${VM_USER}"
  verify_install "${VM_USER}"

  write_summary
  log "provisioning complete"
}

main "$@"
