#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
ASSET_DIR="${HPC_SSHD_ASSET_DIR:-${ROOT_DIR}/crates/ssh_precreate_hook/assets}"
OPENSSH_VERSION="${OPENSSH_VERSION:-V_10_0_P1}"
OPENSSH_REPO="${OPENSSH_REPO:-https://github.com/openssh/openssh-portable.git}"
OPENSSH_SHA="${OPENSSH_SHA:-2593769fb291fe6c542173927698c69e9f9a08b9}"
GLIBC_BASELINE="${HPC_SSHD_GLIBC_BASELINE:-2.34}"
RECIPE_VERSION=1
MANIFEST="${ASSET_DIR}/hpc-sshd.build-info"
BINARIES=(sshd sshd-auth sshd-session)

log() { printf '[build-hpc-sshd] %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
require_cmd() { command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"; }

native_arch() {
  case "$(uname -m)" in
    x86_64|amd64) printf '%s\n' amd64 ;;
    aarch64|arm64) printf '%s\n' arm64 ;;
    *) die "unsupported native build architecture: $(uname -m)" ;;
  esac
}

verify_elf() {
  local binary="$1"
  local file_output interpreter needed versions highest_version newest_allowed

  [ -x "${binary}" ] || return 1
  file_output="$(file -b "${binary}")"
  printf '%s\n' "${file_output}" | grep -Eq 'ELF .*executable|ELF .*shared object' || return 1
  case "${TARGET_ARCH}" in
    amd64) printf '%s\n' "${file_output}" | grep -Eq 'x86-64|x86_64' || return 1 ;;
    arm64) printf '%s\n' "${file_output}" | grep -Eq 'ARM aarch64|aarch64' || return 1 ;;
  esac

  interpreter="$(readelf -l "${binary}" | sed -n 's/.*Requesting program interpreter: \([^]]*\).*/\1/p')"
  printf '%s\n' "${interpreter}" | grep -Eq 'ld-linux-(x86-64|aarch64)\.so' || return 1

  while IFS= read -r needed; do
    case "${needed}" in
      libc.so.6|libpthread.so.0|libdl.so.2|librt.so.1|libm.so.6|libresolv.so.2|libutil.so.1|ld-linux-aarch64.so.1|ld-linux-x86-64.so.2|'') ;;
      *) return 1 ;;
    esac
  done < <(readelf -d "${binary}" | sed -n 's/.*Shared library: \[\([^]]*\)\].*/\1/p')

  versions="$(readelf --version-info "${binary}" 2>/dev/null \
    | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' \
    | sort -Vu \
    | tail -n 1 || true)"
  [ -n "${versions}" ] || return 1
  highest_version="${versions#GLIBC_}"
  newest_allowed="$(printf '%s\n' "${GLIBC_BASELINE}" "${highest_version}" | sort -V | tail -n 1)"
  [ "${newest_allowed}" = "${GLIBC_BASELINE}" ]
}

manifest_value() {
  sed -n "s/^$1=//p" "${MANIFEST}" | head -n 1
}

assets_are_current() {
  [ -f "${MANIFEST}" ] || return 1
  [ "$(manifest_value recipe)" = "${RECIPE_HASH}" ] || return 1
  [ "$(manifest_value arch)" = "${TARGET_ARCH}" ] || return 1
  [ "$(manifest_value openssh_sha)" = "${OPENSSH_SHA}" ] || return 1
  [ "$(manifest_value glibc_baseline)" = "${GLIBC_BASELINE}" ] || return 1

  local binary expected actual
  for binary in "${BINARIES[@]}"; do
    verify_elf "${ASSET_DIR}/${binary}" || return 1
    expected="$(manifest_value "sha256_${binary//-/_}")"
    [ -n "${expected}" ] || return 1
    actual="$(sha256sum "${ASSET_DIR}/${binary}" | awk '{print $1}')"
    [ "${actual}" = "${expected}" ] || return 1
  done
}

for command in dpkg-query file gcc git make readelf sha256sum sort strip; do
  require_cmd "${command}"
done

TARGET_ARCH="${TARGET_ARCH:-$(native_arch)}"
case "${TARGET_ARCH}" in amd64|arm64) ;; *) die "unsupported TARGET_ARCH=${TARGET_ARCH}" ;; esac
[ "${TARGET_ARCH}" = "$(native_arch)" ] \
  || die "TARGET_ARCH=${TARGET_ARCH} does not match native build architecture $(native_arch)"

