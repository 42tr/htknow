#!/usr/bin/env bash
set -euo pipefail

REQUIRED_RUST_VERSION="${REQUIRED_RUST_VERSION:-1.88.0}"
DEFAULT_RUSTUP_INIT_URL="https://rsproxy.cn/rustup-init.sh"
DEFAULT_RUSTUP_DIST_SERVER="https://rsproxy.cn"
DEFAULT_RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup"
RUSTUP_INIT_URL="${RUSTUP_INIT_URL:-${DEFAULT_RUSTUP_INIT_URL}}"
export RUSTUP_DIST_SERVER="${RUSTUP_DIST_SERVER:-${DEFAULT_RUSTUP_DIST_SERVER}}"
export RUSTUP_UPDATE_ROOT="${RUSTUP_UPDATE_ROOT:-${DEFAULT_RUSTUP_UPDATE_ROOT}}"

APT_UPDATED=0
PACMAN_SYNCED=0

as_root() {
  if [ "$(id -u)" -eq 0 ]; then
    "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  else
    echo "Need root privileges to install system packages."
    exit 1
  fi
}

ensure_pkg() {
  if command -v pacman >/dev/null 2>&1; then
    if [ "${PACMAN_SYNCED}" -ne 1 ]; then
      as_root pacman -Sy --noconfirm
      PACMAN_SYNCED=1
    fi
    as_root pacman --noconfirm --needed "$@"
  elif command -v apt-get >/dev/null 2>&1; then
    if [ "${APT_UPDATED}" -ne 1 ]; then
      as_root apt-get update
      APT_UPDATED=1
    fi
    as_root apt-get install -y --no-install-recommends "$@"
  elif command -v apk >/dev/null 2>&1; then
    as_root apk add --no-cache "$@"
  elif command -v dnf >/dev/null 2>&1; then
    as_root dnf install -y "$@"
  else
    echo "Unsupported package manager. Please preinstall required tools: $*"
    exit 1
  fi
}

has_pkg_config() {
  command -v pkg-config >/dev/null 2>&1 || command -v pkgconf >/dev/null 2>&1
}

ensure_cc_toolchain() {
  if command -v cc >/dev/null 2>&1 && command -v make >/dev/null 2>&1 && has_pkg_config; then
    return
  fi
  if command -v pacman >/dev/null 2>&1; then
    ensure_pkg gcc make pkgconf
  elif command -v apt-get >/dev/null 2>&1; then
    ensure_pkg build-essential pkg-config
  elif command -v apk >/dev/null 2>&1; then
    ensure_pkg build-base pkgconf
  elif command -v dnf >/dev/null 2>&1; then
    ensure_pkg gcc gcc-c++ make pkgconf-pkg-config
  else
    echo "Unsupported package manager. Please preinstall C toolchain (cc/make/pkg-config)."
    exit 1
  fi
}

resolve_protoc_include() {
  local protoc_bin candidate proto_file

  protoc_bin="$(command -v protoc || true)"
  for candidate in \
    "${PROTOC_INCLUDE:-}" \
    /usr/include \
    /usr/local/include \
    /usr/include/x86_64-linux-gnu \
    /usr/include/aarch64-linux-gnu \
    /usr/lib/protobuf/include \
    "$(dirname "${protoc_bin}")/../include"; do
    [ -n "${candidate}" ] || continue
    if [ -f "${candidate}/google/protobuf/empty.proto" ]; then
      (
        cd "${candidate}"
        pwd
      )
      return 0
    fi
  done

  if command -v find >/dev/null 2>&1; then
    proto_file="$(find /usr /usr/local -type f -path '*/google/protobuf/empty.proto' 2>/dev/null | head -n 1 || true)"
    if [ -n "${proto_file}" ]; then
      (
        cd "$(dirname "$(dirname "$(dirname "${proto_file}")")")"
        pwd
      )
      return 0
    fi
  fi

  return 1
}

ensure_protoc() {
  if command -v protoc >/dev/null 2>&1 && resolve_protoc_include >/dev/null 2>&1; then
    return
  fi

  if command -v pacman >/dev/null 2>&1; then
    ensure_pkg protobuf
  elif command -v apt-get >/dev/null 2>&1; then
    ensure_pkg protobuf-compiler libprotobuf-dev
  elif command -v apk >/dev/null 2>&1; then
    ensure_pkg protobuf protobuf-dev
  elif command -v dnf >/dev/null 2>&1; then
    ensure_pkg protobuf-compiler protobuf-devel
  else
    echo "Unsupported package manager. Please preinstall protoc."
    exit 1
  fi

  if ! command -v protoc >/dev/null 2>&1; then
    echo "protoc not found after installation."
    exit 1
  fi

  if ! resolve_protoc_include >/dev/null 2>&1; then
    echo "protoc found but standard protobuf include files are missing."
    exit 1
  fi
}

