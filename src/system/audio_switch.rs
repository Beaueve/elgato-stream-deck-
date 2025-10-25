use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkInfo {
    pub id: Option<u32>,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkSelector {
    Id(u32),
    Name(String),
    Description(String),
}

impl SinkSelector {
    pub fn by_id(id: u32) -> Self {
        Self::Id(id)
    }

    pub fn by_name(name: impl Into<String>) -> Self {
        Self::Name(name.into())
    }

    pub fn by_description(description: impl Into<String>) -> Self {
        Self::Description(description.into())
    }

    pub fn describe(&self) -> &str {
        match self {
            SinkSelector::Id(_) => "specified sink id",
            SinkSelector::Name(name) => name.as_str(),
            SinkSelector::Description(description) => description.as_str(),
        }
    }

    pub fn matches(&self, sink: &SinkInfo) -> bool {
        match self {
            SinkSelector::Id(expected) => sink.id == Some(*expected),
            SinkSelector::Name(expected) => {
                let expected = expected.to_ascii_lowercase();
                let name = sink.name.to_ascii_lowercase();
                if name == expected || name.contains(&expected) {
                    return true;
                }
                sink.description
                    .as_ref()
                    .map(|desc| desc.to_ascii_lowercase().contains(&expected))
                    .unwrap_or(false)
            }
            SinkSelector::Description(expected) => {
                let expected = expected.to_ascii_lowercase();

                if sink.name.to_ascii_lowercase() == expected {
                    return true;
                }

                sink.description
                    .as_ref()
                    .map(|desc| desc.to_ascii_lowercase())
                    .map(|desc| desc == expected || desc.contains(&expected))
                    .unwrap_or(false)
            }
        }
    }
}

pub trait AudioSwitchBackend: Send + Sync {
    fn set_default_sink(&self, selector: &SinkSelector) -> Result<SinkInfo>;
    fn current_default_sink(&self) -> Result<Option<SinkInfo>>;
    fn list_sinks(&self) -> Result<Vec<SinkInfo>>;
}

#[derive(Debug, Default, Clone)]
pub struct PulseAudioSwitch;

impl PulseAudioSwitch {
    pub fn new() -> Self {
        Self
    }

    fn run_pactl(args: &[&str]) -> Result<String> {
        let output = Command::new("pactl")
            .args(args)
            .output()
            .with_context(|| format!("failed to execute pactl with args {args:?}"))?;

        if !output.status.success() {
            bail!(
                "pactl exited with status {}",
                output.status.code().unwrap_or(-1)
            );
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn list_sinks_internal(&self) -> Result<Vec<SinkInfo>> {
        let output = Self::run_pactl(&["list", "sinks"])?;
        let sinks = parse_sinks(&output);
        if sinks.is_empty() {
            bail!("no sinks reported by pactl");
        }
        Ok(sinks)
    }

    fn move_inputs(target_sink: &str) -> Result<()> {
        let output = Self::run_pactl(&["list", "short", "sink-inputs"])?;
        for input in parse_sink_inputs(&output) {
            if let Err(err) = Self::run_pactl(&["move-sink-input", &input, target_sink]) {
                tracing::warn!(
                    error = %err,
                    sink_input = %input,
                    target = target_sink,
                    "failed to move sink input"
                );
            }
        }
        Ok(())
    }
}

impl AudioSwitchBackend for PulseAudioSwitch {
    fn set_default_sink(&self, selector: &SinkSelector) -> Result<SinkInfo> {
        let sinks = self.list_sinks_internal()?;
        let sink = select_sink(&sinks, selector)?;

        Self::run_pactl(&["set-default-sink", &sink.name])
            .with_context(|| format!("failed to set default sink to {}", sink.name))?;

        if let Err(err) = Self::move_inputs(&sink.name) {
            tracing::warn!(error = %err, "failed to move sink inputs to {}", sink.name);
        }

        Ok(sink.clone())
    }

    fn current_default_sink(&self) -> Result<Option<SinkInfo>> {
        let output = Self::run_pactl(&["info"])?;
        let Some(default) = parse_default_sink(&output) else {
            return Ok(None);
        };

        let sinks = self.list_sinks_internal()?;
        if let Some(found) = sinks.iter().find(|sink| sink.name == default) {
            return Ok(Some(found.clone()));
        }

        let default_lower = default.to_ascii_lowercase();
        if let Some(found) = sinks.iter().find(|sink| {
            sink.description
                .as_ref()
                .map(|desc| desc.to_ascii_lowercase() == default_lower)
                .unwrap_or(false)
        }) {
            return Ok(Some(found.clone()));
        }

        Ok(Some(SinkInfo {
            id: None,
            name: default,
            description: None,
        }))
    }

    fn list_sinks(&self) -> Result<Vec<SinkInfo>> {
        self.list_sinks_internal()
    }
}

pub(crate) fn select_sink<'a>(
    sinks: &'a [SinkInfo],
    selector: &SinkSelector,
) -> Result<&'a SinkInfo> {
    if let Some(found) = sinks.iter().find(|sink| selector.matches(sink)) {
        return Ok(found);
    }

    match selector {
        SinkSelector::Description(description) => {
            let description_lower = description.to_ascii_lowercase();
            if let Some(found) = sinks.iter().find(|sink| {
                sink.description
                    .as_ref()
                    .map(|desc| desc.to_ascii_lowercase().contains(&description_lower))
                    .unwrap_or(false)
            }) {
                return Ok(found);
            }
        }
        SinkSelector::Name(_) => {}
        SinkSelector::Id(_) => {}
    }

    Err(match selector {
        SinkSelector::Id(id) => anyhow!("no matching sink found for sink id {}", id),
        SinkSelector::Name(_) | SinkSelector::Description(_) => {
            anyhow!("no matching sink found for {}", selector.describe())
        }
    })
}

pub(crate) fn parse_sinks(output: &str) -> Vec<SinkInfo> {
    let mut sinks = Vec::new();
    let mut current_id: Option<u32> = None;
    let mut current_name: Option<String> = None;
    let mut description: Option<String> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("Sink #") {
            if let Some(name) = current_name.take() {
                sinks.push(SinkInfo {
                    id: current_id,
                    name,
                    description: description.take(),
                });
            }
            // reset for the next sink
            current_name = None;
            description = None;
            current_id = value
                .split_whitespace()
                .next()
                .and_then(|value| value.parse().ok());
            continue;
        }

        if let Some(value) = trimmed.strip_prefix("Name:") {
            current_name = Some(value.trim().to_string());
            continue;
        }

        if let Some(value) = trimmed.strip_prefix("Description:") {
            description = Some(value.trim().to_string());
            continue;
        }

        if let Some(value) = trimmed.strip_prefix("device.description =") {
            let value = value.trim().trim_matches('"');
            if !value.is_empty() {
                description = Some(value.to_string());
            }
        }
    }

    if let Some(name) = current_name {
        sinks.push(SinkInfo {
            id: current_id,
            name,
            description,
        });
    }

    sinks
}

