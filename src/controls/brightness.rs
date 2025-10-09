use std::thread;

use anyhow::{Result, anyhow};
use crossbeam_channel::{bounded, Receiver, TryRecvError};
use tracing::warn;

use crate::hardware::{DisplayPipeline, EncoderDisplay, EncoderId};
use crate::system::brightness::BrightnessBackend;

use super::{EncoderController, Tickable};

pub struct BrightnessController<B, D>
where
    B: BrightnessBackend,
    D: DisplayPipeline,
{
    backend: B,
    display: D,
    encoder: EncoderId,
    step: u8,
    min_level: u8,
    max_level: u8,
    level: u8,
    pending_level: u8,
    pending_dirty: bool,
    apply_inflight: Option<u8>,
    apply_rx: Option<Receiver<Result<u8>>>,
    night_level: u8,
    previous_level: u8,
    available: bool,
}

impl<B, D> BrightnessController<B, D>
where
    B: BrightnessBackend + Clone + Send + 'static,
    D: DisplayPipeline,
{
    pub fn new(
        backend: B,
        display: D,
        encoder: EncoderId,
        step: u8,
        min_level: u8,
        max_level: u8,
        night_level: u8,
    ) -> Result<Self> {
        let initial_available = backend.is_available();
        let mut controller = Self {
            backend,
            display,
            encoder,
            step: step.max(1),
            min_level,
            max_level: max_level.max(min_level + 1),
            level: min_level,
            pending_level: min_level,
            pending_dirty: false,
            apply_inflight: None,
            apply_rx: None,
            night_level: night_level.clamp(min_level, max_level),
            previous_level: max_level,
            available: initial_available,
        };
        controller.refresh_state()?;
        Ok(controller)
    }

    fn refresh_state(&mut self) -> Result<()> {
        self.available = self.backend.is_available();
        if !self.available {
            return self.push_unavailable_display();
        }

        let current = match self.backend.get_brightness() {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    error = %err,
                    "failed to query brightness; defaulting to {}%",
                    self.max_level
                );
                self.available = self.backend.is_available();
                self.max_level
            }
        };
        self.level = current.clamp(self.min_level, self.max_level);
        self.pending_level = self.level;
        self.pending_dirty = false;
        self.apply_inflight = None;
        self.apply_rx = None;
        self.previous_level = self.level;
        self.available = self.backend.is_available();
        if !self.available {
            return self.push_unavailable_display();
        }
        self.push_display()
    }

    fn push_display(&self) -> Result<()> {
        let display_level = if self.pending_dirty {
            self.pending_level
        } else {
            self.level
        };
        let mut display = EncoderDisplay::new("bright", format!("{:>3}%", display_level));
        let range = (self.max_level - self.min_level) as f32;
        let progress = if range > 0.0 {
            (display_level.saturating_sub(self.min_level) as f32 / range).clamp(0.0, 1.0)
        } else {
            0.0
        };
        display.progress = Some(progress);

        if self.pending_dirty {
            display.status = Some("pending".into());
        } else if self.apply_inflight.is_some() {
            display.status = Some("apply".into());
        } else if display_level <= self.night_level {
            display.status = Some("night".into());
        }

        self.display.update_encoder(self.encoder, display)
    }

    fn push_unavailable_display(&self) -> Result<()> {
        let mut display = EncoderDisplay::new("bright", "N/A");
        display.status = Some("ddc disabled".into());
        display.progress = Some(0.0);
        self.display.update_encoder(self.encoder, display)
    }

    fn poll_apply(&mut self) -> Result<()> {
        let mut finished = false;
        let mut outcome = None;

        if let Some(rx) = self.apply_rx.as_ref() {
            match rx.try_recv() {
                Ok(result) => {
                    finished = true;
                    outcome = Some(result);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    finished = true;
                    outcome = Some(Err(anyhow!("brightness worker disconnected")));
                }
            }
        }

        if !finished {
            return Ok(());
        }

        self.apply_rx = None;
        self.apply_inflight = None;

        match outcome.unwrap_or_else(|| Ok(self.level)) {
            Ok(applied) => {
                self.level = applied;
                self.pending_level = applied;
                if applied > self.night_level {
                    self.previous_level = applied;
                }
            }
            Err(err) => {
                warn!(error = %err, "failed to apply brightness");
            }
        }

        self.available = self.backend.is_available();
        if !self.available {
            self.push_unavailable_display()
        } else {
            self.push_display()
        }
    }

    fn enqueue_apply(&mut self, target: u8) -> Result<()> {
        self.apply_rx = None;
        let (tx, rx) = bounded(1);
        let backend = self.backend.clone();
        thread::spawn(move || {
            let result = backend.set_brightness(target);
            let _ = tx.send(result);
        });
        self.apply_rx = Some(rx);
        self.apply_inflight = Some(target);
        self.pending_dirty = false;
        self.pending_level = target;
        self.level = target;
        if target > self.night_level {
            self.previous_level = target;
        }
        self.available = self.backend.is_available();
        self.push_display()
    }

    fn preview_level(&mut self, level: i32) -> Result<()> {
        self.poll_apply()?;
        self.available = self.backend.is_available();
        if !self.available {
            return self.push_unavailable_display();
        }

        let clamped = level.clamp(self.min_level as i32, self.max_level as i32) as u8;
        self.pending_level = clamped;
        self.pending_dirty = self.pending_level != self.level;
        if self.pending_dirty {
            self.apply_inflight = None;
        }
        self.push_display()
    }

    fn set_level(&mut self, level: i32) -> Result<()> {
        self.poll_apply()?;
        self.available = self.backend.is_available();
        if !self.available {
            return self.push_unavailable_display();
        }

        let clamped = level.clamp(self.min_level as i32, self.max_level as i32) as u8;
        self.enqueue_apply(clamped)
    }
}