version_ge() {
  [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n 1)" = "$2" ]
}

has_usable_system_rust() {
  local current_rust_version

  if ! command -v cargo >/dev/null 2>&1 || ! command -v rustc >/dev/null 2>&1; then
    return 1
  fi

  current_rust_version="$(rustc --version | awk '{print $2}')"
  if [ -z "${current_rust_version}" ] || ! version_ge "${current_rust_version}" "${REQUIRED_RUST_VERSION}"; then
    return 1
  fi

  cargo fmt --version >/dev/null 2>&1
}

ensure_downloader() {
  if command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1; then
    return
  fi
  ensure_pkg curl
}

install_rustup_with() {
  local init_url

  init_url="$1"
  if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -fsSL "${init_url}" \
      | sh -s -- -y --profile minimal --default-toolchain none
  else
    wget -qO- "${init_url}" \
      | sh -s -- -y --profile minimal --default-toolchain none
  fi
}

setup_rustup_toolchain() {
  ensure_downloader

  if ! command -v rustup >/dev/null 2>&1; then
    if ! install_rustup_with "${RUSTUP_INIT_URL}"; then
      if [ "${RUSTUP_INIT_URL}" = "${DEFAULT_RUSTUP_INIT_URL}" ] \
        && [ "${RUSTUP_DIST_SERVER}" = "${DEFAULT_RUSTUP_DIST_SERVER}" ] \
        && [ "${RUSTUP_UPDATE_ROOT}" = "${DEFAULT_RUSTUP_UPDATE_ROOT}" ]; then
        echo "Primary rustup mirror is unavailable, falling back to official endpoints."
        export RUSTUP_DIST_SERVER="https://static.rust-lang.org"
        export RUSTUP_UPDATE_ROOT="https://static.rust-lang.org/rustup"
        install_rustup_with "https://sh.rustup.rs"
      else
        echo "Failed to install rustup from ${RUSTUP_INIT_URL}."
        exit 1
      fi
    fi
  fi

  if [ -f "${HOME}/.cargo/env" ]; then
    . "${HOME}/.cargo/env"
  fi
  export PATH="${HOME}/.cargo/bin:${PATH}"

  rustup set auto-self-update disable || true
  rustup set profile minimal

  if ! rustup toolchain list | awk '{print $1}' | grep -Eq "^${REQUIRED_RUST_VERSION}(-|$)"; then
    rustup toolchain install "${REQUIRED_RUST_VERSION}" --profile minimal --component rustfmt --no-self-update
  fi

  rustup default "${REQUIRED_RUST_VERSION}"
  rustup component add rustfmt --toolchain "${REQUIRED_RUST_VERSION}" || true
}

ensure_cc_toolchain
ensure_protoc

if ! has_usable_system_rust; then
  setup_rustup_toolchain
fi

if ! has_usable_system_rust; then
  echo "Rust toolchain ${REQUIRED_RUST_VERSION} with rustfmt is not available after setup."
  exit 1
fi

PROTOC_BIN="$(command -v protoc || true)"
if [ -z "${PROTOC_BIN}" ]; then
  echo "protoc not found after installation."
  exit 1
fi

PROTOC_INC="$(resolve_protoc_include || true)"
if [ -z "${PROTOC_INC}" ]; then
  echo "Could not locate google/protobuf include dir for protoc."
  exit 1
fi

if [ -n "${GITHUB_ENV:-}" ]; then
  echo "PROTOC=${PROTOC_BIN}" >> "${GITHUB_ENV}"
  echo "PROTOC_INCLUDE=${PROTOC_INC}" >> "${GITHUB_ENV}"
fi
export PROTOC="${PROTOC_BIN}"
export PROTOC_INCLUDE="${PROTOC_INC}"

if [ -d "${HOME}/.cargo/bin" ] && [ -n "${GITHUB_PATH:-}" ]; then
  echo "${HOME}/.cargo/bin" >> "${GITHUB_PATH}"
fi

cargo --version
rustc --version
protoc --version