script_hash="$(sha256sum "${BASH_SOURCE[0]}" | awk '{print $1}')"
builder_hash="$(sha256sum "${ROOT_DIR}/.devcontainer/glibc/Dockerfile" | awk '{print $1}')"
recipe_input="${RECIPE_VERSION}|${script_hash}|${builder_hash}|${OPENSSH_REPO}|${OPENSSH_VERSION}|${OPENSSH_SHA}|${GLIBC_BASELINE}|${TARGET_ARCH}"
RECIPE_HASH="$(printf '%s' "${recipe_input}" | sha256sum | awk '{print $1}')"

if assets_are_current; then
  log "reusing validated ${TARGET_ARCH} assets in ${ASSET_DIR}"
  exit 0
fi

work_dir="$(mktemp -d)"
stage_dir="${ASSET_DIR}/.hpc-sshd-stage.$$"
cleanup() {
  rm -rf "${work_dir}" "${stage_dir}"
}
trap cleanup EXIT HUP INT TERM

openssh_src="${work_dir}/openssh"
static_lib_dir="${work_dir}/static-libs"
mkdir -p "${static_lib_dir}" "${stage_dir}"

system_lib_dir="/usr/lib/$(gcc -print-multiarch)"
for archive in libcrypto.a libcrypt.a; do
  [ -f "${system_lib_dir}/${archive}" ] || die "required static archive not found: ${system_lib_dir}/${archive}"
  cp "${system_lib_dir}/${archive}" "${static_lib_dir}/${archive}"
done

log "checking out OpenSSH ${OPENSSH_VERSION}"
git init -q "${openssh_src}"
git -C "${openssh_src}" remote add origin "${OPENSSH_REPO}"
git -C "${openssh_src}" fetch --depth 1 origin \
  "refs/tags/${OPENSSH_VERSION}:refs/tags/${OPENSSH_VERSION}" >/dev/null
git -C "${openssh_src}" checkout --detach "refs/tags/${OPENSSH_VERSION}" >/dev/null
actual_sha="$(git -C "${openssh_src}" rev-parse HEAD)"
[ "${actual_sha}" = "${OPENSSH_SHA}" ] \
  || die "OpenSSH source mismatch: expected ${OPENSSH_SHA}, got ${actual_sha}"

log "building OpenSSH for ${TARGET_ARCH} with glibc baseline ${GLIBC_BASELINE}"
(
  cd "${openssh_src}"
  touch configure
  CFLAGS='-O2 -fPIC' \
  LDFLAGS="-L${static_lib_dir}" \
  LIBS='-lcrypto -lcrypt' \
  ac_cv_func_arc4random=no \
  ac_cv_func_arc4random_buf=no \
  ac_cv_func_arc4random_uniform=no \
  ./configure \
    --without-zlib \
    --disable-security-key \
    --disable-pkcs11 \
    --without-pam \
    --without-kerberos5 \
    --without-selinux \
    --without-libedit
  make -j"$(getconf _NPROCESSORS_ONLN 2>/dev/null || printf '1\n')"
)

for binary in "${BINARIES[@]}"; do
  [ -x "${openssh_src}/${binary}" ] || die "OpenSSH did not produce ${binary}"
  install -m0755 "${openssh_src}/${binary}" "${stage_dir}/${binary}"
  strip "${stage_dir}/${binary}"
  verify_elf "${stage_dir}/${binary}" \
    || die "OpenSSH produced an incompatible ${binary}"
done

{
  printf 'recipe=%s\n' "${RECIPE_HASH}"
  printf 'arch=%s\n' "${TARGET_ARCH}"
  printf 'openssh_repo=%s\n' "${OPENSSH_REPO}"
  printf 'openssh_ref=%s\n' "${OPENSSH_VERSION}"
  printf 'openssh_sha=%s\n' "${actual_sha}"
  printf 'glibc_baseline=%s\n' "${GLIBC_BASELINE}"
  printf 'libssl_dev_version=%s\n' "$(dpkg-query -W -f='${Version}' libssl-dev)"
  printf 'libcrypt_dev_version=%s\n' "$(dpkg-query -W -f='${Version}' libcrypt-dev)"
  for binary in "${BINARIES[@]}"; do
    printf 'sha256_%s=%s\n' "${binary//-/_}" \
      "$(sha256sum "${stage_dir}/${binary}" | awk '{print $1}')"
  done
} > "${stage_dir}/hpc-sshd.build-info"

mkdir -p "${ASSET_DIR}"
for binary in "${BINARIES[@]}"; do
  mv -f "${stage_dir}/${binary}" "${ASSET_DIR}/${binary}"
done
mv -f "${stage_dir}/hpc-sshd.build-info" "${MANIFEST}"
log "published ${TARGET_ARCH} assets in ${ASSET_DIR}"
