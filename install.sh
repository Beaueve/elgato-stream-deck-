#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "${BASH_SOURCE[0]%/*}" && pwd)"
INSTALL_BIN="/usr/local/bin/streamdeck_ctrl"
CONFIG_DIR="${HOME}/.config/streamdeck_ctrl"
CONFIG_FILE="${CONFIG_DIR}/stream-deck.json"
SERVICE_SOURCE="${REPO_ROOT}/packaging/systemd/streamdeck_ctrl.service"
SERVICE_TARGET="${HOME}/.config/systemd/user/streamdeck_ctrl.service"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found. Please install Rust (https://rustup.rs)" >&2
  exit 1
fi

if ! command -v systemctl >/dev/null 2>&1; then
  echo "error: systemctl not found. This installer targets systemd-based systems." >&2
  exit 1
fi

SYSTEMD_USER_DIR="${HOME}/.config/systemd/user"
mkdir -p "${SYSTEMD_USER_DIR}"

cargo install --path "${REPO_ROOT}" --locked
sudo install -Dm755 "${HOME}/.cargo/bin/streamdeck_ctrl" "${INSTALL_BIN}"

mkdir -p "${CONFIG_DIR}"
if [ ! -f "${CONFIG_FILE}" ]; then
  cat <<'JSON' > "${CONFIG_FILE}"
{
  "button_index": 0,
  "outputs": [
    {
      "description": "HDMI/DisplayPort - HDA NVidia",
      "icon": { "material": "monitor" }
    },
    {
      "description": "Digital Output - A50",
      "icon": { "material": "headphones" }
    }
  ]
}
JSON
fi

ASSETS_SOURCE_DIR="${REPO_ROOT}/assets/icons/material"
ASSETS_DEST_DIR="${CONFIG_DIR}/assets"
mkdir -p "${ASSETS_DEST_DIR}"
for icon in monitor.svg headphones.svg; do
  if [ ! -f "${ASSETS_DEST_DIR}/${icon}" ]; then
    install -Dm644 "${ASSETS_SOURCE_DIR}/${icon}" "${ASSETS_DEST_DIR}/${icon}"
  fi
done

install -Dm644 "${SERVICE_SOURCE}" "${SERVICE_TARGET}"

systemctl --user daemon-reload
systemctl --user enable --now streamdeck_ctrl.service

cat <<'NOTE'
streamdeck_ctrl has been installed and started for the current user.
- Binary: /usr/local/bin/streamdeck_ctrl
- Config: ~/.config/streamdeck_ctrl/stream-deck.json
- Service: ~/.config/systemd/user/streamdeck_ctrl.service

Update the config to match your sinks (`pactl list sinks short`), then restart the service with:
  systemctl --user restart streamdeck_ctrl.service
NOTE
