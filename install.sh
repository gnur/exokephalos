#!/usr/bin/env bash

set -euo pipefail

REPO="${XO_REPO:-gnur/exokephalos}"
INSTALL_DIR="${XO_INSTALL_DIR:-${HOME}/.local/bin}"
CONFIG_DIR="${HOME}/.config/xo"
SYNCD_CONFIG_DIR="${HOME}/.config/xo-syncd"
CLIENT_STATE_DIR="${HOME}/.local/share/xo"
SYNC_STATE_DIR="${XO_SYNCD_STATE_DIR:-${HOME}/.local/share/xo-syncd}"
SYSTEMD_USER_DIR="${HOME}/.config/systemd/user"
SYNCD_WAS_RUNNING=false
SYNCD_SERVICE_RESTORED=false

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
  for cmd in curl tar install; do
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

syncd_service_is_running() {
  command -v systemctl >/dev/null 2>&1 \
    && systemctl --user is-active --quiet xo-syncd.service
}

restore_running_syncd() {
  if [[ "${SYNCD_WAS_RUNNING}" == true && "${SYNCD_SERVICE_RESTORED}" != true ]]; then
    log "Restarting the previously running xo-syncd.service..."
    systemctl --user daemon-reload || warn "systemctl --user daemon-reload failed"
    if systemctl --user start xo-syncd.service; then
      SYNCD_SERVICE_RESTORED=true
    else
      warn "Could not restart xo-syncd.service"
    fi
  fi
}

stop_running_syncd() {
  if syncd_service_is_running; then
    SYNCD_WAS_RUNNING=true
    log "Stopping the running xo-syncd.service before replacing its binary..."
    systemctl --user stop xo-syncd.service \
      || fatal "Could not stop the running xo-syncd.service"
    trap restore_running_syncd EXIT
  fi
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
  for binary in xo xo-lsp xo-syncd; do
    if [[ -f "${tmp_dir}/${binary}" ]]; then
      install -m 0755 "${tmp_dir}/${binary}" "${INSTALL_DIR}/.${binary}.new"
      mv -f "${INSTALL_DIR}/.${binary}.new" "${INSTALL_DIR}/${binary}"
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

ask_installation_mode() {
  echo "" >&2
  echo "How would you like to configure xo on this system?" >&2
  echo "  1) xo CLI/TUI only (desktop client)" >&2
  echo "  2) xo-syncd background daemon as a systemd user unit" >&2
  echo "  3) Both xo TUI and xo-syncd systemd user unit" >&2
  echo "  4) Skip configuration (binaries installed only)" >&2
  echo "" >&2

  local choice
  choice="$(prompt_choice "Select option (1-4)" "1")"
  case "${choice}" in
    1) echo "xo" ;;
    2) echo "syncd" ;;
    3) echo "both" ;;
    *) echo "none" ;;
  esac
}

ensure_config() {
  mkdir -p "${CONFIG_DIR}" "${CLIENT_STATE_DIR}"
  local config_file="${CONFIG_DIR}/config.scm"

  if [[ ! -f "${config_file}" ]]; then
    log "Creating initial workspace configuration at ${config_file}..."
    "${INSTALL_DIR}/xo" config-init > "${config_file}"
  fi
}

ask_syncd_configuration_mode() {
  local config_file="${SYNCD_CONFIG_DIR}/config.scm"
  if [[ ! -f "${config_file}" ]]; then
    echo "fresh"
    return
  fi

  echo "" >&2
  echo "Existing xo-syncd configuration found at ${config_file}." >&2
  echo "  1) Upgrade in place (keep the current configuration and workspace state)" >&2
  echo "  2) Start from scratch (back up and replace the configuration and state)" >&2
  echo "  3) Choose which xo components to configure" >&2
  echo "" >&2

  local choice
  choice="$(prompt_choice "Select option (1-3)" "1")"
  case "${choice}" in
    2) echo "fresh" ;;
    3) echo "choose" ;;
    *) echo "upgrade" ;;
  esac
}

