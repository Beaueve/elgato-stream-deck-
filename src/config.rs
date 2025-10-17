use std::env;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::Value;

use crate::controls::AudioToggleConfig;

#[derive(Debug, Clone)]
pub struct StreamDeckSettings {
    pub path: PathBuf,
    pub audio_toggle: Option<AudioToggleConfig>,
    pub launchers: Vec<LauncherButtonConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LauncherButtonConfig {
    #[serde(alias = "index", alias = "button")]
    pub button_index: u8,
    #[serde(alias = "desktop", alias = "path")]
    pub desktop_file: PathBuf,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct StructuredConfig {
    pub audio_toggle: Option<AudioToggleConfig>,
    pub launchers: Vec<LauncherButtonConfig>,
}

pub fn load_settings() -> Result<Option<StreamDeckSettings>> {
    for candidate in default_config_paths() {
        if !candidate.exists() {
            continue;
        }
        let contents = fs::read_to_string(&candidate).with_context(|| {
            format!(
                "failed to read streamdeck_ctrl configuration at {}",
                candidate.display()
            )
        })?;
        let structured = parse_config(&contents).with_context(|| {
            format!(
                "failed to parse streamdeck_ctrl configuration at {}",
                candidate.display()
            )
        })?;
        return Ok(Some(StreamDeckSettings {
            path: candidate,
            audio_toggle: structured.audio_toggle,
            launchers: structured.launchers,
        }));
    }
    Ok(None)
}

fn parse_config(contents: &str) -> Result<StructuredConfig> {
    let value: Value =
        serde_json::from_str(contents).context("configuration file is not valid JSON")?;

    if let Some(object) = value.as_object() {
        let mut map = object.clone();

        let launchers = map
            .remove("launchers")
            .map(|raw| {
                serde_json::from_value(raw)
                    .context("failed to parse `launchers` entries from configuration")
            })
            .transpose()?
            .unwrap_or_default();

        let audio_toggle = map
            .remove("audio_toggle")
            .map(|raw| {
                serde_json::from_value(raw)
                    .context("failed to parse `audio_toggle` configuration section")
            })
            .transpose()?;

        let inline_toggle = if audio_toggle.is_none() && map.contains_key("outputs") {
            let mut inline_map = map.clone();
            inline_map.remove("launchers");
            serde_json::from_value(Value::Object(inline_map)).ok()
        } else {
            None
        };

        return Ok(StructuredConfig {
            audio_toggle: audio_toggle.or(inline_toggle),
            launchers,
        });
    }

    match serde_json::from_value::<AudioToggleConfig>(value.clone()) {
        Ok(audio_toggle) => Ok(StructuredConfig {
            audio_toggle: Some(audio_toggle),
            launchers: Vec::new(),
        }),
        Err(err) => Err(anyhow!(err)),
    }
}

pub fn default_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(explicit) = env::var_os("STREAMDECK_CTRL_CONFIG") {
        paths.push(PathBuf::from(explicit));
    }

    let candidate_names = ["stream-deck.json", "audio_toggle.json"];

    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME") {
        let base = PathBuf::from(xdg).join("streamdeck_ctrl");
        for name in &candidate_names {
            paths.push(base.join(name));
        }
    }

    if let Some(home) = env::var_os("HOME") {
        let base = PathBuf::from(home).join(".config/streamdeck_ctrl");
        for name in &candidate_names {
            paths.push(base.join(name));
        }
    }

    for name in &candidate_names {
        paths.push(PathBuf::from(name));
        paths.push(PathBuf::from("config").join(name));
        paths.push(PathBuf::from("target/debug").join(name));
        paths.push(PathBuf::from("target/release").join(name));
        let legacy = match *name {
            "stream-deck.json" => "audio_toggle.json",
            other => other,
        };
        paths.push(PathBuf::from("target/debug").join(legacy));
        paths.push(PathBuf::from("target/release").join(legacy));
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    #[test]
    fn parses_structured_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("stream-deck.json");
        fs::write(
            &path,
            r#"{
                "audio_toggle": {
                    "button_index": 1,
                    "outputs": [
                        {"description": "Display"},
                        {"description": "Headset"}
                    ]
                },
                "launchers": [
                    {"button_index": 4, "desktop_file": "/tmp/app.desktop"}
                ]
            }"#,
        )
        .unwrap();

        let settings = parse_config(
            &fs::read_to_string(&path).expect("failed to read written config"),
        )
        .unwrap();

        assert!(settings.audio_toggle.is_some());
        assert_eq!(settings.launchers.len(), 1);
        assert_eq!(settings.launchers[0].button_index, 4);
        assert_eq!(
            settings.launchers[0].desktop_file,
            PathBuf::from("/tmp/app.desktop")
        );
    }

    #[test]
    fn parses_legacy_audio_toggle_only_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("stream-deck.json");
        fs::write(
            &path,
            r#"{
                "button_index": 0,
                "outputs": [
                    { "description": "Primary" },
                    { "description": "Secondary" }
                ]
            }"#,
        )
        .unwrap();

        let settings = parse_config(
            &fs::read_to_string(&path).expect("failed to read written config"),
        )
        .unwrap();

        assert!(settings.audio_toggle.is_some());
        assert!(settings.launchers.is_empty());
    }
}
