# Elgato Stream Deck +

## Running Locally

```bash
cargo run
```

## Quick Install (systemd user unit)

```bash
./install.sh
```

The script builds the project, installs `streamdeck_ctrl` to `/usr/local/bin`, seeds `~/.config/streamdeck_ctrl/stream-deck.json`, copies the Material icons into `~/.config/streamdeck_ctrl/assets/`, installs the user service, and enables it immediately.

## Manual Installation

1. **Build & install the binary**
   ```bash
   cargo install --path . --locked
   install -Dm755 ~/.cargo/bin/streamdeck_ctrl /usr/local/bin/streamdeck_ctrl
   ```

2. **Create configuration & assets**
   ```bash
   mkdir -p ~/.config/streamdeck_ctrl/assets
   cat > ~/.config/streamdeck_ctrl/stream-deck.json <<'JSON'
   {
     "button_index": 0,
     "outputs": [
       { "description": "HDMI/DisplayPort - HDA NVidia", "icon": { "material": "monitor" } },
       { "description": "Digital Output - A50", "icon": { "material": "headphones" } }
     ]
   }
   JSON
   install -Dm644 assets/icons/material/monitor.svg ~/.config/streamdeck_ctrl/assets/monitor.svg
   install -Dm644 assets/icons/material/headphones.svg ~/.config/streamdeck_ctrl/assets/headphones.svg
   ```
   Update the JSON to match your sinks (`pactl list sinks short`), or point `STREAMDECK_CTRL_CONFIG` to an alternate file.

3. **Install the systemd user unit**
   ```bash
   install -Dm644 packaging/systemd/streamdeck_ctrl.service \
     ~/.config/systemd/user/streamdeck_ctrl.service
   ```

4. **Enable the service**
   ```bash
   systemctl --user daemon-reload
   systemctl --user enable --now streamdeck_ctrl.service
   ```

For a system-wide deployment, copy the unit to `/etc/systemd/system` and run the equivalent `systemctl` commands as root.
