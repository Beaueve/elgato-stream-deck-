use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use image::{ImageFormat, RgbaImage};
use once_cell::sync::OnceCell;

use crate::hardware::{ButtonImage, DisplayPipeline};

use super::Tickable;

const IDLE_TINT: [u8; 3] = [112, 112, 122];
const REMINDER_TINT: [u8; 3] = [105, 205, 165];
const PULSE_PERIOD_SECS: u64 = 8;

static POSTURE_ICON: OnceCell<Arc<RgbaImage>> = OnceCell::new();

pub struct PostureReminderController<H>
where
    H: DisplayPipeline,
{
    hardware: H,
    button_index: u8,
    icon: Arc<RgbaImage>,
    min_secs: u64,
    max_secs: u64,
    rng: XorShift64,
    state: ReminderState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReminderState {
    Waiting { remaining_secs: u64 },
    Due { pulse_ticks: u64 },
}

impl<H> PostureReminderController<H>
where
    H: DisplayPipeline,
{
    pub fn new(hardware: H, button_index: u8, min_secs: u64, max_secs: u64) -> Result<Self> {
        let min_secs = min_secs.max(1);
        let max_secs = max_secs.max(min_secs);
        let mut controller = Self {
            hardware,
            button_index,
            icon: load_posture_icon()?,
            min_secs,
            max_secs,
            rng: XorShift64::seeded(),
            state: ReminderState::Waiting {
                remaining_secs: min_secs,
            },
        };
        controller.reset_schedule()?;
        Ok(controller)
    }

    pub fn on_button_pressed(&mut self, index: u8) -> Result<bool> {
        if index != self.button_index {
            return Ok(false);
        }

        if matches!(self.state, ReminderState::Due { .. }) {
            self.reset_schedule()?;
        }

        Ok(true)
    }

    fn reset_schedule(&mut self) -> Result<()> {
        let delay = self.next_delay_secs();
        self.state = ReminderState::Waiting {
            remaining_secs: delay,
        };
        self.push_icon(self.idle_icon())
    }

    fn next_delay_secs(&mut self) -> u64 {
        self.rng.next_range_inclusive(self.min_secs, self.max_secs)
    }

    fn push_icon(&self, icon: ButtonImage) -> Result<()> {
        self.hardware
            .update_button_icon(self.button_index, Some(icon))
    }

    fn idle_icon(&self) -> ButtonImage {
        self.button_icon("posture-reminder-idle", IDLE_TINT)
    }

    fn due_icon(&self, pulse_ticks: u64) -> ButtonImage {
        self.button_icon("posture-reminder-due", pulse_tint(pulse_ticks))
    }

    fn button_icon(&self, id: &str, tint: [u8; 3]) -> ButtonImage {
        ButtonImage {
            id: format!("{id}-button-{}", self.button_index),
            image: Arc::clone(&self.icon),
            tint: Some(tint),
        }
    }

    #[cfg(test)]
    fn current_state(&self) -> ReminderState {
        self.state
    }
}

impl<H> Tickable for PostureReminderController<H>
where
    H: DisplayPipeline,
{
    fn on_tick(&mut self) -> Result<()> {
        match self.state {
            ReminderState::Waiting { remaining_secs } => {
                if remaining_secs > 1 {
                    self.state = ReminderState::Waiting {
                        remaining_secs: remaining_secs - 1,
                    };
                    Ok(())
                } else {
                    self.state = ReminderState::Due { pulse_ticks: 0 };
                    self.push_icon(self.due_icon(0))
                }
            }
            ReminderState::Due { pulse_ticks } => {
                let next_tick = pulse_ticks.saturating_add(1);
                self.state = ReminderState::Due {
                    pulse_ticks: next_tick,
                };
                self.push_icon(self.due_icon(next_tick))
            }
        }
    }
}

fn load_posture_icon() -> Result<Arc<RgbaImage>> {
    POSTURE_ICON
        .get_or_try_init(|| {
            let image = image::load_from_memory_with_format(
                include_bytes!("../../assets/icons/icons8/icons8-haltung-100.png"),
                ImageFormat::Png,
            )
            .context("failed to decode embedded posture reminder icon")?;
            Ok(Arc::new(image.to_rgba8()))
        })
        .map(Arc::clone)
}

fn pulse_tint(pulse_ticks: u64) -> [u8; 3] {
    // Smoothly blend from the muted state toward the reminder colour so the prompt
    // stays noticeable without becoming visually noisy.
    let phase = pulse_ticks as f32 * std::f32::consts::TAU / PULSE_PERIOD_SECS as f32;
    let factor = ((phase.sin() + 1.0) * 0.5).clamp(0.0, 1.0);
    blend_tint(IDLE_TINT, REMINDER_TINT, factor)
}

fn blend_tint(start: [u8; 3], end: [u8; 3], factor: f32) -> [u8; 3] {
    let factor = factor.clamp(0.0, 1.0);
    let mut tint = [0; 3];
    for channel in 0..3 {
        let from = start[channel] as f32;
        let to = end[channel] as f32;
        tint[channel] = (from + (to - from) * factor).round().clamp(0.0, 255.0) as u8;
    }
    tint
}

#[derive(Debug, Clone, Copy)]
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn seeded() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let seed = nanos ^ ((std::process::id() as u64) << 32);
        Self {
            state: seed.max(1),
        }
    }

    fn next_u64(&mut self) -> u64 {
        // A tiny PRNG is sufficient here because the reminder only needs jitter,
        // not reproducible randomness or cryptographic guarantees.
        let mut value = self.state;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.state = value.max(1);
        self.state
    }

    fn next_range_inclusive(&mut self, min: u64, max: u64) -> u64 {
        if min >= max {
            return min;
        }

        let span = max - min + 1;
        min + (self.next_u64() % span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct RecordingDisplay {
        icons: Arc<Mutex<Vec<(u8, Option<ButtonImage>)>>>,
    }

    impl RecordingDisplay {
        fn last_tint(&self) -> Option<[u8; 3]> {
            self.icons
                .lock()
                .unwrap()
                .last()
                .and_then(|(_, icon)| icon.as_ref().and_then(|icon| icon.tint))
        }
    }

    impl DisplayPipeline for RecordingDisplay {
        fn update_encoder(
            &self,
            _encoder: crate::hardware::EncoderId,
            _display: crate::hardware::EncoderDisplay,
        ) -> Result<()> {
            Ok(())
        }

        fn update_button_icon(&self, index: u8, icon: Option<ButtonImage>) -> Result<()> {
            self.icons.lock().unwrap().push((index, icon));
            Ok(())
        }
    }

    #[test]
    fn shows_idle_icon_until_the_reminder_is_due() {
        let display = RecordingDisplay::default();
        let mut controller = PostureReminderController::new(display.clone(), 3, 2, 2).unwrap();

        assert_eq!(display.last_tint(), Some(IDLE_TINT));

        controller.on_tick().unwrap();
        assert_eq!(controller.current_state(), ReminderState::Waiting { remaining_secs: 1 });
        assert_eq!(display.last_tint(), Some(IDLE_TINT));

        controller.on_tick().unwrap();
        let due_tint = display.last_tint().unwrap();
        assert_eq!(controller.current_state(), ReminderState::Due { pulse_ticks: 0 });
        assert_ne!(due_tint, IDLE_TINT);
    }

    #[test]
    fn due_icon_continues_to_pulse() {
        let display = RecordingDisplay::default();
        let mut controller = PostureReminderController::new(display.clone(), 3, 1, 1).unwrap();

        controller.on_tick().unwrap();
        let first_due = display.last_tint().unwrap();
        controller.on_tick().unwrap();
        let second_due = display.last_tint().unwrap();

        assert_ne!(first_due, IDLE_TINT);
        assert_ne!(second_due, IDLE_TINT);
        assert_ne!(first_due, second_due);
    }

    #[test]
    fn acknowledgement_resets_the_schedule() {
        let display = RecordingDisplay::default();
        let mut controller = PostureReminderController::new(display.clone(), 6, 1, 1).unwrap();

        assert!(!controller.on_button_pressed(5).unwrap());

        controller.on_tick().unwrap();
        assert_eq!(controller.current_state(), ReminderState::Due { pulse_ticks: 0 });

        assert!(controller.on_button_pressed(6).unwrap());
        assert_eq!(controller.current_state(), ReminderState::Waiting { remaining_secs: 1 });
        assert_eq!(display.last_tint(), Some(IDLE_TINT));
    }
}
