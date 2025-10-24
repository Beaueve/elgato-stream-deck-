use anyhow::{Context, Result};

use crate::hardware::{DisplayPipeline, EncoderDisplay, EncoderId};
use crate::system::now_playing::{NowPlayingBackend, PlaybackState, PlaybackStatus};

use super::Tickable;

pub struct NowPlayingController<B, D>
where
    B: NowPlayingBackend,
    D: DisplayPipeline,
{
    backend: B,
    display: D,
    encoder: EncoderId,
    last_state: Option<PlaybackState>,
}

impl<B, D> NowPlayingController<B, D>
where
    B: NowPlayingBackend,
    D: DisplayPipeline,
{
    pub fn new(backend: B, display: D, encoder: EncoderId) -> Result<Self> {
        let mut controller = Self {
            backend,
            display,
            encoder,
            last_state: None,
        };
        controller
            .refresh_display()
            .context("initial now-playing refresh failed")?;
        Ok(controller)
    }

    fn refresh_display(&mut self) -> Result<()> {
        let state = self.backend.now_playing()?;
        if self.last_state.as_ref() == Some(&state) {
            return Ok(());
        }

        self.push_display(&state)?;
        self.last_state = Some(state);
        Ok(())
    }

    fn push_display(&self, state: &PlaybackState) -> Result<()> {
        let mut value = match state.status {
            PlaybackStatus::Playing | PlaybackStatus::Paused => state
                .title
                .as_deref()
                .filter(|title| !title.is_empty())
                .unwrap_or("No title"),
            PlaybackStatus::Stopped => "Not playing",
            PlaybackStatus::Unavailable => "Unavailable",
        }
        .to_string();

        if value.len() > 40 {
            value.truncate(40);
        }

        let mut display = EncoderDisplay::new("spotify", value);

        let mut status_line = match state.status {
            PlaybackStatus::Playing => None,
            PlaybackStatus::Paused => Some("paused".to_string()),
            PlaybackStatus::Stopped => Some("stopped".to_string()),
            PlaybackStatus::Unavailable => Some("playerctl missing".to_string()),
        };

        if let Some(artist) = state.artist.as_deref().filter(|artist| !artist.is_empty()) {
            status_line = match status_line.take() {
                Some(prefix) => Some(format!("{prefix} · {artist}")),
                None => Some(artist.to_string()),
            };
        }

        display.status = status_line;
        self.display.update_encoder(self.encoder, display)
    }
}

impl<B, D> Tickable for NowPlayingController<B, D>
where
    B: NowPlayingBackend,
    D: DisplayPipeline,
{
    fn on_tick(&mut self) -> Result<()> {
        self.refresh_display()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::DisplayPipeline;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct RecordingDisplay {
        inner: Arc<Mutex<Vec<(EncoderId, EncoderDisplay)>>>,
    }

    impl RecordingDisplay {
        fn new() -> Self {
            Self {
                inner: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl DisplayPipeline for RecordingDisplay {
        fn update_encoder(&self, encoder: EncoderId, display: EncoderDisplay) -> Result<()> {
            self.inner.lock().unwrap().push((encoder, display));
            Ok(())
        }
    }

    struct MockBackend {
        states: Vec<PlaybackState>,
        index: usize,
    }

    impl MockBackend {
        fn new(states: Vec<PlaybackState>) -> Self {
            Self { states, index: 0 }
        }
    }

    impl NowPlayingBackend for MockBackend {
        fn now_playing(&self) -> Result<PlaybackState> {
            Ok(self
                .states
                .get(self.index)
                .cloned()
                .unwrap_or_else(PlaybackState::stopped))
        }
    }

    #[test]
    fn initial_refresh_pushes_display() {
        let backend = MockBackend::new(vec![PlaybackState {
            status: PlaybackStatus::Playing,
            title: Some("Track A".into()),
            artist: Some("Artist A".into()),
        }]);

        let display = RecordingDisplay::new();
        let _controller =
            NowPlayingController::new(backend, display.clone(), EncoderId::Four).expect("init");

        let events = display.inner.lock().unwrap();
        assert_eq!(events.len(), 1);
        let (_, event) = &events[0];
        assert_eq!(event.value, "Track A");
        assert_eq!(event.status.as_deref(), Some("Artist A"));
    }
}
