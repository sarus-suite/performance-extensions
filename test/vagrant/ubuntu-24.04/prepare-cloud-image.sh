#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CACHE_DIR="${ROOT_DIR}/.cache"
SEED_DIR="${CACHE_DIR}/nocloud"
SEED_IMAGE="${CACHE_DIR}/nocloud-seed.iso"
VAGRANT_INSECURE_PRIVATE_KEY="${HOME}/.vagrant.d/insecure_private_key"
VM_DISK_SIZE="${UBUNTU_CLOUD_IMAGE_SIZE:-40G}"

log() {
  printf '[prepare-cloud-image] %s\n' "$*"
}

detect_arch() {
  case "$(uname -m)" in
    arm64|aarch64)
      printf 'arm64\n'
      ;;
    x86_64|amd64)
      printf 'amd64\n'
      ;;
    *)
      echo "unsupported host architecture: $(uname -m)" >&2
      exit 1
      ;;
  esac
}

main() {
  local arch image_name image_url downloaded_image_path prepared_image_path insecure_pubkey

  arch="$(detect_arch)"
  image_name="noble-server-cloudimg-${arch}.img"
  image_url="https://cloud-images.ubuntu.com/noble/current/${image_name}"
  downloaded_image_path="${CACHE_DIR}/${image_name}"
  prepared_image_path="${CACHE_DIR}/noble-server-cloudimg-${arch}-${VM_DISK_SIZE,,}.qcow2"
  insecure_pubkey="$(ssh-keygen -y -f "${VAGRANT_INSECURE_PRIVATE_KEY}")"

  mkdir -p "${CACHE_DIR}"

  if [ -f "${downloaded_image_path}" ]; then
    log "reusing ${downloaded_image_path}"
  else
    log "downloading ${image_url}"
    curl -fL "${image_url}" -o "${downloaded_image_path}"
    log "saved ${downloaded_image_path}"
  fi

  if ! command -v qemu-img >/dev/null 2>&1; then
    echo "missing qemu-img; install QEMU on the host first" >&2
    exit 1
  fi

  if [ ! -f "${prepared_image_path}" ]; then
    log "creating resized guest image ${prepared_image_path}"
    cp "${downloaded_image_path}" "${prepared_image_path}"
  else
    log "reusing ${prepared_image_path}"
  fi

  qemu-img resize "${prepared_image_path}" "${VM_DISK_SIZE}" >/dev/null
  log "resized ${prepared_image_path} to ${VM_DISK_SIZE}"

  mkdir -p "${SEED_DIR}"
  cat > "${SEED_DIR}/user-data" <<EOF
#cloud-config
growpart:
  mode: auto
  devices: ["/"]
  ignore_growroot_disabled: false
resize_rootfs: true
users:
  - default
  - name: vagrant
    gecos: Vagrant
    groups: [adm, cdrom, dip, plugdev, sudo]
    shell: /bin/bash
    sudo: ALL=(ALL) NOPASSWD:ALL
    lock_passwd: false
    ssh_authorized_keys:
      - ${insecure_pubkey}
ssh_pwauth: false
disable_root: true
package_update: false
EOF

  cat > "${SEED_DIR}/meta-data" <<EOF
instance-id: performance-extensions-ubuntu-2404
local-hostname: performance-extensions-noble
EOF

  if command -v cloud-localds >/dev/null 2>&1; then
    log "building NoCloud seed with cloud-localds"
    cloud-localds "${SEED_IMAGE}" "${SEED_DIR}/user-data" "${SEED_DIR}/meta-data"
  elif command -v hdiutil >/dev/null 2>&1; then
    local tmp_base="${CACHE_DIR}/nocloud-seed"
    local generated_path=""
    rm -f "${tmp_base}" "${tmp_base}.cdr" "${tmp_base}.iso" "${SEED_IMAGE}"
    log "building NoCloud seed with hdiutil"
    hdiutil makehybrid \
      -o "${tmp_base}" \
      "${SEED_DIR}" \
      -iso \
      -joliet \
      -default-volume-name cidata \
      >/dev/null

    for candidate in "${tmp_base}" "${tmp_base}.cdr" "${tmp_base}.iso"; do
      if [ -f "${candidate}" ]; then
        generated_path="${candidate}"
        break
      fi
    done

    if [ -z "${generated_path}" ]; then
      echo "hdiutil did not create an output image under ${tmp_base}[.cdr|.iso]" >&2
      exit 1
    fi

    mv "${generated_path}" "${SEED_IMAGE}"
  elif command -v genisoimage >/dev/null 2>&1; then
    log "building NoCloud seed with genisoimage"
    genisoimage -output "${SEED_IMAGE}" -volid cidata -joliet -rock "${SEED_DIR}/user-data" "${SEED_DIR}/meta-data" >/dev/null
  elif command -v mkisofs >/dev/null 2>&1; then
    log "building NoCloud seed with mkisofs"
    mkisofs -output "${SEED_IMAGE}" -volid cidata -joliet -rock "${SEED_DIR}/user-data" "${SEED_DIR}/meta-data" >/dev/null
  else
    echo "missing tool to build cloud-init seed ISO; install cloud-localds, hdiutil, genisoimage, or mkisofs" >&2
    exit 1
  fi

  log "saved ${SEED_IMAGE}"
}

main "$@"
