use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use tracing::{info, warn};

use crate::hardware::{ButtonImage, DisplayPipeline};
use crate::system::audio_switch::{AudioSwitchBackend, PulseAudioSwitch, SinkInfo, SinkSelector};
use crate::util::icons;

const MATERIAL_ICON_TINT: [u8; 3] = [220, 235, 255];

#[derive(Debug, Clone, Deserialize)]
pub struct AudioToggleConfig {
    #[serde(default = "default_button_index")]
    pub button_index: Option<u8>,
    #[serde(default)]
    pub outputs: Vec<AudioOutputConfig>,
}

#[derive(Debug, Clone)]
pub struct AudioToggleSettings {
    pub config: AudioToggleConfig,
    pub config_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AudioOutputConfig {
    #[serde(default)]
    pub button_index: Option<u8>,
    #[serde(default)]
    pub id: Option<u32>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon: Option<IconConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum IconConfig {
    Material { material: MaterialIcon },
    Path { path: String },
    Simple(MaterialIcon),
    File(String),
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaterialIcon {
    Monitor,
    Headphones,
}

const ACTIVE_TINT: [u8; 3] = [0, 200, 150];
const AVAILABLE_TINT: [u8; 3] = [120, 185, 255];
const UNAVAILABLE_TINT: [u8; 3] = [110, 110, 125];
const DEGRADED_TINT: [u8; 3] = [230, 170, 90];

fn default_button_index() -> Option<u8> {
    Some(0)
}

impl AudioToggleConfig {
    pub fn load_default() -> Result<Option<AudioToggleSettings>> {
        if let Some(settings) = crate::config::load_settings()? {
            if let Some(config) = settings.audio_toggle {
                return Ok(Some(AudioToggleSettings {
                    config,
                    config_path: Some(settings.path),
                }));
            }
        }
        Ok(None)
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read audio toggle config at {}", path.display()))?;
        let config: Self = serde_json::from_str(&contents).with_context(|| {
            format!("failed to parse audio toggle config at {}", path.display())
        })?;
        Ok(config)
    }
}

pub struct AudioToggleController<B, H>
where
    B: AudioSwitchBackend,
    H: DisplayPipeline,
{
    backend: B,
    hardware: H,
    outputs: Vec<OutputEntry>,
    button_map: HashMap<u8, Vec<usize>>,
}

#[derive(Debug, Clone)]
struct OutputEntry {
    profile: OutputProfile,
    state: OutputState,
}

#[derive(Debug, Clone)]
struct OutputProfile {
    selector: SinkSelector,
    icons: OutputIcons,
    label: String,
    button_index: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OutputState {
    available: bool,
    active: bool,
}

impl Default for OutputState {
    fn default() -> Self {
        Self {
            available: false,
            active: false,
        }
    }
}

#[derive(Debug, Clone)]
struct OutputIcons {
    available_selected: ButtonImage,
    available_inactive: ButtonImage,
    unavailable_selected: ButtonImage,
    unavailable_inactive: ButtonImage,
}

impl<B, H> AudioToggleController<B, H>
where
    B: AudioSwitchBackend,
    H: DisplayPipeline,
{
    fn new(
        config: AudioToggleConfig,
        backend: B,
        hardware: H,
        icon_paths: &IconPaths,
    ) -> Result<Self> {
        if config.outputs.is_empty() {
            bail!("audio toggle requires at least one configured output");
        }

        let fallback_button = config.button_index;

        let mut outputs = Vec::with_capacity(config.outputs.len());
        for (index, entry) in config.outputs.iter().enumerate() {
            let profile = OutputProfile::from_config(entry, fallback_button, index, icon_paths)?;
            outputs.push(OutputEntry {
                profile,
                state: OutputState::default(),
            });
        }

        let mut button_map: HashMap<u8, Vec<usize>> = HashMap::new();
        for (idx, entry) in outputs.iter().enumerate() {
            button_map
                .entry(entry.profile.button_index)
                .or_default()
                .push(idx);
        }

        let mut controller = Self {
            backend,
            hardware,
            outputs,
            button_map,
        };
        controller.initialise_icons()?;
        controller.refresh_state()?;
        Ok(controller)
    }

    pub fn on_button_pressed(&mut self, button_index: u8) -> Result<bool> {
        let Some(indices) = self.button_map.get(&button_index) else {
            return Ok(false);
        };

        if indices.is_empty() {
            return Ok(false);
        }

        let target_index = if indices.len() == 1 {
            indices[0]
        } else {
            self.select_next_in_group(indices)
        };

        let target = &self.outputs[target_index];
        info!(target = %target.profile.label, "switching audio output");

        match self
            .backend
            .set_default_sink(&target.profile.selector)
            .with_context(|| format!("failed to set default sink to {}", target.profile.label))
        {
            Ok(_) => {
                if let Err(err) = self.refresh_state() {
                    warn!(
                        error = %err,
                        "failed to refresh audio sink state after switch"
                    );
                }
            }
            Err(err) => {
                warn!(
                    error = %err,
                    target = %target.profile.label,
                    "failed to switch audio output"
                );
                notify_switch_failure(&target.profile.label, &err);
                if let Err(refresh_err) = self.refresh_state() {
                    warn!(
                        error = %refresh_err,
                        "failed to refresh audio sink state after switch failure"
                    );
                }
            }
        }

        Ok(true)
    }

    pub fn on_tick(&mut self) -> Result<()> {
        self.refresh_state()
    }

    fn select_next_in_group(&self, indices: &[usize]) -> usize {
        if indices.len() <= 1 {
            return indices[0];
        }

        let active_position = indices.iter().enumerate().find_map(|(pos, idx)| {
            let active = self.outputs[*idx].state.active;
            active.then_some(pos)
        });

        if let Some(pos) = active_position {
            let next = (pos + 1) % indices.len();
            return indices[next];
        }

        indices
            .iter()
            .copied()
            .find(|idx| self.outputs[*idx].state.available)
            .unwrap_or(indices[0])
    }

    fn initialise_icons(&mut self) -> Result<()> {
        for idx in 0..self.outputs.len() {
            self.push_icon(idx)?;
        }
        Ok(())
    }

    fn refresh_state(&mut self) -> Result<()> {
        let sinks = self.backend.list_sinks()?;
        let current = self.backend.current_default_sink()?;
        let mut matched_default = false;

        for index in 0..self.outputs.len() {
            let profile = &self.outputs[index].profile;
            let available = sinks.iter().any(|sink| profile.selector.matches(sink));
            let active = current
                .as_ref()
                .map(|sink| profile.selector.matches(sink))
                .unwrap_or(false);
            if active {
                matched_default = true;
            }
            let new_state = OutputState { available, active };
            self.apply_state(index, new_state)?;
        }

        if let Some(current_sink) = &current {
            if !matched_default {
                warn!(
                    sink = %current_sink.name,
                    "default sink not present in audio toggle configuration"
                );
            }
        }

        Ok(())
    }

    fn apply_state(&mut self, index: usize, new_state: OutputState) -> Result<()> {
        let entry = self
            .outputs
            .get_mut(index)
            .ok_or_else(|| anyhow!("output index {} out of bounds", index))?;

        if entry.state == new_state {
            return Ok(());
        }

        entry.state = new_state;
        self.push_icon(index)
    }

    fn push_icon(&self, index: usize) -> Result<()> {
        let entry = self
            .outputs
            .get(index)
            .ok_or_else(|| anyhow!("output index {} out of bounds", index))?;
        let icon = entry.profile.icons.icon(entry.state);
        self.hardware
            .update_button_icon(entry.profile.button_index, Some(icon))
    }

    #[cfg(test)]
    fn state_for_index(&self, index: usize) -> OutputState {
        self.outputs[index].state
    }
}

impl<H> AudioToggleController<PulseAudioSwitch, H>
where
    H: DisplayPipeline,
{
    pub fn with_default_backend(
        settings: AudioToggleSettings,
        hardware: H,
    ) -> Result<AudioToggleController<PulseAudioSwitch, H>> {
        let icon_paths = IconPaths::new(settings.config_path.as_deref());
        AudioToggleController::new(
            settings.config,
            PulseAudioSwitch::new(),
            hardware,
            &icon_paths,
        )
    }
}

impl OutputProfile {
    fn from_config(
        config: &AudioOutputConfig,
        fallback_button: Option<u8>,
        index: usize,
        icon_paths: &IconPaths,
    ) -> Result<Self> {
        let selector = config.selector()?;
        let button_index = config.button_index.or(fallback_button).ok_or_else(|| {
            anyhow!(
                "audio output configuration at index {} must define `button_index`",
                index
            )
        })?;
        let fallback_icon = match index {
            0 => MaterialIcon::Monitor,
            _ => MaterialIcon::Headphones,
        };
        let mut base_icon = load_icon_from_config(config.icon.as_ref(), fallback_icon, icon_paths)?;
        base_icon.tint = None;
        let label = config.label();
        let icons = OutputIcons::from_base(&base_icon, button_index, index);
        Ok(Self {
            selector,
            icons,
            label,
            button_index,
        })
    }
}

impl AudioOutputConfig {
    fn selector(&self) -> Result<SinkSelector> {
        if let Some(id) = self.id {
            return Ok(SinkSelector::by_id(id));
        }

        if let Some(name) = &self.name {
            return Ok(SinkSelector::by_name(name.clone()));
        }

        if let Some(description) = &self.description {
            return Ok(SinkSelector::by_description(description.clone()));
        }

        bail!("audio toggle output entry must provide `id`, `name`, or `description`");
    }

    fn label(&self) -> String {
        self.name
            .as_ref()
            .or(self.description.as_ref())
            .cloned()
            .or_else(|| self.id.map(|id| format!("sink #{id}")))
            .unwrap_or_else(|| "unnamed sink".to_string())
    }
}

impl OutputIcons {
    fn from_base(base: &ButtonImage, button_index: u8, index: usize) -> Self {
        let base_id = normalize_id(&base.id);
        Self {
            available_selected: tinted_variant(
                base,
                button_index,
                index,
                &base_id,
                "active",
                ACTIVE_TINT,
            ),
            available_inactive: tinted_variant(
                base,
                button_index,
                index,
                &base_id,
                "available",
                AVAILABLE_TINT,
            ),
            unavailable_selected: tinted_variant(
                base,
                button_index,
                index,
                &base_id,
                "unavailable-active",
                DEGRADED_TINT,
            ),
            unavailable_inactive: tinted_variant(
                base,
                button_index,
                index,
                &base_id,
                "unavailable",
                UNAVAILABLE_TINT,
            ),
        }
    }

    fn icon(&self, state: OutputState) -> ButtonImage {
        match (state.available, state.active) {
            (true, true) => self.available_selected.clone(),
            (true, false) => self.available_inactive.clone(),
            (false, true) => self.unavailable_selected.clone(),
            (false, false) => self.unavailable_inactive.clone(),
        }
    }
}

fn normalize_id(id: &str) -> String {
    let mut slug = String::with_capacity(id.len());
    for ch in id.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' {
            if !slug.ends_with(ch) {
                slug.push(ch);
            }
        }
    }

    if slug.is_empty() {
        "icon".to_string()
    } else {
        slug.truncate(32);
        slug
    }
}

fn tinted_variant(
    base: &ButtonImage,
    button_index: u8,
    index: usize,
    base_id: &str,
    suffix: &str,
    tint: [u8; 3],
) -> ButtonImage {
    ButtonImage {
        id: format!("audio-{}-{}-{}-{}", button_index, index, base_id, suffix),
        image: Arc::clone(&base.image),
        tint: Some(tint),
    }
}

fn load_icon_from_config(
    icon: Option<&IconConfig>,
    fallback: MaterialIcon,
    paths: &IconPaths,
) -> Result<ButtonImage> {
    match icon {
        Some(IconConfig::Material { material }) => load_material_icon(*material, paths),
        Some(IconConfig::Path { path }) => load_icon_from_path(Path::new(path), path, None, paths),
        Some(IconConfig::Simple(material)) => load_material_icon(*material, paths),
        Some(IconConfig::File(path)) => load_icon_from_path(Path::new(path), path, None, paths),
        None => load_material_icon(fallback, paths),
    }
}

fn load_material_icon(icon: MaterialIcon, paths: &IconPaths) -> Result<ButtonImage> {
    let (filename, id) = match icon {
        MaterialIcon::Monitor => ("monitor.svg", "monitor"),
        MaterialIcon::Headphones => ("headphones.svg", "headphones"),
    };

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(root) = &paths.assets_root {
        candidates.push(root.join(filename));
    }
    if let Some(base) = &paths.base_dir {
        candidates.push(base.join(filename));
    }
    candidates.push(PathBuf::from("assets/icons/material").join(filename));

    let mut last_error: Option<anyhow::Error> = None;
    for candidate in candidates {
        if candidate.exists() {
            match load_icon_from_resolved(&candidate, id.to_string(), Some(MATERIAL_ICON_TINT)) {
                Ok(icon) => return Ok(icon),
                Err(err) => last_error = Some(err),
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        anyhow!(
            "material icon {} not found; expected it in assets directory",
            filename
        )
    }))
}

fn load_icon_from_path(
    path: &Path,
    id_hint: impl Into<String>,
    tint: Option<[u8; 3]>,
    paths: &IconPaths,
) -> Result<ButtonImage> {
    let id = id_hint.into();
    let resolved = resolve_icon_path(path, paths)
        .ok_or_else(|| anyhow!("icon not found at {}", path.display()))?;
    load_icon_from_resolved(&resolved, id, tint)
}

fn resolve_icon_path(path: &Path, paths: &IconPaths) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if path.is_absolute() {
        candidates.push(path.to_path_buf());
    } else {
        if let Some(base) = &paths.base_dir {
            candidates.push(base.join(path));
        }
        if let Some(assets) = &paths.assets_root {
            candidates.push(assets.join(path));
        }
        candidates.push(PathBuf::from(path));
    }

    for candidate in candidates {
        if let Ok(canonical) = candidate.canonicalize() {
            return Some(canonical);
        } else if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn load_icon_from_resolved(path: &Path, id: String, tint: Option<[u8; 3]>) -> Result<ButtonImage> {
    let image = icons::load_icon(path)?;
    Ok(ButtonImage { id, image, tint })
}

fn notify_switch_failure(label: &str, error: &anyhow::Error) {
    let title = "Stream Deck Audio Toggle";
    let body = format!("Failed to switch to {}:\n{}", label, error);
    match Command::new("notify-send").arg(title).arg(body).status() {
        Ok(status) => {
            if !status.success() {
                warn!(code = ?status.code(), "notify-send exited with failure status");
            }
        }
        Err(err) => {
            warn!(error = %err, "failed to send audio switch failure notification");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::hardware::{ButtonImage, EncoderDisplay, EncoderId};
    use once_cell::sync::Lazy;
    use std::env;
    use std::sync::{Arc, Mutex};

    struct RecordingHardware {
        inner: Mutex<Vec<(u8, Option<String>)>>,
    }

    impl RecordingHardware {
        fn new() -> Self {
            Self {
                inner: Mutex::new(Vec::new()),
            }
        }

        fn updates(&self) -> Vec<(u8, Option<String>)> {
            self.inner.lock().unwrap().clone()
        }
    }

    impl DisplayPipeline for RecordingHardware {
        fn update_encoder(&self, _encoder: EncoderId, _display: EncoderDisplay) -> Result<()> {
            Ok(())
        }

        fn update_button_icon(&self, index: u8, icon: Option<ButtonImage>) -> Result<()> {
            let id = icon.map(|value| value.id.clone());
            self.inner.lock().unwrap().push((index, id));
            Ok(())
        }
    }

    impl DisplayPipeline for Arc<RecordingHardware> {
        fn update_encoder(&self, _encoder: EncoderId, _display: EncoderDisplay) -> Result<()> {
            Ok(())
        }

        fn update_button_icon(&self, index: u8, icon: Option<ButtonImage>) -> Result<()> {
            let id = icon.map(|value| value.id.clone());
            self.inner.lock().unwrap().push((index, id));
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeBackend {
        sinks: Vec<SinkInfo>,
        set_calls: std::sync::Mutex<Vec<SinkSelector>>,
        current: std::sync::Mutex<Option<SinkInfo>>,
    }

    impl AudioSwitchBackend for FakeBackend {
        fn set_default_sink(&self, selector: &SinkSelector) -> Result<SinkInfo> {
            self.set_calls.lock().unwrap().push(selector.clone());
            let sink = self
                .sinks
                .iter()
                .find(|sink| selector.matches(sink))
                .cloned()
                .ok_or_else(|| anyhow!("no sink matches selector {:?}", selector))?;
            *self.current.lock().unwrap() = Some(sink.clone());
            Ok(sink)
        }

        fn current_default_sink(&self) -> Result<Option<SinkInfo>> {
            Ok(self.current.lock().unwrap().clone())
        }

        fn list_sinks(&self) -> Result<Vec<SinkInfo>> {
            Ok(self.sinks.clone())
        }
    }

    fn sample_config() -> AudioToggleConfig {
        AudioToggleConfig {
            button_index: Some(2),
            outputs: vec![
                AudioOutputConfig {
                    button_index: None,
                    id: Some(1),
                    name: None,
                    description: Some("HDMI/DisplayPort - HDA NVidia".into()),
                    icon: Some(IconConfig::Material {
                        material: MaterialIcon::Monitor,
                    }),
                },
                AudioOutputConfig {
                    button_index: None,
                    id: Some(2),
                    name: None,
                    description: Some("Digital Output - A50".into()),
                    icon: Some(IconConfig::Material {
                        material: MaterialIcon::Headphones,
                    }),
                },
            ],
        }
    }

    fn multi_button_config() -> AudioToggleConfig {
        AudioToggleConfig {
            button_index: None,
            outputs: vec![
                AudioOutputConfig {
                    button_index: Some(0),
                    id: Some(1),
                    name: Some("sink_monitor".into()),
                    description: Some("Monitor".into()),
                    icon: Some(IconConfig::Material {
                        material: MaterialIcon::Monitor,
                    }),
                },
                AudioOutputConfig {
                    button_index: Some(1),
                    id: Some(2),
                    name: Some("sink_headset".into()),
                    description: Some("Headset".into()),
                    icon: Some(IconConfig::Material {
                        material: MaterialIcon::Headphones,
                    }),
                },
                AudioOutputConfig {
                    button_index: Some(2),
                    id: Some(3),
                    name: Some("sink_earbuds".into()),
                    description: Some("Earbuds".into()),
                    icon: Some(IconConfig::Material {
                        material: MaterialIcon::Headphones,
                    }),
                },
            ],
        }
    }

    #[test]
    fn config_loads_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio_toggle.json");
        fs::write(
            &path,
            r#"{
            "button_index": 1,
            "outputs": [
                { "description": "Output A", "icon": { "material": "monitor" } },
                { "description": "Output B", "icon": { "material": "headphones" } }
            ]
        }"#,
        )
        .unwrap();

        let config = AudioToggleConfig::from_path(&path).unwrap();
        assert_eq!(config.button_index, Some(1));
        assert_eq!(config.outputs[0].description.as_deref(), Some("Output A"));
    }

    #[test]
    fn parses_string_icon_path() {
        let config: AudioToggleConfig = serde_json::from_str(
            r#"{
                "button_index": 0,
                "outputs": [
                    { "description": "Monitor", "icon": "assets/monitor.png" },
                    { "description": "Headset" }
                ]
            }"#,
        )
        .unwrap();

        match config.outputs[0].icon.as_ref().unwrap() {
            IconConfig::File(path) => assert_eq!(path, "assets/monitor.png"),
            other => panic!("unexpected icon variant: {:?}", other),
        }
    }

    static ENV_GUARD: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    #[test]
    fn load_default_prefers_env_override() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("stream-deck.json");
        fs::write(
            &config_path,
            r#"{
            "button_index": 0,
            "outputs": [
                { "description": "Env Monitor" },
                { "description": "Env Headphones" }
            ]
        }"#,
        )
        .unwrap();

        let _guard = ENV_GUARD.lock().unwrap();
        let previous = env::var_os("STREAMDECK_CTRL_CONFIG");
        unsafe {
            // UNSAFETY: modifying process-wide environment for duration of test
            env::set_var("STREAMDECK_CTRL_CONFIG", &config_path);
        }

        let settings = AudioToggleConfig::load_default().unwrap().unwrap();
        assert_eq!(
            settings.config.outputs[0].description.as_deref(),
            Some("Env Monitor")
        );

        if let Some(value) = previous {
            unsafe {
                env::set_var("STREAMDECK_CTRL_CONFIG", value);
            }
        } else {
            unsafe {
                env::remove_var("STREAMDECK_CTRL_CONFIG");
            }
        }
    }

    #[test]
    fn controller_initialises_with_current_sink() {
        let config = sample_config();
        let backend = FakeBackend {
            sinks: vec![
                SinkInfo {
                    id: Some(1),
                    name: "sink_a".into(),
                    description: Some("HDMI/DisplayPort - HDA NVidia".into()),
                },
                SinkInfo {
                    id: Some(2),
                    name: "sink_b".into(),
                    description: Some("Digital Output - A50".into()),
                },
            ],
            current: std::sync::Mutex::new(Some(SinkInfo {
                id: Some(2),
                name: "sink_b".into(),
                description: Some("Digital Output - A50".into()),
            })),
            ..Default::default()
        };

        let hardware = RecordingHardware::new();
        let icon_paths = IconPaths::new(None);
        let controller =
            AudioToggleController::new(config, backend, Arc::new(hardware), &icon_paths).unwrap();
        assert!(controller.state_for_index(0).available);
        assert!(controller.state_for_index(1).active);
    }

    #[test]
    fn toggles_between_outputs() {
        let config = sample_config();
        let backend = FakeBackend {
            sinks: vec![
                SinkInfo {
                    id: Some(1),
                    name: "sink_monitor".into(),
                    description: Some("HDMI/DisplayPort - HDA NVidia".into()),
                },
                SinkInfo {
                    id: Some(2),
                    name: "sink_headset".into(),
                    description: Some("Digital Output - A50".into()),
                },
            ],
            current: std::sync::Mutex::new(Some(SinkInfo {
                id: Some(1),
                name: "sink_monitor".into(),
                description: Some("HDMI/DisplayPort - HDA NVidia".into()),
            })),
            ..Default::default()
        };

        let hardware = Arc::new(RecordingHardware::new());
        let icon_paths = IconPaths::new(None);
        let mut controller =
            AudioToggleController::new(config, backend, Arc::clone(&hardware), &icon_paths)
                .unwrap();
        assert!(controller.state_for_index(0).active);
        assert!(controller.on_button_pressed(2).unwrap());
        assert!(controller.state_for_index(1).active);
        let updates = hardware.updates();
        assert!(!updates.is_empty());
        assert_eq!(updates.last().unwrap().0, 2);
    }

    #[test]
    fn selects_individual_buttons() {
        let config = multi_button_config();
        let backend = FakeBackend {
            sinks: vec![
                SinkInfo {
                    id: Some(1),
                    name: "sink_monitor".into(),
                    description: Some("Monitor".into()),
                },
                SinkInfo {
                    id: Some(2),
                    name: "sink_headset".into(),
                    description: Some("Headset".into()),
                },
                SinkInfo {
                    id: Some(3),
                    name: "sink_earbuds".into(),
                    description: Some("Earbuds".into()),
                },
            ],
            current: std::sync::Mutex::new(Some(SinkInfo {
                id: Some(1),
                name: "sink_monitor".into(),
                description: Some("Monitor".into()),
            })),
            ..Default::default()
        };

        let hardware = Arc::new(RecordingHardware::new());
        let icon_paths = IconPaths::new(None);
        let mut controller =
            AudioToggleController::new(config, backend, Arc::clone(&hardware), &icon_paths)
                .unwrap();

        assert!(controller.state_for_index(0).active);
        assert!(controller.state_for_index(1).available);
        assert!(controller.state_for_index(2).available);

        assert!(controller.on_button_pressed(2).unwrap());
        assert!(controller.state_for_index(2).active);

        assert!(controller.on_button_pressed(1).unwrap());
        assert!(controller.state_for_index(1).active);
    }

    #[test]
    fn material_icons_are_tinted() {
        let icon_paths = IconPaths::new(None);
        let icon = load_material_icon(MaterialIcon::Monitor, &icon_paths).unwrap();
        assert_eq!(icon.tint, Some(MATERIAL_ICON_TINT));
    }
}
#[derive(Clone, Debug)]
struct IconPaths {
    base_dir: Option<PathBuf>,
    assets_root: Option<PathBuf>,
}

impl IconPaths {
    fn new(config_path: Option<&Path>) -> Self {
        let env_assets = env::var_os("STREAMDECK_CTRL_ASSETS").map(PathBuf::from);
        let base_dir = config_path
            .and_then(|path| path.parent())
            .map(|parent| parent.to_path_buf());
        let assets_root = env_assets.or_else(|| base_dir.as_ref().map(|dir| dir.join("assets")));
        Self {
            base_dir,
            assets_root,
        }
    }
}
