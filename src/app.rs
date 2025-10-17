use std::time::Duration;

use anyhow::Result;
use crossbeam_channel::Receiver;
use tracing::{info, warn};

use crate::controls::{
    AudioToggleConfig, AudioToggleController, BrightnessController, EncoderController, Tickable,
    TimerController, VolumeController,
};
use crate::hardware::{
    EncoderId, HardwareConfig, HardwareEvent, HardwareHandle, start as start_hardware,
};
use crate::system::audio::PulseAudioBackend;
use crate::system::audio_switch::PulseAudioSwitch;
use crate::system::brightness::DdcutilBackend;

pub struct App {
    volume: VolumeController<PulseAudioBackend, HardwareHandle>,
    brightness: BrightnessController<DdcutilBackend, HardwareHandle>,
    timer: TimerController<HardwareHandle>,
    audio_toggle: Option<AudioToggleController<PulseAudioSwitch, HardwareHandle>>,
    hardware: HardwareHandle,
    shutdown: Option<Receiver<()>>,
    events: Receiver<HardwareEvent>,
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub volume_step_percent: i32,
    pub brightness_step_percent: u8,
    pub brightness_min: u8,
    pub brightness_max: u8,
    pub brightness_night: u8,
    pub timer_step_secs: u64,
    pub timer_min_secs: u64,
    pub timer_max_secs: u64,
    pub timer_default_secs: u64,
    pub pulse_sink: Option<String>,
    pub monitor_display: Option<String>,
    pub monitor_bus: Option<u8>,
    pub hardware: HardwareConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            volume_step_percent: 3,
            brightness_step_percent: 5,
            brightness_min: 10,
            brightness_max: 100,
            brightness_night: 15,
            timer_step_secs: 30,
            timer_min_secs: 30,
            timer_max_secs: 60 * 60,
            timer_default_secs: 25 * 60,
            pulse_sink: None,
            monitor_display: None,
            monitor_bus: None,
            hardware: HardwareConfig::default(),
        }
    }
}

impl App {
    pub fn new(config: AppConfig) -> Result<Self> {
        info!("starting hardware backend");
        let (hardware_handle, events) = start_hardware(config.hardware.clone())?;

        let pulse_audio = config
            .pulse_sink
            .as_ref()
            .map(|sink| PulseAudioBackend::new(sink.clone()))
            .unwrap_or_default();
        if !pulse_audio.is_available() {
            warn!("PulseAudio CLI (`pactl`) not found; volume control disabled");
        }

        let ddc_backend = DdcutilBackend::new(config.monitor_display.clone(), config.monitor_bus);
        if !ddc_backend.is_available() {
            warn!("ddcutil not found or failed; brightness control disabled");
        }

        let volume = VolumeController::new(
            pulse_audio,
            hardware_handle.clone(),
            EncoderId::One,
            config.volume_step_percent,
        )?;

        let brightness = BrightnessController::new(
            ddc_backend,
            hardware_handle.clone(),
            EncoderId::Two,
            config.brightness_step_percent,
            config.brightness_min,
            config.brightness_max,
            config.brightness_night,
        )?;

        let timer = TimerController::new(
            hardware_handle.clone(),
            EncoderId::Three,
            config.timer_step_secs,
            config.timer_min_secs,
            config.timer_max_secs,
            config.timer_default_secs,
        )?;

        let audio_toggle = match AudioToggleConfig::load_default() {
            Ok(Some(toggle_config)) => match AudioToggleController::with_default_backend(
                toggle_config,
                hardware_handle.clone(),
            ) {
                Ok(controller) => Some(controller),
                Err(err) => {
                    warn!(error = %err, "failed to initialise audio output toggle");
                    None
                }
            },
            Ok(None) => None,
            Err(err) => {
                warn!(error = %err, "failed to load audio toggle configuration");
                None
            }
        };

        Ok(Self {
            volume,
            brightness,
            timer,
            audio_toggle,
            hardware: hardware_handle,
            shutdown: None,
            events,
        })
    }