backup_syncd_installation() {
  local timestamp config_file configured_state_dir old_state_dir state_backup
  timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
  config_file="${SYNCD_CONFIG_DIR}/config.scm"
  old_state_dir="${SYNC_STATE_DIR}"

  # Prefer the state directory declared by the existing generated-style config.
  # Fall back to XO_SYNCD_STATE_DIR/the default if a hand-written expression
  # cannot be parsed safely by this shell installer.
  configured_state_dir="$(sed -n 's/^[[:space:]]*(state-dir[[:space:]]*"\([^"]*\)")[[:space:]]*$/\1/p' "${config_file}")"
  case "${configured_state_dir}" in
    "~") old_state_dir="${HOME}" ;;
    "~/"*) old_state_dir="${HOME}/${configured_state_dir#\~/}" ;;
    "") ;;
    /*) old_state_dir="${configured_state_dir}" ;;
    *)
      warn "Leaving relative configured state path untouched: ${configured_state_dir}"
      old_state_dir=""
      ;;
  esac
  case "${old_state_dir}" in
    "/" | "${HOME}" | "${CONFIG_DIR}" | "${SYNCD_CONFIG_DIR}")
      warn "Refusing to move unsafe workspace state path: ${old_state_dir}"
      old_state_dir=""
      ;;
  esac

  if [[ -f "${config_file}" ]]; then
    mv "${config_file}" "${config_file}.backup-${timestamp}"
    log "Backed up the previous configuration to ${config_file}.backup-${timestamp}"
  fi
  if [[ -n "${old_state_dir}" && -e "${old_state_dir}" ]]; then
    state_backup="${old_state_dir}.backup-${timestamp}"
    mv "${old_state_dir}" "${state_backup}"
    log "Backed up the previous workspace state to ${state_backup}"
  fi
}

setup_systemd_unit() {
  local configuration_mode="$1"

  if ! command -v systemctl >/dev/null 2>&1; then
    warn "systemctl is not available on this system; skipping systemd unit installation."
    return 0
  fi

  log "Setting up systemd user unit for xo-syncd..."
  mkdir -p "${SYSTEMD_USER_DIR}" "${SYNCD_CONFIG_DIR}"

  local syncd_config_file="${SYNCD_CONFIG_DIR}/config.scm"
  if [[ "${configuration_mode}" == "fresh" ]]; then
    local oidc_issuer oidc_audience oidc_client_id
    oidc_issuer="$(prompt_choice "Pocket ID issuer URL" "")"
    oidc_audience="$(prompt_choice "Pocket ID xo API resource" "")"
    oidc_client_id="$(prompt_choice "Pocket ID public OIDC client ID" "")"
    for value in "${oidc_issuer}" "${oidc_audience}" "${oidc_client_id}"; do
      if [[ -z "${value}" || "${value}" =~ [[:space:]\"\\] ]]; then
        fatal "OIDC settings must be non-empty and cannot contain whitespace, quotes, or backslashes"
      fi
    done

    local new_config_file="${syncd_config_file}.new"
    cat > "${new_config_file}" <<EOF
; xo-syncd server configuration; command-line flags override these values.
(xo-syncd-config
  (schema 1)
  (state-dir "${SYNC_STATE_DIR}")
  (bind "127.0.0.1:9464")
  (oidc-issuer "${oidc_issuer}")
  (oidc-audience "${oidc_audience}")
  (oidc-client-id "${oidc_client_id}"))
EOF
    chmod 0600 "${new_config_file}"
    if [[ -f "${syncd_config_file}" ]]; then
      backup_syncd_installation
    fi
    mkdir -p "${SYNC_STATE_DIR}"
    mv "${new_config_file}" "${syncd_config_file}"
    log "xo-syncd configuration written to ${syncd_config_file}"
  else
    log "Keeping existing xo-syncd configuration at ${syncd_config_file}"
  fi

  local unit_file="${SYSTEMD_USER_DIR}/xo-syncd.service"
  cat > "${unit_file}" <<EOF
[Unit]
Description=xo persistent synchronization daemon
Documentation=https://github.com/${REPO}
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=${INSTALL_DIR}/xo-syncd --config ${syncd_config_file}
Restart=on-failure
RestartSec=5s
Environment=RUST_BACKTRACE=1

[Install]
WantedBy=default.target
EOF

  log "Systemd user unit written to ${unit_file}"
}

main() {
  check_dependencies
  stop_running_syncd

  local target_triple release_info tag archive_url asset_name
  target_triple="$(detect_target)"
  log "Detected target platform: ${target_triple}"

  release_info="$(resolve_release "${target_triple}")"
  IFS='|' read -r tag archive_url asset_name <<< "${release_info}"

  log "Found latest release ${tag}"
  download_and_extract "${archive_url}" "${asset_name}"

  ensure_config

  local mode syncd_configuration_mode=""
  if [[ -f "${SYNCD_CONFIG_DIR}/config.scm" ]] \
    && command -v systemctl >/dev/null 2>&1; then
    syncd_configuration_mode="$(ask_syncd_configuration_mode)"
    if [[ "${syncd_configuration_mode}" == "choose" ]]; then
      syncd_configuration_mode=""
      mode="$(ask_installation_mode)"
    else
      mode="syncd"
    fi
  else
    mode="$(ask_installation_mode)"
  fi

  case "${mode}" in
    syncd | both)
      if command -v systemctl >/dev/null 2>&1; then
        if [[ -z "${syncd_configuration_mode}" ]]; then
          syncd_configuration_mode="$(ask_syncd_configuration_mode)"
        fi
        setup_systemd_unit "${syncd_configuration_mode}"
        systemctl --user daemon-reload || warn "systemctl --user daemon-reload failed"
        if [[ "${SYNCD_WAS_RUNNING}" == true ]]; then
          restore_running_syncd
        else
          local enable_now
          enable_now="$(prompt_choice "Enable and start xo-syncd.service now? (y/n)" "y")"
          case "${enable_now}" in
            y | Y | yes | Yes)
              systemctl --user enable --now xo-syncd.service \
                || warn "Could not enable/start xo-syncd.service"
              log "xo-syncd.service enabled and started."
              ;;
            *)
              log "xo-syncd.service registered. Enable later with: systemctl --user enable --now xo-syncd"
              ;;
          esac
        fi
      else
        warn "systemctl is not available on this system; skipping systemd unit installation."
      fi
      ;;
  esac

  restore_running_syncd
  trap - EXIT

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
