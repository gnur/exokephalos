#!/usr/bin/env bash

set -euo pipefail

REPO="${XO_REPO:-gnur/exokephalos}"
INSTALL_DIR="${XO_INSTALL_DIR:-${HOME}/.local/bin}"
CONFIG_DIR="${HOME}/.config/xo"
CLIENT_STATE_DIR="${HOME}/.local/share/xo"
SYNC_STATE_DIR="${XO_SYNCD_STATE_DIR:-${HOME}/.local/share/xo-syncd}"
WORKSPACE_ID="${XO_WORKSPACE_ID:-}"
SYNC_TICKET="${XO_SYNC_TICKET:-}"
SYSTEMD_USER_DIR="${HOME}/.config/systemd/user"

log() {
  echo "==> $*"
}

warn() {
  echo "==> WARNING: $*" >&2
}

fatal() {
  echo "==> ERROR: $*" >&2
  exit 1
}

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "${os}" in
    Linux)
      case "${arch}" in
        x86_64) echo "x86_64-unknown-linux-gnu" ;;
        aarch64 | arm64) echo "aarch64-unknown-linux-gnu" ;;
        *) fatal "Unsupported Linux architecture: ${arch}" ;;
      esac
      ;;
    Darwin)
      case "${arch}" in
        aarch64 | arm64) echo "aarch64-apple-darwin" ;;
        *) fatal "Unsupported macOS architecture: ${arch}. xo release binaries support Apple Silicon (aarch64)." ;;
      esac
      ;;
    *)
      fatal "Unsupported operating system: ${os}"
      ;;
  esac
}

check_dependencies() {
  local missing=()
  for cmd in curl tar; do
    if ! command -v "${cmd}" >/dev/null 2>&1; then
      missing+=("${cmd}")
    fi
  done

  if ((${#missing[@]} > 0)); then
    fatal "Missing required tools: ${missing[*]}"
  fi
}

resolve_release() {
  local release_url api_json tag archive_url
  release_url="https://api.github.com/repos/${REPO}/releases/latest"
  log "Fetching latest release information from GitHub (${REPO})..." >&2

  api_json="$(curl --fail --show-error --silent --location "${release_url}")" || {
    fatal "Could not reach GitHub Releases API (${release_url})."
  }

  tag="$(echo "${api_json}" | grep -m1 '"tag_name":' | cut -d'"' -f4)"
  if [[ -z "${tag}" ]]; then
    fatal "Could not determine latest release tag from GitHub API response."
  fi

  local target_triple="$1"
  local asset_name="xo-${target_triple}.tar.gz"

  archive_url="$(echo "${api_json}" | grep -F '"browser_download_url":' | grep -F "${asset_name}" | cut -d'"' -f4 | head -n1)"
  if [[ -z "${archive_url}" ]]; then
    archive_url="https://github.com/${REPO}/releases/download/${tag}/${asset_name}"
  fi

  echo "${tag}|${archive_url}|${asset_name}"
}

download_and_extract() {
  local archive_url="$1" asset_name="$2"
  local tmp_dir archive_file

  tmp_dir="$(mktemp -d)"
  archive_file="${tmp_dir}/${asset_name}"

  log "Downloading ${archive_url}..."
  curl --fail --show-error --silent --location "${archive_url}" --output "${archive_file}" || {
    rm -rf "${tmp_dir}"
    fatal "Failed to download release archive from ${archive_url}"
  }

  log "Extracting release binaries..."
  tar -xzf "${archive_file}" -C "${tmp_dir}" || {
    rm -rf "${tmp_dir}"
    fatal "Failed to extract ${asset_name}"
  }

  mkdir -p "${INSTALL_DIR}"
  for binary in xo xo-admin xo-lsp xo-syncd; do
    if [[ -f "${tmp_dir}/${binary}" ]]; then
      cp "${tmp_dir}/${binary}" "${INSTALL_DIR}/${binary}"
      chmod 0755 "${INSTALL_DIR}/${binary}"
    fi
  done

  rm -rf "${tmp_dir}"
  log "Installed binaries to ${INSTALL_DIR}"
}

prompt_choice() {
  local prompt="$1" default="$2"
  local reply=""

  if [[ -r /dev/tty ]]; then
    read -r -p "${prompt} [${default}]: " reply </dev/tty || reply=""
  fi

  if [[ -z "${reply}" ]]; then
    echo "${default}"
  else
    echo "${reply}"
  fi
}

prompt_secret() {
  local prompt="$1" reply=""
  if [[ -r /dev/tty ]]; then
    read -r -s -p "${prompt}: " reply </dev/tty || reply=""
    echo "" >/dev/tty
  fi
  echo "${reply}"
}

ask_installation_mode() {
  echo "" >&2
  echo "How would you like to configure xo on this system?" >&2
  echo "  1) xo CLI/TUI only (desktop client)" >&2
  echo "  2) xo-syncd background daemon as a systemd user unit" >&2
  echo "  3) Both xo TUI and xo-syncd systemd user unit" >&2
  echo "  4) Skip configuration (binaries installed only)" >&2
  echo "" >&2

  local choice default_choice="1"
  if [[ -n "${SYNC_TICKET}" ]]; then
    default_choice="2"
  fi
  choice="$(prompt_choice "Select option (1-4)" "${default_choice}")"
  case "${choice}" in
    1) echo "xo" ;;
    2) echo "syncd" ;;
    3) echo "both" ;;
    *) echo "none" ;;
  esac
}