impl<B, D> EncoderController for BrightnessController<B, D>
where
    B: BrightnessBackend + Clone + Send + 'static,
    D: DisplayPipeline,
{
    fn on_turn(&mut self, delta: i32) -> Result<()> {
        self.poll_apply()?;
        self.available = self.backend.is_available();
        if !self.available {
            return self.push_unavailable_display();
        }

        if delta == 0 {
            return Ok(());
        }
        let magnitude = (delta.abs() as u32)
            .saturating_mul(self.step as u32)
            .min(u8::MAX as u32) as i32;
        let delta_value = if delta > 0 { magnitude } else { -magnitude };
        self.preview_level(self.pending_level as i32 + delta_value)
    }

    fn on_press(&mut self) -> Result<()> {
        self.poll_apply()?;
        self.available = self.backend.is_available();
        if !self.available {
            return self.push_unavailable_display();
        }

        if self.pending_dirty {
            return self.set_level(self.pending_level as i32);
        }

        if self.level <= self.night_level {
            let restore = self.previous_level.max(self.night_level + 1);
            self.set_level(restore as i32)
        } else {
            self.previous_level = self.level;
            self.set_level(self.night_level as i32)
        }
    }

    fn on_release(&mut self) -> Result<()> {
        self.poll_apply()?;
        Ok(())
    }
}

impl<B, D> Tickable for BrightnessController<B, D>
where
    B: BrightnessBackend + Clone + Send + 'static,
    D: DisplayPipeline,
{
    fn on_tick(&mut self) -> Result<()> {
        self.poll_apply()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::DisplayPipeline;
    use crate::system::brightness::tests::{MockBrightnessBackend, MockBrightnessState};
    use crate::controls::Tickable;
    use anyhow::Result;
    use std::thread;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Clone, Default)]
    struct TestDisplay {
        updates: Arc<Mutex<Vec<EncoderDisplay>>>,
    }

    impl DisplayPipeline for TestDisplay {
        fn update_encoder(&self, _encoder: EncoderId, display: EncoderDisplay) -> Result<()> {
            self.updates.lock().unwrap().push(display);
            Ok(())
        }
    }

    fn wait_for_apply(
        controller: &mut BrightnessController<MockBrightnessBackend, TestDisplay>,
    ) {
        for _ in 0..20 {
            controller.on_tick().unwrap();
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn rotation_applies_step_changes() {
        let backend = MockBrightnessBackend {
            inner: Arc::new(Mutex::new(MockBrightnessState {
                level: 60,
                ..Default::default()
            })),
        };
        let display = TestDisplay::default();
        let mut controller = BrightnessController::new(
            backend.clone(),
            display.clone(),
            EncoderId::Two,
            5,
            10,
            100,
            15,
        )
        .expect("init");

        controller.on_turn(-1).expect("turn");
        let updates = display.updates.lock().unwrap();
        assert!(!updates.is_empty());
    }

    #[test]
    fn rotation_defers_backend_updates_until_press() {
        let backend = MockBrightnessBackend {
            inner: Arc::new(Mutex::new(MockBrightnessState {
                level: 60,
                ..Default::default()
            })),
        };
        let display = TestDisplay::default();
        let mut controller = BrightnessController::new(
            backend.clone(),
            display.clone(),
            EncoderId::Two,
            5,
            10,
            100,
            15,
        )
        .expect("init");

        controller.on_turn(-2).expect("preview turn");
        {
            let state = backend.inner.lock().unwrap();
            assert!(state.history.is_empty());
            assert_eq!(state.level, 60);
        }

        {
            let updates = display.updates.lock().unwrap();
            let last = updates.last().expect("display updated");
            assert_eq!(last.value, " 50%");
            assert!(matches!(last.status.as_deref(), Some("pending")));
        }

        controller.on_press().expect("commit press");
        wait_for_apply(&mut controller);
        {
            let state = backend.inner.lock().unwrap();
            assert_eq!(state.history, vec![50]);
            assert_eq!(state.level, 50);
        }
    }

    #[test]
    fn press_toggles_night_mode() {
        let backend = MockBrightnessBackend::default();
        let display = TestDisplay::default();
        let mut controller = BrightnessController::new(
            backend.clone(),
            display.clone(),
            EncoderId::Two,
            5,
            10,
            100,
            15,
        )
        .expect("init");

        controller.on_press().expect("press");
        wait_for_apply(&mut controller);
        let updates = display.updates.lock().unwrap();
        let status = updates.last().unwrap().status.clone();
        assert!(matches!(status.as_deref(), Some("night")));
    }
}
