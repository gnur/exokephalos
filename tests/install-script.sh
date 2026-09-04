#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE="$(mktemp -d)"
export HOME="${FIXTURE}/home"
export SYSTEMCTL_LOG="${FIXTURE}/systemctl.log"
mkdir -p "${HOME}" "${FIXTURE}/bin"

cat > "${FIXTURE}/bin/systemctl" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${SYSTEMCTL_LOG}"
if [[ "$*" == "--user is-active --quiet xo-syncd.service" ]]; then
  exit 0
fi
EOF
chmod +x "${FIXTURE}/bin/systemctl"
export PATH="${FIXTURE}/bin:${PATH}"

# shellcheck source=../install.sh
source "${ROOT}/install.sh"
trap 'restore_running_syncd; rm -rf "${FIXTURE}"' EXIT

stop_running_syncd
[[ "${SYNCD_WAS_RUNNING}" == true ]]
grep -Fx -- '--user stop xo-syncd.service' "${SYSTEMCTL_LOG}"
restore_running_syncd
[[ "${SYNCD_SERVICE_RESTORED}" == true ]]
grep -Fx -- '--user start xo-syncd.service' "${SYSTEMCTL_LOG}"

CUSTOM_STATE_DIR="${HOME}/custom-syncd-state"
mkdir -p "${SYNCD_CONFIG_DIR}" "${CUSTOM_STATE_DIR}"
printf '(xo-syncd-config\n  (state-dir "~/custom-syncd-state")\n  (bind "127.0.0.1:9464"))\n' > "${SYNCD_CONFIG_DIR}/config.scm"
printf 'workspace state\n' > "${CUSTOM_STATE_DIR}/workspace-id"

TEST_CHOICE=1
prompt_choice() {
  echo "${TEST_CHOICE}"
}
[[ "$(ask_syncd_configuration_mode)" == upgrade ]]
setup_systemd_unit upgrade
grep -F '(state-dir "~/custom-syncd-state")' "${SYNCD_CONFIG_DIR}/config.scm"
grep -F "ExecStart=${INSTALL_DIR}/xo-syncd --config ${SYNCD_CONFIG_DIR}/config.scm" \
  "${SYSTEMD_USER_DIR}/xo-syncd.service"
TEST_CHOICE=2
[[ "$(ask_syncd_configuration_mode)" == fresh ]]
TEST_CHOICE=3
[[ "$(ask_syncd_configuration_mode)" == choose ]]
prompt_choice() {
  case "$1" in
    "Pocket ID issuer URL") echo "https://id.example.com" ;;
    "Pocket ID xo API resource") echo "https://notes.example.com" ;;
    "Pocket ID public OIDC client ID") echo "xo-test-client" ;;
    *) echo "$2" ;;
  esac
}
setup_systemd_unit fresh
[[ -f "${SYNCD_CONFIG_DIR}/config.scm" ]]
grep -F '(oidc-client-id "xo-test-client")' "${SYNCD_CONFIG_DIR}/config.scm"
[[ ! -e "${CUSTOM_STATE_DIR}" ]]
[[ -d "${SYNC_STATE_DIR}" ]]
compgen -G "${SYNCD_CONFIG_DIR}/config.scm.backup-*" >/dev/null
compgen -G "${CUSTOM_STATE_DIR}.backup-*" >/dev/null
