use anyhow::Result;

use crate::hardware::{DisplayPipeline, EncoderDisplay, EncoderId};
use crate::system::audio::AudioBackend;

use super::EncoderController;

pub struct VolumeController<A, D>
where
    A: AudioBackend,
    D: DisplayPipeline,
{
    audio: A,
    display: D,
    encoder: EncoderId,
    step: i32,
    muted: bool,
    volume: f32,
    available: bool,
}

impl<A, D> VolumeController<A, D>
where
    A: AudioBackend,
    D: DisplayPipeline,
{
    pub fn new(audio: A, display: D, encoder: EncoderId, step: i32) -> Result<Self> {
        let available = audio.is_available();
        let mut controller = Self {
            audio,
            display,
            encoder,
            step: step.max(1),
            muted: false,
            volume: 0.0,
            available,
        };
        if controller.available {
            controller.refresh_state()?;
        } else {
            controller.push_unavailable_display()?;
        }
        Ok(controller)
    }

    fn refresh_state(&mut self) -> Result<()> {
        self.available = self.audio.is_available();
        if !self.available {
            return self.push_unavailable_display();
        }

        self.volume = self.audio.get_volume()?;
        self.muted = self.audio.is_muted()?;
        self.available = self.audio.is_available();
        if !self.available {
            return self.push_unavailable_display();
        }
        self.push_display()
    }

    fn push_display(&self) -> Result<()> {
        let mut display = EncoderDisplay::new("volume", format!("{:>3.0}%", self.volume));

        let progress = (self.volume / 100.0).clamp(0.0, 1.25);
        display.progress = Some(progress.min(1.0));

        if self.muted {
            display.status = Some("muted".into());
        }

        self.display.update_encoder(self.encoder, display)
    }

    fn push_unavailable_display(&self) -> Result<()> {
        let mut display = EncoderDisplay::new("volume", "N/A");
        display.status = Some("audio disabled".into());
        display.progress = Some(0.0);
        self.display.update_encoder(self.encoder, display)
    }
}

impl<A, D> EncoderController for VolumeController<A, D>
where
    A: AudioBackend,
    D: DisplayPipeline,
{
    fn on_turn(&mut self, delta: i32) -> Result<()> {
        self.available = self.audio.is_available();
        if !self.available {
            return self.push_unavailable_display();
        }

        if delta == 0 {
            return Ok(());
        }

        // Unmute on interaction if currently muted
        if self.muted {
            self.muted = self.audio.toggle_mute()?;
        }

        let change = delta * self.step;
        self.audio.adjust_volume(change)?;
        self.refresh_state()
    }

    fn on_press(&mut self) -> Result<()> {
        self.available = self.audio.is_available();
        if !self.available {
            return self.push_unavailable_display();
        }

        self.audio.toggle_mute()?;
        self.refresh_state()
    }

    fn on_release(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::DisplayPipeline;
    use crate::system::audio::tests::{MockAudioBackend, MockAudioState};
    use anyhow::Result;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct TestDisplay {
        inner: Arc<Mutex<Vec<(EncoderId, EncoderDisplay)>>>,
    }

    impl DisplayPipeline for TestDisplay {
        fn update_encoder(&self, encoder: EncoderId, display: EncoderDisplay) -> Result<()> {
            self.inner.lock().unwrap().push((encoder, display));
            Ok(())
        }
    }

    #[test]
    fn turning_encoder_updates_volume() {
        let audio_backend = MockAudioBackend {
            inner: Arc::new(Mutex::new(MockAudioState::default())),
        };
        let display = TestDisplay::default();
        let mut controller =
            VolumeController::new(audio_backend.clone(), display.clone(), EncoderId::One, 2)
                .expect("init");

        controller.on_turn(3).expect("turn");

        let events = display.inner.lock().unwrap();
        assert!(!events.is_empty());
        let (_, last) = events.last().unwrap();
        assert!(last.value.contains('%'));
    }

    #[test]
    fn pressing_toggles_mute_status() {
        let audio_backend = MockAudioBackend {
            inner: Arc::new(Mutex::new(MockAudioState {
                muted: false,
                ..Default::default()
            })),
        };
        let display = TestDisplay::default();
        let mut controller =
            VolumeController::new(audio_backend.clone(), display.clone(), EncoderId::One, 2)
                .expect("init");

        controller.on_press().expect("press");
        let events = display.inner.lock().unwrap();
        let (_, last) = events.last().unwrap();
        assert!(matches!(last.status.as_deref(), Some("muted")));
    }
}
