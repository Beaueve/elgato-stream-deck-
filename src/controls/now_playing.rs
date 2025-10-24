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
    marquee: Option<Marquee>,
}

impl<B, D> NowPlayingController<B, D>
where
    B: NowPlayingBackend,
    D: DisplayPipeline,
{
    const MAX_TITLE_CHARS: usize = 22;

    pub fn new(backend: B, display: D, encoder: EncoderId) -> Result<Self> {
        let mut controller = Self {
            backend,
            display,
            encoder,
            last_state: None,
            marquee: None,
        };
        controller
            .refresh_display(false)
            .context("initial now-playing refresh failed")?;
        Ok(controller)
    }

    fn refresh_display(&mut self, advance_scroll: bool) -> Result<()> {
        let state = self.backend.now_playing()?;
        let state_changed = self.last_state.as_ref() != Some(&state);
        if state_changed {
            self.marquee = Marquee::from_state(&state, Self::MAX_TITLE_CHARS);
            self.last_state = Some(state.clone());
        }

        if !state_changed && self.marquee.is_none() {
            return Ok(());
        }

        self.push_display(&state, advance_scroll && !state_changed)
    }

    pub fn on_turn(&mut self, delta: i32) -> Result<()> {
        if delta > 0 {
            self.backend.next()?;
        } else if delta < 0 {
            self.backend.previous()?;
        }
        self.refresh_display(false)
    }

    fn push_display(&mut self, state: &PlaybackState, advance_marquee: bool) -> Result<()> {
        let base_value = match state.status {
            PlaybackStatus::Playing | PlaybackStatus::Paused => state
                .title
                .as_deref()
                .filter(|title| !title.is_empty())
                .unwrap_or("No title")
                .to_string(),
            PlaybackStatus::Stopped => "Not playing".to_string(),
            PlaybackStatus::Unavailable => "playerctl missing".to_string(),
        };

        let value = if let Some(marquee) = self.marquee.as_mut() {
            marquee.render(advance_marquee)
        } else {
            ellipsize(&base_value, Self::MAX_TITLE_CHARS)
        };

        let mut display = EncoderDisplay::new("spotify", value);

        let mut status_line = match state.status {
            PlaybackStatus::Playing => None,
            PlaybackStatus::Paused => Some("paused".to_string()),
            PlaybackStatus::Stopped => Some("stopped".to_string()),
            PlaybackStatus::Unavailable => None,
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
        self.refresh_display(true)
    }
}

#[derive(Debug, Clone)]
struct Marquee {
    chars: Vec<char>,
    window: usize,
    offset: usize,
}

impl Marquee {
    const GAP: usize = 6;
    const STEP: usize = 2;

    fn from_state(state: &PlaybackState, window: usize) -> Option<Self> {
        match state.status {
            PlaybackStatus::Playing | PlaybackStatus::Paused => {
                let title = state
                    .title
                    .as_deref()
                    .filter(|title| !title.is_empty())
                    .unwrap_or("No title")
                    .to_string();
                Self::new(title, window)
            }
            _ => None,
        }
    }

    fn new(text: String, window: usize) -> Option<Self> {
        if window == 0 {
            return None;
        }

        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() {
            return None;
        }

        let mut buffer: Vec<char> = chars.clone();
        if buffer.len() <= window {
            buffer.extend(std::iter::repeat(' ').take(Self::GAP));
            buffer.extend(chars.iter().copied());
        } else {
            buffer.extend(std::iter::repeat(' ').take(Self::GAP));
        }

        Some(Self {
            chars: buffer,
            window,
            offset: 0,
        })
    }

    fn render(&mut self, advance: bool) -> String {
        if self.chars.is_empty() || self.window == 0 {
            return String::new();
        }

        if advance {
            self.offset = (self.offset + Self::STEP) % self.chars.len();
        }

        let mut value = String::with_capacity(self.window);
        let len = self.chars.len();
        for step in 0..self.window {
            let idx = (self.offset + step) % len;
            value.push(self.chars[idx]);
        }
        value
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

        fn next(&self) -> Result<()> {
            Ok(())
        }

        fn previous(&self) -> Result<()> {
            Ok(())
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
        assert!(event.value.starts_with("Track A"));
        assert_eq!(event.status.as_deref(), Some("Artist A"));
    }

    #[test]
    fn unavailable_shows_playerctl_missing() {
        let backend = MockBackend::new(vec![PlaybackState::unavailable()]);
        let display = RecordingDisplay::new();
        let _controller =
            NowPlayingController::new(backend, display.clone(), EncoderId::Four).expect("init");

        let events = display.inner.lock().unwrap();
        assert_eq!(events.len(), 1);
        let (_, event) = &events[0];
        assert_eq!(event.value, "playerctl missing");
        assert!(event.status.is_none());
    }

    #[test]
    fn long_titles_scroll_across_display() {
        let backend = MockBackend::new(vec![PlaybackState {
            status: PlaybackStatus::Playing,
            title: Some("An Incredibly Long Song Title That Keeps Going".into()),
            artist: None,
        }]);

        let display = RecordingDisplay::new();
        let mut controller =
            NowPlayingController::new(backend, display.clone(), EncoderId::Four).expect("init");

        {
            let events = display.inner.lock().unwrap();
            assert_eq!(events.len(), 1);
            let (_, event) = &events[0];
            let max_chars = NowPlayingController::<MockBackend, RecordingDisplay>::MAX_TITLE_CHARS;
            assert_eq!(event.value.chars().count(), max_chars);
        }

        controller.on_tick().unwrap();

        {
            let events = display.inner.lock().unwrap();
            assert_eq!(events.len(), 2);
            let first = &events[0].1.value;
            let second = &events[1].1.value;
            assert_ne!(first, second);
            let max_chars = NowPlayingController::<MockBackend, RecordingDisplay>::MAX_TITLE_CHARS;
            assert_eq!(second.chars().count(), max_chars);
        }
    }

    #[test]
    fn short_titles_scroll_as_marquee() {
        let backend = MockBackend::new(vec![PlaybackState {
            status: PlaybackStatus::Playing,
            title: Some("Short Title".into()),
            artist: None,
        }]);

        let display = RecordingDisplay::new();
        let mut controller =
            NowPlayingController::new(backend, display.clone(), EncoderId::Four).expect("init");

        controller.on_tick().unwrap();
        controller.on_tick().unwrap();

        let events = display.inner.lock().unwrap();
        assert!(events.len() >= 3);
        let first = &events[0].1.value;
        let second = &events[1].1.value;
        let third = &events[2].1.value;
        assert_ne!(first, second);
        assert_ne!(second, third);
    }
}

fn ellipsize(input: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let mut chars = input.chars();
    if chars.clone().count() <= max_chars {
        return input.to_string();
    }

    if max_chars == 1 {
        return "…".to_string();
    }

    let truncated: String = chars.by_ref().take(max_chars - 1).collect();
    format!("{truncated}…")
}
