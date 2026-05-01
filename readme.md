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

## Configuration

The config file is loaded from `STREAMDECK_CTRL_CONFIG` if set, otherwise from `~/.config/streamdeck_ctrl/stream-deck.json` (also `config/stream-deck.json` in the repo when running locally).

Icons are resolved relative to the config file directory or an `assets/` subdirectory. You can also set `STREAMDECK_CTRL_ASSETS` to point at an assets directory.

### Audio buttons (audio toggle)

Use `audio_toggle` to map one or more audio sinks to Stream Deck buttons. Each output must match a sink `id`, `name`, or `description` (see `pactl list sinks short` / `pactl list sinks`).

```json
{
  "audio_toggle": {
    "button_index": 0,
    "outputs": [
      { "description": "HDMI/DisplayPort - HDA NVidia", "icon": { "material": "monitor" } },
      { "description": "Digital Output - A50", "icon": { "material": "headphones" } }
    ]
  }
}
```

- `button_index` sets the default button used to cycle outputs.
- Each output can override `button_index` to pin it to a specific button.
- `icon` can be `{ "material": "monitor" | "headphones" }` or a file path (relative to the config or assets dir).

Other optional keys:
- `now_playing_player`: playerctl selector string (e.g. `spotify,%any`).
- `launchers`: list of `{ "button_index": 4, "desktop_file": "/path/to/app.desktop" }`.

## Posture Reminder

The first unassigned Stream Deck Plus button is reserved automatically for a posture reminder. It uses the bundled `icons8-haltung-100.png` asset, stays grey while idle, and then starts pulsing with colour after a random delay between 10 and 30 minutes. Press the button to acknowledge the reminder and start a new random interval.
