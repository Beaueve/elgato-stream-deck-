use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use crossbeam_channel::{Receiver, Sender};
use elgato_streamdeck::info::Kind;
use elgato_streamdeck::{
    StreamDeck, StreamDeckError, StreamDeckInput, list_devices, new_hidapi, refresh_device_list,
};
use tracing::{debug, error, info, warn};

use image::RgbaImage;

use crate::hardware::render;

#[derive(Clone, Debug)]
pub struct HardwareConfig {
    pub serial: Option<String>,
    pub device_brightness: u8,
}

impl Default for HardwareConfig {
    fn default() -> Self {
        Self {
            serial: None,
            device_brightness: 40,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EncoderId {
    One,
    Two,
    Three,
    Four,
}

impl EncoderId {
    pub fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::One),
            1 => Some(Self::Two),
            2 => Some(Self::Three),
            3 => Some(Self::Four),
            _ => None,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::One => 0,
            Self::Two => 1,
            Self::Three => 2,
            Self::Four => 3,
        }
    }

    pub fn all() -> [Self; 4] {
        [Self::One, Self::Two, Self::Three, Self::Four]
    }
}

#[derive(Debug, Clone)]
pub struct EncoderDisplay {
    pub title: String,
    pub value: String,
    pub status: Option<String>,
    pub progress: Option<f32>,
    pub progress_color: Option<[u8; 3]>,
    pub value_color: Option<[u8; 3]>,
}

impl EncoderDisplay {
    pub fn new(title: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            value: value.into(),
            status: None,
            progress: None,
            progress_color: None,
            value_color: None,
        }
    }

    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    pub fn with_progress(mut self, progress: f32) -> Self {
        self.progress = Some(progress.clamp(0.0, 1.0));
        self
    }
}

#[derive(Debug, Clone)]
pub struct ButtonImage {
    pub id: String,
    pub image: Arc<RgbaImage>,
    pub tint: Option<[u8; 3]>,
}

#[derive(Debug)]
pub enum HardwareEvent {
    EncoderTurned { encoder: EncoderId, delta: i32 },
    EncoderPressed { encoder: EncoderId },
    EncoderReleased { encoder: EncoderId },
    ButtonPressed(u8),
    ButtonReleased(u8),
    Touch,
}

pub trait DisplayPipeline: Send + Sync {
    fn update_encoder(&self, encoder: EncoderId, display: EncoderDisplay) -> Result<()>;
    fn update_button_icon(&self, _index: u8, _icon: Option<ButtonImage>) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct HardwareHandle {
    command_tx: Sender<HardwareCommand>,
}

enum HardwareCommand {
    UpdateEncoderDisplay {
        encoder: EncoderId,
        display: EncoderDisplay,
    },
    UpdateButtonIcon {
        index: u8,
        icon: Option<ButtonImage>,
    },
}

impl DisplayPipeline for HardwareHandle {
    fn update_encoder(&self, encoder: EncoderId, display: EncoderDisplay) -> Result<()> {
        self.command_tx
            .send(HardwareCommand::UpdateEncoderDisplay { encoder, display })
            .map_err(|err| anyhow!("hardware command channel closed: {err}"))
    }