prompt_sync_workspace() {
  if [[ -z "${WORKSPACE_ID}" ]]; then
    WORKSPACE_ID="$(prompt_choice "Workspace ID" "")"
  fi
  if [[ -z "${SYNC_TICKET}" ]]; then
    SYNC_TICKET="$(prompt_secret "Writable workspace ticket")"
  fi
  [[ -n "${WORKSPACE_ID}" ]] || fatal "A workspace ID is required for xo-syncd setup."
  [[ -n "${SYNC_TICKET}" ]] || fatal "A writable workspace ticket is required for xo-syncd setup."
}

import_sync_ticket() {
  log "Importing workspace ${WORKSPACE_ID} into ${SYNC_STATE_DIR}..."
  local output imported_workspace
  output="$("${INSTALL_DIR}/xo-admin" import-ticket "${SYNC_STATE_DIR}" "${SYNC_TICKET}")" || {
    fatal "Could not import the writable ticket. Check that its source peer is online and reachable."
  }
  imported_workspace="$(printf '%s\n' "${output}" | awk -F= '$1 == "workspace_id" { print $2; exit }')"
  if [[ "${imported_workspace}" != "${WORKSPACE_ID}" ]]; then
    fatal "The ticket belongs to workspace ${imported_workspace:-<unknown>}, not ${WORKSPACE_ID}."
  fi
}

ensure_config() {
  mkdir -p "${CONFIG_DIR}" "${CLIENT_STATE_DIR}"
  local config_file="${CONFIG_DIR}/config.scm"

  if [[ ! -f "${config_file}" ]]; then
    log "Creating initial workspace configuration at ${config_file}..."
    "${INSTALL_DIR}/xo" config-init > "${config_file}"
  fi
}

setup_systemd_unit() {
  if ! command -v systemctl >/dev/null 2>&1; then
    warn "systemctl is not available on this system; skipping systemd unit installation."
    return 0
  fi

  log "Setting up systemd user unit for xo-syncd..."
  mkdir -p "${SYSTEMD_USER_DIR}" "${SYNC_STATE_DIR}"

  local unit_file="${SYSTEMD_USER_DIR}/xo-syncd.service"
  cat > "${unit_file}" <<EOF
[Unit]
Description=xo persistent synchronization daemon
Documentation=https://github.com/${REPO}
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=${INSTALL_DIR}/xo-syncd --state-dir ${SYNC_STATE_DIR}
Restart=on-failure
RestartSec=5s
Environment=RUST_BACKTRACE=1

[Install]
WantedBy=default.target
EOF

  log "Systemd user unit written to ${unit_file}"

  if [[ -t 0 ]]; then
    local enable_now
    enable_now="$(prompt_choice "Enable and start xo-syncd.service now? (y/n)" "y")"
    case "${enable_now}" in
      y | Y | yes | Yes)
        systemctl --user daemon-reload || warn "systemctl --user daemon-reload failed"
        systemctl --user enable --now xo-syncd.service || warn "Could not enable/start xo-syncd.service"
        log "xo-syncd.service enabled and started."
        ;;
      *)
        systemctl --user daemon-reload || true
        log "xo-syncd.service registered. Enable later with: systemctl --user enable --now xo-syncd"
        ;;
    esac
  fi
}

main() {
  check_dependencies

  local target_triple release_info tag archive_url asset_name
  target_triple="$(detect_target)"
  log "Detected target platform: ${target_triple}"

  release_info="$(resolve_release "${target_triple}")"
  IFS='|' read -r tag archive_url asset_name <<< "${release_info}"

  log "Found latest release ${tag}"
  download_and_extract "${archive_url}" "${asset_name}"

  ensure_config

  local mode
  mode="$(ask_installation_mode)"

  if [[ -n "${SYNC_TICKET}" && "${mode}" == "xo" ]]; then
    warn "XO_SYNC_TICKET was supplied, but xo-syncd was not selected; the ticket was not imported."
  fi

  case "${mode}" in
    syncd | both)
      prompt_sync_workspace
      systemctl --user stop xo-syncd.service 2>/dev/null || true
      import_sync_ticket
      setup_systemd_unit
      ;;
  esac

  echo ""
  log "Installation complete!"
  echo ""
  if [[ ":${PATH}:" != *":${INSTALL_DIR}:"* ]]; then
    warn "${INSTALL_DIR} is not in your PATH."
    echo "Add it by adding this line to your shell profile (~/.bashrc, ~/.zshrc, etc.):"
    echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
    echo ""
  fi

  echo "Installed binaries:"
  echo "  ${INSTALL_DIR}/xo"
  echo "  ${INSTALL_DIR}/xo-admin"
  echo "  ${INSTALL_DIR}/xo-lsp"
  echo "  ${INSTALL_DIR}/xo-syncd"
  echo ""
  echo "Configuration directory: ${CONFIG_DIR}"
  echo "TUI state directory:     ${CLIENT_STATE_DIR}"
  if [[ "${mode}" == "syncd" || "${mode}" == "both" ]]; then
    echo "xo-syncd state directory: ${SYNC_STATE_DIR}"
  fi
  echo ""
}

# BASH_SOURCE is unset when the installer is streamed into `bash`. Run main in
# that case, while still allowing tests and shell sessions to source this file.
if [[ -z "${BASH_SOURCE[0]-}" || "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