pub(crate) fn parse_default_sink(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix("Default Sink:")
            .map(|value| value.trim().to_string())
    })
}

fn parse_sink_inputs(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            trimmed
                .split_whitespace()
                .next()
                .map(|value| value.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_sinks() {
        let output = r#"
Sink #1
    State: RUNNING
    Name: alsa_output.pci-0000_09_00.3.hdmi-stereo-extra2
    Description: HDMI/DisplayPort 3 (HDA NVidia Digital Stereo (HDMI))
    Properties:
        device.description = "HDMI/DisplayPort - HDA NVidia"

Sink #2
    State: IDLE
    Name: alsa_output.usb-SteelSeries_Arctis_Pro-00.analog-stereo
    Properties:
        device.description = "Digital Output (SteelSeries Arctis Pro)"
"#;

        let sinks = parse_sinks(output);
        assert_eq!(sinks.len(), 2);
        assert_eq!(sinks[0].id, Some(1));
        assert_eq!(
            sinks[0].name,
            "alsa_output.pci-0000_09_00.3.hdmi-stereo-extra2"
        );
        assert_eq!(
            sinks[0].description.as_deref(),
            Some("HDMI/DisplayPort - HDA NVidia")
        );
        assert_eq!(sinks[1].id, Some(2));
        assert_eq!(
            sinks[1].description.as_deref(),
            Some("Digital Output (SteelSeries Arctis Pro)")
        );
    }

    #[test]
    fn parses_default_sink() {
        let output = r#"
Server String: /run/user/1000/pulse/native
Default Sink: alsa_output.usb-SteelSeries_Arctis_Pro-00.analog-stereo
Default Source: alsa_input.usb-SteelSeries_Arctis_Pro-00.mono-fallback
"#;
        let default = parse_default_sink(output);
        assert_eq!(
            default,
            Some("alsa_output.usb-SteelSeries_Arctis_Pro-00.analog-stereo".to_string())
        );
    }

    #[test]
    fn selects_by_description_substring() {
        let sinks = vec![
            SinkInfo {
                id: Some(1),
                name: "sink_a".into(),
                description: Some("First Sink".into()),
            },
            SinkInfo {
                id: Some(2),
                name: "sink_b".into(),
                description: Some("Second Device".into()),
            },
        ];

        let selector = SinkSelector::by_description("second");
        let selected = select_sink(&sinks, &selector).unwrap();
        assert_eq!(selected.name, "sink_b");
    }

    #[test]
    fn parse_sink_inputs_extracts_ids() {
        let output = r#"
36  123 sink_b  protocol-native.c  s16le 2ch 44100Hz
37  321 sink_a  protocol-native.c  s16le 2ch 44100Hz
"#;
        let inputs = parse_sink_inputs(output);
        assert_eq!(inputs, vec!["36".to_string(), "37".to_string()]);
    }

    #[test]
    fn selects_by_id() {
        let sinks = vec![
            SinkInfo {
                id: Some(11),
                name: "sink_a".into(),
                description: Some("First Sink".into()),
            },
            SinkInfo {
                id: Some(12),
                name: "sink_b".into(),
                description: Some("Second Device".into()),
            },
        ];

        let selector = SinkSelector::by_id(12);
        let selected = select_sink(&sinks, &selector).unwrap();
        assert_eq!(selected.name, "sink_b");
    }
}