    pub fn run(&mut self) -> Result<()> {
        let ticker = crossbeam_channel::tick(Duration::from_secs(1));
        let shutdown_rx = self.shutdown.clone();
        let result = (|| -> Result<()> {
            loop {
                if let Some(ref shutdown) = shutdown_rx {
                    crossbeam_channel::select! {
                        recv(self.events) -> event => match event {
                            Ok(event) => self.handle_event(event)?,
                            Err(_) => {
                                warn!("hardware event channel closed");
                                break Ok(());
                            }
                        },
                        recv(ticker) -> _ => {
                            if let Err(err) = self.timer.on_tick() {
                                warn!(error = %err, "timer tick failed");
                            }
                            if let Err(err) = self.brightness.on_tick() {
                                warn!(error = %err, "brightness tick failed");
                            }
                        },
                        recv(shutdown) -> _ => {
                            break Ok(());
                        }
                    }
                } else {
                    crossbeam_channel::select! {
                        recv(self.events) -> event => match event {
                            Ok(event) => self.handle_event(event)?,
                            Err(_) => {
                                warn!("hardware event channel closed");
                                break Ok(());
                            }
                        },
                        recv(ticker) -> _ => {
                            if let Err(err) = self.timer.on_tick() {
                                warn!(error = %err, "timer tick failed");
                            }
                            if let Err(err) = self.brightness.on_tick() {
                                warn!(error = %err, "brightness tick failed");
                            }
                        }
                    }
                }
            }
        })();

        if let Err(err) = self.hardware.clear_all_displays() {
            warn!(error = %err, "failed to clear stream deck displays");
        }

        result
    }

    fn handle_event(&mut self, event: HardwareEvent) -> Result<()> {
        match event {
            HardwareEvent::EncoderTurned { encoder, delta } => self.handle_turn(encoder, delta),
            HardwareEvent::EncoderPressed { encoder } => self.handle_press(encoder),
            HardwareEvent::EncoderReleased { encoder } => self.handle_release(encoder),
            HardwareEvent::ButtonPressed(index) => self.handle_button_press(index),
            HardwareEvent::ButtonReleased(_) => Ok(()),
            HardwareEvent::Touch => Ok(()),
        }
    }

    fn handle_turn(&mut self, encoder: EncoderId, delta: i32) -> Result<()> {
        match encoder {
            EncoderId::One => self.volume.on_turn(delta),
            EncoderId::Two => self.brightness.on_turn(delta),
            EncoderId::Three => self.timer.on_turn(delta),
            EncoderId::Four => {
                info!("encoder 4 not assigned");
                Ok(())
            }
        }
    }

    fn handle_press(&mut self, encoder: EncoderId) -> Result<()> {
        match encoder {
            EncoderId::One => self.volume.on_press(),
            EncoderId::Two => self.brightness.on_press(),
            EncoderId::Three => self.timer.on_press(),
            EncoderId::Four => Ok(()),
        }
    }

    fn handle_release(&mut self, encoder: EncoderId) -> Result<()> {
        match encoder {
            EncoderId::One => self.volume.on_release(),
            EncoderId::Two => self.brightness.on_release(),
            EncoderId::Three => self.timer.on_release(),
            EncoderId::Four => Ok(()),
        }
    }

    fn handle_button_press(&mut self, index: u8) -> Result<()> {
        let mut handled = false;
        if let Some(toggle) = self.audio_toggle.as_mut() {
            if index == toggle.button_index() {
                toggle.on_button_pressed(index)?;
                self.volume.sync()?;
                handled = true;
            }
        }

        if !handled {
            info!(index, "button pressed (unused)");
        }

        Ok(())
    }

    pub fn set_shutdown_channel(&mut self, shutdown: Receiver<()>) {
        self.shutdown = Some(shutdown);
    }

    pub fn hardware_handle(&self) -> HardwareHandle {
        self.hardware.clone()
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Err(err) = self.hardware.clear_all_displays() {
            warn!(error = %err, "failed to clear stream deck displays on drop");
        }
    }
}