    fn update_button_icon(&self, index: u8, icon: Option<ButtonImage>) -> Result<()> {
        self.command_tx
            .send(HardwareCommand::UpdateButtonIcon { index, icon })
            .map_err(|err| anyhow!("hardware command channel closed: {err}"))
    }
}

pub fn start(config: HardwareConfig) -> Result<(HardwareHandle, Receiver<HardwareEvent>)> {
    let (event_tx, event_rx) = crossbeam_channel::unbounded();
    let (command_tx, command_rx) = crossbeam_channel::unbounded();

    thread::Builder::new()
        .name("streamdeck-backend".into())
        .spawn(move || {
            if let Err(err) = run_backend(config, event_tx, command_rx) {
                error!(error = %err, "hardware backend terminated");
            }
        })
        .context("failed to spawn hardware backend")?;

    Ok((HardwareHandle { command_tx }, event_rx))
}

fn run_backend(
    config: HardwareConfig,
    event_tx: Sender<HardwareEvent>,
    command_rx: Receiver<HardwareCommand>,
) -> Result<()> {
    let mut hid = new_hidapi().context("failed to initialise hidapi")?;
    refresh_device_list(&mut hid).ok();

    let devices = list_devices(&hid);
    debug!(device_count = devices.len(), "found stream deck devices");

    let selected = match select_device(&devices, &config.serial) {
        Ok(device) => device,
        Err(err) => {
            warn!(
                error = %err,
                "no Stream Deck detected; running hardware backend in headless mode"
            );
            return run_headless(event_tx, command_rx);
        }
    };
    info!(kind = ?selected.kind, serial = %selected.serial, "connecting to Stream Deck Plus");

    let mut permission_warned = false;
    let deck = loop {
        match StreamDeck::connect(&hid, selected.kind, &selected.serial) {
            Ok(deck) => break deck,
            Err(err) if is_permission_denied(&err) => {
                if !permission_warned {
                    warn!(
                        error = %err,
                        serial = %selected.serial,
                        "permission denied opening Stream Deck; check udev rules or group membership. Retrying in 2s"
                    );
                    permission_warned = true;
                }
                thread::sleep(Duration::from_secs(2));
                continue;
            }
            Err(err) => {
                warn!(
                    error = %err,
                    serial = %selected.serial,
                    "failed to connect to Stream Deck; running in headless mode"
                );
                return run_headless(event_tx, command_rx);
            }
        }
    };
    info!(serial = %selected.serial, "Stream Deck connection established");

    deck.set_brightness(config.device_brightness)
        .context("failed to set device brightness")?;

    let mut displays: [Option<EncoderDisplay>; 4] = [None, None, None, None];
    let mut button_icons = vec![None; selected.kind.key_count() as usize];
    render::flush_strip(&deck, &displays)?;

    let mut encoder_press_state = [false; 4];
    let mut button_press_state = vec![false; selected.kind.key_count() as usize];

    loop {
        // Drain command queue first to keep UI responsive
        process_commands(&deck, &mut displays, &mut button_icons, &command_rx)?;

        match deck.read_input(Some(Duration::from_millis(25))) {
            Ok(input) => handle_input(
                input,
                &mut encoder_press_state,
                &mut button_press_state,
                &event_tx,
            )?,
            Err(err) => handle_input_error(err)?,
        }
    }
}

fn process_commands(
    deck: &StreamDeck,
    displays: &mut [Option<EncoderDisplay>; 4],
    button_icons: &mut [Option<ButtonImage>],
    command_rx: &Receiver<HardwareCommand>,
) -> Result<()> {
    let mut displays_changed = false;
    let mut buttons_changed: Vec<u8> = Vec::new();
    while let Ok(command) = command_rx.try_recv() {
        match command {
            HardwareCommand::UpdateEncoderDisplay { encoder, display } => {
                displays[encoder.index()] = Some(display);
                displays_changed = true;
            }
            HardwareCommand::UpdateButtonIcon { index, icon } => {
                if let Some(slot) = button_icons.get_mut(index as usize) {
                    *slot = icon;
                    buttons_changed.push(index);
                } else {
                    warn!(index, "ignoring button icon update for out-of-range index");
                }
            }
        }
    }

    if displays_changed {
        render::flush_strip(deck, displays)?;
    }

    if !buttons_changed.is_empty() {
        render::flush_buttons(deck, button_icons, &buttons_changed)?;
    }

    Ok(())
}

fn handle_input(
    input: StreamDeckInput,
    encoder_state: &mut [bool; 4],
    button_state: &mut Vec<bool>,
    event_tx: &Sender<HardwareEvent>,
) -> Result<()> {
    match input {
        StreamDeckInput::NoData => {}
        StreamDeckInput::ButtonStateChange(states) => {
            for (index, state) in states.iter().enumerate() {
                if index >= button_state.len() {
                    warn!(
                        expected = button_state.len(),
                        actual = states.len(),
                        "expanding button state buffer to match hardware report"
                    );
                    button_state.resize(states.len(), false);
                }
                let previous = match button_state.get_mut(index) {
                    Some(slot) => slot,
                    None => continue,
                };
                if *previous != *state {
                    *previous = *state;
                    let event = if *state {
                        HardwareEvent::ButtonPressed(index as u8)
                    } else {
                        HardwareEvent::ButtonReleased(index as u8)
                    };
                    event_tx.send(event).ok();
                }
            }
        }
        StreamDeckInput::EncoderStateChange(states) => {
            for (index, state) in states.iter().enumerate().take(encoder_state.len()) {
                let previous = &mut encoder_state[index];
                if *previous != *state {
                    *previous = *state;
                    let encoder = match EncoderId::from_index(index) {
                        Some(enc) => enc,
                        None => continue,
                    };
                    let event = if *state {
                        HardwareEvent::EncoderPressed { encoder }
                    } else {
                        HardwareEvent::EncoderReleased { encoder }
                    };
                    event_tx.send(event).ok();
                }
            }
        }
        StreamDeckInput::EncoderTwist(deltas) => {
            for (index, delta) in deltas.iter().enumerate().take(encoder_state.len()) {
                if *delta == 0 {
                    continue;
                }
                if let Some(encoder) = EncoderId::from_index(index) {
                    event_tx
                        .send(HardwareEvent::EncoderTurned {
                            encoder,
                            delta: i32::from(*delta),
                        })
                        .ok();
                }
            }
        }
        other => {
            debug!("unhandled hardware input: {:?}", other);
        }
    }
    Ok(())
}

fn handle_input_error(err: StreamDeckError) -> Result<()> {
    match err {
        StreamDeckError::HidError(inner) => {
            Err(anyhow!(inner).context("hid error while reading input"))
        }
        StreamDeckError::BadData => {
            warn!("received malformed input packet from device");
            Ok(())
        }
        other => Err(anyhow!(other).context("stream deck error while reading input")),
    }
}

fn run_headless(
    event_tx: Sender<HardwareEvent>,
    command_rx: Receiver<HardwareCommand>,
) -> Result<()> {
    info!("hardware backend running without a connected Stream Deck");

    for command in command_rx.iter() {
        match command {
            HardwareCommand::UpdateEncoderDisplay { .. } => {
                // Ignore display updates while headless
            }
            HardwareCommand::UpdateButtonIcon { .. } => {
                // Ignore button icon updates while headless
            }
        }
    }

    drop(event_tx);
    Ok(())
}

fn is_permission_denied(err: &StreamDeckError) -> bool {
    match err {
        StreamDeckError::HidError(inner) => inner
            .to_string()
            .to_ascii_lowercase()
            .contains("permission denied"),
        _ => false,
    }
}

#[derive(Debug)]
struct SelectedDevice {
    kind: Kind,
    serial: String,
}

fn select_device(devices: &[(Kind, String)], serial: &Option<String>) -> Result<SelectedDevice> {
    let missing_device_msg = "no Stream Deck Plus detected. Ensure the device is connected and you have permissions to access it.";
    if !devices.iter().any(|(kind, _)| matches!(kind, Kind::Plus)) {
        return Err(anyhow!(missing_device_msg));
    }

    if let Some(serial_filter) = serial {
        let (kind, serial) = devices
            .iter()
            .find(|(kind, s)| matches!(kind, Kind::Plus) && s == serial_filter)
            .ok_or_else(|| anyhow!("no Stream Deck Plus with serial {serial_filter} was found"))?;
        return Ok(SelectedDevice {
            kind: *kind,
            serial: serial.clone(),
        });
    }

    let (kind, serial) = devices
        .iter()
        .find(|(kind, _)| matches!(kind, Kind::Plus))
        .ok_or_else(|| anyhow!(missing_device_msg))?;
    Ok(SelectedDevice {
        kind: *kind,
        serial: serial.clone(),
    })
}

impl std::fmt::Debug for HardwareHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HardwareHandle").finish_non_exhaustive()
    }
}
