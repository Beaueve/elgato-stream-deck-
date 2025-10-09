use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use once_cell::sync::Lazy;
use regex::Regex;
use tracing::warn;

const DEFAULT_SINK: &str = "@DEFAULT_SINK@";
static PACTL_AVAILABLE: Lazy<bool> = Lazy::new(|| {
    Command::new("pactl")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
});
static WARNED_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

pub trait AudioBackend: Send {
    fn get_volume(&self) -> Result<f32>;
    fn adjust_volume(&self, delta_percent: i32) -> Result<f32>;
    fn is_muted(&self) -> Result<bool>;
    fn toggle_mute(&self) -> Result<bool>;
    fn is_available(&self) -> bool {
        true
    }
}

pub struct PulseAudioBackend {
    sink: String,
    available: Arc<AtomicBool>,
}

impl Clone for PulseAudioBackend {
    fn clone(&self) -> Self {
        Self {
            sink: self.sink.clone(),
            available: Arc::clone(&self.available),
        }
    }
}

impl Default for PulseAudioBackend {
    fn default() -> Self {
        Self {
            sink: DEFAULT_SINK.to_string(),
            available: Arc::new(AtomicBool::new(*PACTL_AVAILABLE)),
        }
    }
}

impl PulseAudioBackend {
    pub fn new(sink: impl Into<String>) -> Self {
        Self {
            sink: sink.into(),
            available: Arc::new(AtomicBool::new(*PACTL_AVAILABLE)),
        }
    }

    pub fn is_available(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }

    fn run_pactl(&self, args: &[String]) -> Result<String> {
        if !self.is_available() {
            bail!("pactl not available");
        }

        let output = Command::new("pactl")
            .args(args)
            .output()
            .with_context(|| format!("failed to execute pactl with args {args:?}"))?;

        if !output.status.success() {
            let message = format!(
                "pactl exited with status {}",
                output.status.code().unwrap_or(-1)
            );
            self.mark_unavailable(message.clone());
            bail!(message);
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn sink_arg(&self) -> String {
        self.sink.clone()
    }

    fn mark_unavailable(&self, reason: impl Into<String>) {
        if self.available.swap(false, Ordering::Relaxed) {
            let reason = reason.into();
            warn_backend_disabled_with_reason(&reason);
        }
    }
}

impl std::fmt::Debug for PulseAudioBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PulseAudioBackend")
            .field("sink", &self.sink)
            .field("available", &self.available.load(Ordering::Relaxed))
            .finish()
    }
}

impl AudioBackend for PulseAudioBackend {
    fn get_volume(&self) -> Result<f32> {
        if !self.is_available() {
            warn_backend_disabled();
            return Ok(0.0);
        }

        static PERCENT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d+)%").unwrap());
        let output = match self.run_pactl(&[String::from("get-sink-volume"), self.sink_arg()]) {
            Ok(output) => output,
            Err(err) => {
                warn!(error = %err, "pactl get-sink-volume failed; disabling PulseAudio backend");
                self.mark_unavailable(err.to_string());
                return Ok(0.0);
            }
        };
        let captures = match PERCENT_RE.captures_iter(&output).next() {
            Some(capture) => capture,
            None => {
                warn!("could not parse pactl volume output: {output}");
                self.mark_unavailable("unexpected pactl volume output");
                return Ok(0.0);
            }
        };
        let value = captures
            .get(1)
            .ok_or_else(|| anyhow!("missing capture group for volume"))?
            .as_str()
            .parse::<f32>()
            .context("failed to parse volume percentage")?;
        Ok(value.min(150.0))
    }

    fn adjust_volume(&self, delta_percent: i32) -> Result<f32> {
        if !self.is_available() {
            warn_backend_disabled();
            return Ok(0.0);
        }

        if delta_percent == 0 {
            return self.get_volume();
        }

        let amount = delta_percent.abs();
        let mut arg = String::new();
        if delta_percent >= 0 {
            arg.push('+');
        } else {
            arg.push('-');
        }
        arg.push_str(&amount.to_string());
        arg.push('%');

        if let Err(err) = self.run_pactl(&[String::from("set-sink-volume"), self.sink_arg(), arg]) {
            warn!(error = %err, "pactl set-sink-volume failed; disabling PulseAudio backend");
            self.mark_unavailable(err.to_string());
            return Ok(0.0);
        }

        self.get_volume()
    }

    fn is_muted(&self) -> Result<bool> {
        if !self.is_available() {
            warn_backend_disabled();
            return Ok(false);
        }

        static MUTE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"Mute:\s+(yes|no)").unwrap());
        let output = match self.run_pactl(&[String::from("get-sink-mute"), self.sink_arg()]) {
            Ok(output) => output,
            Err(err) => {
                warn!(error = %err, "pactl get-sink-mute failed; disabling PulseAudio backend");
                self.mark_unavailable(err.to_string());
                return Ok(false);
            }
        };
        let muted = match MUTE_RE.captures(&output).and_then(|capture| capture.get(1)) {
            Some(mat) => mat.as_str().eq_ignore_ascii_case("yes"),
            None => {
                warn!("could not parse pactl mute output: {output}");
                self.mark_unavailable("unexpected pactl mute output");
                return Ok(false);
            }
        };
        Ok(muted)
    }

    fn toggle_mute(&self) -> Result<bool> {
        if !self.is_available() {
            warn_backend_disabled();
            return Ok(false);
        }

        if let Err(err) = self.run_pactl(&[
            String::from("set-sink-mute"),
            self.sink_arg(),
            String::from("toggle"),
        ]) {
            warn!(error = %err, "pactl toggle mute failed; disabling PulseAudio backend");
            self.mark_unavailable(err.to_string());
            return Ok(false);
        }
        self.is_muted()
    }

    fn is_available(&self) -> bool {
        PulseAudioBackend::is_available(self)
    }
}

fn warn_backend_disabled() {
    warn_backend_disabled_with_reason("PulseAudio CLI (`pactl`) not found or returned an error");
}

fn warn_backend_disabled_with_reason(reason: &str) {
    if !WARNED_UNAVAILABLE.swap(true, Ordering::Relaxed) {
        warn!(
            "PulseAudio backend disabled ({reason}); volume encoder operates in read-only placeholder mode"
        );
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, Default)]
    pub struct MockAudioBackend {
        pub inner: Arc<Mutex<MockAudioState>>,
    }

    impl AudioBackend for MockAudioBackend {
        fn get_volume(&self) -> Result<f32> {
            Ok(self.inner.lock().unwrap().volume)
        }

        fn adjust_volume(&self, delta_percent: i32) -> Result<f32> {
            let mut state = self.inner.lock().unwrap();
            state.history.push(format!("adjust:{delta_percent}"));
            let new_volume = (state.volume + delta_percent as f32).clamp(0.0, 150.0);
            state.volume = new_volume;
            Ok(new_volume)
        }

        fn is_muted(&self) -> Result<bool> {
            Ok(self.inner.lock().unwrap().muted)
        }

        fn toggle_mute(&self) -> Result<bool> {
            let mut state = self.inner.lock().unwrap();
            state.history.push("toggle_mute".into());
            state.muted = !state.muted;
            Ok(state.muted)
        }
    }

    #[derive(Debug)]
    pub struct MockAudioState {
        pub volume: f32,
        pub muted: bool,
        pub history: Vec<String>,
    }

    impl Default for MockAudioState {
        fn default() -> Self {
            Self {
                volume: 50.0,
                muted: false,
                history: Vec::new(),
            }
        }
    }
}
