use std::process::{Command, Output};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use once_cell::sync::Lazy;
use tracing::{debug, info, warn};

use crate::system::availability::RetryableAvailability;

const FIELD_SEPARATOR: &str = "\u{1F}";
const PLAYERCTL_BACKOFF_SECS: u64 = 10;

static PLAYERCTL_AVAILABLE: Lazy<bool> = Lazy::new(|| {
    Command::new("playerctl")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
    Unavailable,
}

impl PlaybackStatus {
    fn from_status_string(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "playing" => Some(Self::Playing),
            "paused" => Some(Self::Paused),
            "stopped" => Some(Self::Stopped),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackState {
    pub status: PlaybackStatus,
    pub title: Option<String>,
    pub artist: Option<String>,
}

impl PlaybackState {
    pub fn unavailable() -> Self {
        Self {
            status: PlaybackStatus::Unavailable,
            title: None,
            artist: None,
        }
    }

    pub fn stopped() -> Self {
        Self {
            status: PlaybackStatus::Stopped,
            title: None,
            artist: None,
        }
    }
}

pub trait NowPlayingBackend: Send {
    fn now_playing(&self) -> Result<PlaybackState>;
}

#[derive(Debug, Clone)]
pub struct PlayerctlBackend {
    player: String,
    availability: Arc<RetryableAvailability>,
}

impl PlayerctlBackend {
    pub fn new(player: impl Into<String>) -> Self {
        Self {
            player: player.into(),
            availability: Arc::new(RetryableAvailability::new(
                *PLAYERCTL_AVAILABLE,
                PLAYERCTL_BACKOFF_SECS,
            )),
        }
    }

    fn mark_unavailable(&self, reason: &str) {
        if self.availability.mark_unavailable() {
            warn!(
                player = %self.player,
                %reason,
                "playerctl backend temporarily disabled"
            );
        }
    }

    fn mark_available(&self) {
        if self.availability.mark_available() {
            info!(player = %self.player, "playerctl backend is available again");
        }
    }

    fn should_attempt(&self) -> bool {
        let (available, became_available) = self.availability.try_acquire();
        if became_available {
            info!(player = %self.player, "retrying playerctl backend");
        }
        available
    }

    fn run_metadata_query(&self) -> Result<Output> {
        if !self.should_attempt() {
            bail!("playerctl backend currently unavailable");
        }

        let command = Command::new("playerctl")
            .arg("--player")
            .arg(&self.player)
            .arg("metadata")
            .arg("--format")
            .arg(format!(
                "{{{{status}}}}{sep}{{{{xesam:title}}}}{sep}{{{{xesam:artist}}}}",
                sep = FIELD_SEPARATOR
            ))
            .output()
            .with_context(|| {
                format!(
                    "failed to execute playerctl metadata for player {}",
                    self.player
                )
            })?;

        Ok(command)
    }

    fn parse_metadata(&self, output: &str) -> Option<PlaybackState> {
        let mut parts = output.splitn(3, FIELD_SEPARATOR);
        let status_str = parts.next()?.trim();
        let title_raw = parts.next().unwrap_or_default().trim();
        let artist_raw = parts.next().unwrap_or_default().trim();

        let status = PlaybackStatus::from_status_string(status_str)?;
        let title = if title_raw.is_empty() {
            None
        } else {
            Some(title_raw.to_string())
        };
        let artist = if artist_raw.is_empty() {
            None
        } else {
            Some(artist_raw.replace(';', ", "))
        };

        Some(PlaybackState {
            status,
            title,
            artist,
        })
    }
}

impl NowPlayingBackend for PlayerctlBackend {
    fn now_playing(&self) -> Result<PlaybackState> {
        if !*PLAYERCTL_AVAILABLE {
            return Ok(PlaybackState::unavailable());
        }

        let output = match self.run_metadata_query() {
            Ok(output) => output,
            Err(err) => {
                self.mark_unavailable(&err.to_string());
                debug!(error = %err, "playerctl metadata invocation failed");
                return Ok(PlaybackState::unavailable());
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let message = format!("{stderr}{stdout}");

            if message.contains("No players found")
                || message.contains("No player could satisfy")
                || message.contains("Command 'metadata' is not valid")
            {
                self.mark_available();
                return Ok(PlaybackState::stopped());
            }

            self.mark_unavailable(message.trim());
            debug!(
                player = %self.player,
                status = ?output.status.code(),
                stderr = %stderr,
                "playerctl metadata returned error"
            );
            return Ok(PlaybackState::unavailable());
        }

        self.mark_available();
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(state) = self.parse_metadata(stdout.trim()) {
            Ok(state)
        } else {
            Ok(PlaybackState::stopped())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_status_parses_known_states() {
        assert_eq!(
            PlaybackStatus::from_status_string("Playing"),
            Some(PlaybackStatus::Playing)
        );
        assert_eq!(
            PlaybackStatus::from_status_string("paused"),
            Some(PlaybackStatus::Paused)
        );
        assert_eq!(
            PlaybackStatus::from_status_string("STOPPED"),
            Some(PlaybackStatus::Stopped)
        );
        assert_eq!(PlaybackStatus::from_status_string("unknown"), None);
    }

    #[test]
    fn parse_metadata_extracts_fields() {
        let backend = PlayerctlBackend::new("spotify");
        let state = backend
            .parse_metadata("Playing\u{1F}Song Name\u{1F}Artist Name")
            .expect("metadata parsed");

        assert_eq!(state.status, PlaybackStatus::Playing);
        assert_eq!(state.title.as_deref(), Some("Song Name"));
        assert_eq!(state.artist.as_deref(), Some("Artist Name"));
    }

    #[test]
    fn parse_metadata_handles_missing_fields() {
        let backend = PlayerctlBackend::new("spotify");
        let state = backend
            .parse_metadata("Paused\u{1F}\u{1F}")
            .expect("metadata parsed");

        assert_eq!(state.status, PlaybackStatus::Paused);
        assert!(state.title.is_none());
        assert!(state.artist.is_none());
    }
}
