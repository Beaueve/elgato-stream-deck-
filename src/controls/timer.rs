use anyhow::Result;

use crate::hardware::{DisplayPipeline, EncoderDisplay, EncoderId};
use crate::util::format_duration;

use super::{EncoderController, Tickable};

const PROGRESS_ALERT_COLOR: [u8; 3] = [64, 130, 255];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerDisplayState {
    Setting,
    Running,
    Finished,
}

pub struct TimerController<D>
where
    D: DisplayPipeline,
{
    display: D,
    encoder: EncoderId,
    configured: u64,
    remaining: u64,
    step: u64,
    min: u64,
    max: u64,
    state: TimerDisplayState,
}

impl<D> TimerController<D>
where
    D: DisplayPipeline,
{
    pub fn new(
        display: D,
        encoder: EncoderId,
        step: u64,
        min: u64,
        max: u64,
        default: u64,
    ) -> Result<Self> {
        let step = step.max(1);
        let min_bound = min.min(max);
        let mut max_bound = max.max(min_bound);
        if max_bound < step {
            max_bound = step;
        }
        let fallback = min_bound.max(step).min(max_bound);

        let mut configured = default.clamp(min_bound, max_bound);
        if configured < fallback {
            configured = fallback;
        }

        let controller = Self {
            display,
            encoder,
            configured,
            remaining: configured,
            step,
            min: min_bound,
            max: max_bound,
            state: TimerDisplayState::Setting,
        };
        controller.push_display()?;
        Ok(controller)
    }

    fn clamp_configured(&self, value: i64) -> u64 {
        value.clamp(self.min as i64, self.max as i64) as u64
    }

    fn push_display(&self) -> Result<()> {
        let value = match self.state {
            TimerDisplayState::Running => format_duration(self.remaining),
            TimerDisplayState::Finished => "00:00".to_string(),
            TimerDisplayState::Setting => format_duration(self.configured),
        };

        let mut display = EncoderDisplay::new("timer", value);
        let ratio = if self.configured > 0 {
            (self.remaining as f32 / self.configured as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };

        match self.state {
            TimerDisplayState::Setting => {
                display.progress = Some(if self.configured > 0 { 1.0 } else { 0.0 });
            }
            TimerDisplayState::Running => {
                display.progress = Some(ratio);
                if ratio <= 0.1 {
                    display.progress_color = Some(PROGRESS_ALERT_COLOR);
                }
            }
            TimerDisplayState::Finished => {
                display.progress = Some(0.0);
            }
        }

        let status = match self.state {
            TimerDisplayState::Setting => Some("set"),
            TimerDisplayState::Running => Some("run"),
            TimerDisplayState::Finished => Some("done"),
        };
        display.status = status.map(|s| s.to_string());

        self.display.update_encoder(self.encoder, display)
    }

    fn start(&mut self) -> Result<()> {
        if self.configured == 0 {
            return Ok(());
        }
        self.remaining = self.configured;
        self.state = TimerDisplayState::Running;
        self.push_display()
    }

    fn reset_to_setting(&mut self) -> Result<()> {
        self.remaining = self.configured;
        self.state = TimerDisplayState::Setting;
        self.push_display()
    }

    fn finish(&mut self) -> Result<()> {
        self.remaining = 0;
        self.state = TimerDisplayState::Finished;
        self.push_display()
    }
}

impl<D> EncoderController for TimerController<D>
where
    D: DisplayPipeline,
{
    fn on_turn(&mut self, delta: i32) -> Result<()> {
        if delta == 0 {
            return Ok(());
        }

        if matches!(self.state, TimerDisplayState::Running) {
            return Ok(()); // ignore adjustments while running
        }

        let delta_steps = (delta as i64) * self.step as i64;
        let new_value = self.configured as i64 + delta_steps;
        self.configured = self.clamp_configured(new_value);
        self.remaining = self.configured;
        self.state = TimerDisplayState::Setting;
        self.push_display()
    }

    fn on_press(&mut self) -> Result<()> {
        match self.state {
            TimerDisplayState::Setting => self.start(),
            TimerDisplayState::Running | TimerDisplayState::Finished => self.reset_to_setting(),
        }
    }

    fn on_release(&mut self) -> Result<()> {
        Ok(())
    }
}

impl<D> Tickable for TimerController<D>
where
    D: DisplayPipeline,
{
    fn on_tick(&mut self) -> Result<()> {
        if !matches!(self.state, TimerDisplayState::Running) {
            return Ok(());
        }

        if self.remaining == 0 {
            return self.finish();
        }

        self.remaining = self.remaining.saturating_sub(1);
        if self.remaining == 0 {
            self.finish()
        } else {
            self.push_display()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::DisplayPipeline;
    use anyhow::Result;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct TestDisplay {
        pub updates: Arc<Mutex<Vec<EncoderDisplay>>>,
    }

    impl DisplayPipeline for TestDisplay {
        fn update_encoder(&self, _encoder: EncoderId, display: EncoderDisplay) -> Result<()> {
            self.updates.lock().unwrap().push(display);
            Ok(())
        }
    }

    #[test]
    fn rotation_adjusts_configuration() {
        let display = TestDisplay::default();
        let mut controller = TimerController::new(
            display.clone(),
            EncoderId::Three,
            60,
            60,
            3600,
            120,
        )
        .unwrap();

        controller.on_turn(1).unwrap(); // +60s -> 03:00
        let updates = display.updates.lock().unwrap();
        let last = updates.last().unwrap();
        assert_eq!(last.value, "03:00");
        assert_eq!(last.status.as_deref(), Some("set"));
    }

    #[test]
    fn press_starts_and_counts_down() {
        let display = TestDisplay::default();
        let mut controller = TimerController::new(
            display.clone(),
            EncoderId::Three,
            10,
            10,
            600,
            100,
        )
        .unwrap();

        controller.on_press().unwrap(); // start
        controller.on_tick().unwrap(); // first second
        controller.on_tick().unwrap(); // second second

        let updates = display.updates.lock().unwrap();
        assert!(updates.iter().any(|d| d.status.as_deref() == Some("run")));

        let before = controller.remaining;
        controller.on_turn(1).unwrap(); // ignored while running
        assert_eq!(controller.remaining, before);
    }

    #[test]
    fn pressing_while_running_resets_without_restarting() {
        let display = TestDisplay::default();
        let mut controller = TimerController::new(
            display.clone(),
            EncoderId::Three,
            60,
            60,
            3600,
            120,
        )
        .unwrap();

        controller.on_press().unwrap();
        controller.on_tick().unwrap();
        assert!(controller.remaining < controller.configured);

        controller.on_press().unwrap(); // restart
        assert_eq!(controller.remaining, controller.configured);
        assert!(matches!(controller.state, TimerDisplayState::Setting));

        let updates = display.updates.lock().unwrap();
        let last = updates.last().unwrap();
        assert_eq!(last.value, "02:00");
        assert_eq!(last.status.as_deref(), Some("set"));
    }

    #[test]
    fn progress_bar_turns_blue_under_ten_percent() {
        let display = TestDisplay::default();
        let mut controller = TimerController::new(
            display.clone(),
            EncoderId::Three,
            5,
            5,
            600,
            100,
        )
        .unwrap();

        controller.on_press().unwrap();
        for _ in 0..90 {
            controller.on_tick().unwrap();
        }

        let updates = display.updates.lock().unwrap();
        let blue = updates
            .iter()
            .rev()
            .find(|d| d.progress_color.is_some())
            .expect("blue update present");

        assert_eq!(blue.status.as_deref(), Some("run"));
        assert!(blue
            .progress
            .map(|p| (p - 0.1).abs() < f32::EPSILON)
            .unwrap_or(false));
        assert_eq!(blue.progress_color, Some(PROGRESS_ALERT_COLOR));
    }

    #[test]
    fn pressing_after_finish_resets_without_restarting() {
        let display = TestDisplay::default();
        let mut controller = TimerController::new(
            display.clone(),
            EncoderId::Three,
            1,
            1,
            600,
            3,
        )
        .unwrap();

        controller.on_press().unwrap();
        for _ in 0..3 {
            controller.on_tick().unwrap();
        }
        assert!(matches!(controller.state, TimerDisplayState::Finished));

        controller.on_press().unwrap();
        assert!(matches!(controller.state, TimerDisplayState::Setting));
        assert_eq!(controller.remaining, controller.configured);

        let updates = display.updates.lock().unwrap();
        let last = updates.last().unwrap();
        assert_eq!(last.status.as_deref(), Some("set"));
    }
}
