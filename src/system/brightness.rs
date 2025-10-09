use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use once_cell::sync::Lazy;
use regex::Regex;
use tracing::warn;

pub trait BrightnessBackend: Send {
    fn get_brightness(&self) -> Result<u8>;
    fn set_brightness(&self, value: u8) -> Result<u8>;
    fn is_available(&self) -> bool {
        true
    }
}

static DDCUTIL_AVAILABLE: Lazy<bool> = Lazy::new(|| {
    Command::new("ddcutil")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
});
static WARNED_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

pub struct DdcutilBackend {
    pub display: Option<String>,
    pub bus: Option<u8>,
    available: Arc<AtomicBool>,
}

impl Clone for DdcutilBackend {
    fn clone(&self) -> Self {
        Self {
            display: self.display.clone(),
            bus: self.bus,
            available: Arc::clone(&self.available),
        }
    }
}

impl Default for DdcutilBackend {
    fn default() -> Self {
        Self::new(None, None)
    }
}

impl std::fmt::Debug for DdcutilBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DdcutilBackend")
            .field("display", &self.display)
            .field("bus", &self.bus)
            .field("available", &self.available.load(Ordering::Relaxed))
            .finish()
    }
}

impl DdcutilBackend {
    pub fn new(display: Option<String>, bus: Option<u8>) -> Self {
        Self {
            display,
            bus,
            available: Arc::new(AtomicBool::new(*DDCUTIL_AVAILABLE)),
        }
    }

    pub fn is_available(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }

    fn spawn_command(&self, command: &str, value: Option<String>) -> Result<String> {
        if !self.is_available() {
            bail!("ddcutil not available");
        }

        let mut cmd = Command::new("ddcutil");
        match command {
            "getvcp" => {
                cmd.arg("getvcp").arg("10");
            }
            "setvcp" => {
                cmd.arg("setvcp").arg("10");
                if let Some(value) = value {
                    cmd.arg(value);
                }
            }
            other => return Err(anyhow!("unsupported ddcutil command: {other}")),
        }

        if let Some(display) = &self.display {
            cmd.arg("--display").arg(display);
        }
        if let Some(bus) = self.bus {
            cmd.arg("--bus").arg(bus.to_string());
        }

        let output = cmd
            .output()
            .with_context(|| format!("failed to execute {cmd:?}"))?;

        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
            self.mark_unavailable(format!("ddcutil exited with {code}"));
            bail!("ddcutil exited with {code}");
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn mark_unavailable(&self, reason: impl Into<String>) {
        if self.available.swap(false, Ordering::Relaxed) {
            let reason = reason.into();
            warn_backend_disabled(&reason);
        }
    }
}

impl BrightnessBackend for DdcutilBackend {
    fn get_brightness(&self) -> Result<u8> {
        if !self.is_available() {
            return Ok(100);
        }

        static BRIGHT_RE: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"current value\s*=\s*(\d+)").unwrap());
        let output = match self.spawn_command("getvcp", None) {
            Ok(output) => output,
            Err(err) => {
                warn!(error = %err, "ddcutil getvcp failed; disabling brightness backend");
                self.mark_unavailable(err.to_string());
                return Ok(100);
            }
        };
        let captures = match BRIGHT_RE.captures(&output).and_then(|cap| cap.get(1)) {
            Some(capture) => capture,
            None => {
                warn!("unable to parse brightness from {output}");
                self.mark_unavailable("unexpected ddcutil getvcp output");
                return Ok(100);
            }
        };
        let value = captures
            .as_str()
            .parse::<u16>()
            .context("failed to parse brightness value")?;
        Ok(value.min(100) as u8)
    }

    fn set_brightness(&self, value: u8) -> Result<u8> {
        if !self.is_available() {
            return Ok(value.min(100));
        }

        if let Err(err) = self.spawn_command("setvcp", Some(value.min(100).to_string())) {
            warn!(error = %err, "ddcutil setvcp failed; disabling brightness backend");
            self.mark_unavailable(err.to_string());
            return Ok(value.min(100));
        }
        // Re-read value to keep state accurate
        self.get_brightness()
    }

    fn is_available(&self) -> bool {
        DdcutilBackend::is_available(self)
    }
}

fn warn_backend_disabled(reason: &str) {
    if !WARNED_UNAVAILABLE.swap(true, Ordering::Relaxed) {
        warn!(
            "ddcutil backend disabled ({reason}); brightness encoder operates in placeholder mode"
        );
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub struct MockBrightnessBackend {
        pub inner: Arc<Mutex<MockBrightnessState>>,
    }

    impl BrightnessBackend for MockBrightnessBackend {
        fn get_brightness(&self) -> Result<u8> {
            Ok(self.inner.lock().unwrap().level)
        }

        fn set_brightness(&self, value: u8) -> Result<u8> {
            let mut state = self.inner.lock().unwrap();
            state.history.push(value);
            state.level = value;
            Ok(value)
        }
    }

    #[derive(Debug)]
    pub struct MockBrightnessState {
        pub level: u8,
        pub history: Vec<u8>,
    }

    impl Default for MockBrightnessState {
        fn default() -> Self {
            Self {
                level: 50,
                history: Vec::new(),
            }
        }
    }
}
