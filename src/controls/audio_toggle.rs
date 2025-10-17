use std::collections::HashMap;
use std::convert::TryInto;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use image::{ImageReader, RgbaImage};
use once_cell::sync::Lazy;
use serde::Deserialize;
use tracing::{info, warn};

use resvg::render as render_svg_tree;
use tiny_skia::{Pixmap, Transform};
use usvg::{Options as UsvgOptions, Tree as UsvgTree};

use crate::hardware::{ButtonImage, DisplayPipeline};
use crate::system::audio_switch::{AudioSwitchBackend, PulseAudioSwitch, SinkInfo, SinkSelector};

const DEFAULT_CONFIG_PATHS: &[&str] = &[
    "audio_toggle.json",
    "config/audio_toggle.json",
    "target/debug/audio_toggle.json",
    "target/release/audio_toggle.json",
];
const MATERIAL_MONITOR_ICON: &str = "assets/icons/material/monitor.svg";
const MATERIAL_HEADPHONES_ICON: &str = "assets/icons/material/headphones.svg";
const MATERIAL_ICON_TINT: [u8; 3] = [220, 235, 255];

#[derive(Debug, Clone, Deserialize)]
pub struct AudioToggleConfig {
    #[serde(default = "default_button_index")]
    pub button_index: u8,
    pub outputs: [AudioOutputConfig; 2],
}

#[derive(Debug, Clone, Deserialize)]
pub struct AudioOutputConfig {
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
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaterialIcon {
    Monitor,
    Headphones,
}

fn default_button_index() -> u8 {
    0
}

impl AudioToggleConfig {
    pub fn load_default() -> Result<Option<Self>> {
        for candidate in DEFAULT_CONFIG_PATHS {
            let path = Path::new(candidate);
            if path.exists() {
                return Self::from_path(path).map(Some);
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
    button_index: u8,
    outputs: [OutputProfile; 2],
    active_index: usize,
}

#[derive(Debug, Clone)]
struct OutputProfile {
    selector: SinkSelector,
    icon: ButtonImage,
    label: String,
}

impl<B, H> AudioToggleController<B, H>
where
    B: AudioSwitchBackend,
    H: DisplayPipeline,
{
    pub fn new(config: AudioToggleConfig, backend: B, hardware: H) -> Result<Self> {
        let outputs = config
            .outputs
            .iter()
            .enumerate()
            .map(|(index, entry)| OutputProfile::from_config(entry, index))
            .collect::<Result<Vec<_>>>()?;

        let outputs: [OutputProfile; 2] = outputs
            .try_into()
            .map_err(|_| anyhow!("audio toggle requires exactly two outputs in configuration"))?;

        let mut controller = Self {
            backend,
            hardware,
            button_index: config.button_index,
            outputs,
            active_index: 0,
        };

        controller.refresh_state()?;
        Ok(controller)
    }

    pub fn button_index(&self) -> u8 {
        self.button_index
    }

    pub fn on_button_pressed(&mut self, button_index: u8) -> Result<()> {
        if button_index != self.button_index {
            return Ok(());
        }

        let next_index = 1usize.saturating_sub(self.active_index); // toggle between 0 and 1
        let target = &self.outputs[next_index];
        info!(target = %target.label, "switching audio output");
        let sink = self
            .backend
            .set_default_sink(&target.selector)
            .with_context(|| format!("failed to switch audio output to {}", target.label))?;
        self.active_index = self.index_for_sink(&sink).unwrap_or(next_index);
        self.update_button_icon()
    }

    fn refresh_state(&mut self) -> Result<()> {
        match self.backend.current_default_sink() {
            Ok(Some(current)) => {
                if let Some(index) = self.index_for_sink(&current) {
                    self.active_index = index;
                } else {
                    warn!(
                        sink = %current.name,
                        "default sink not present in toggle configuration; using configured primary output"
                    );
                    self.active_index = 0;
                }
            }
            Ok(None) => {
                self.active_index = 0;
            }
            Err(err) => {
                warn!(error = %err, "failed to determine current default sink");
                self.active_index = 0;
            }
        }

        self.update_button_icon()
    }

    fn index_for_sink(&self, sink: &SinkInfo) -> Option<usize> {
        self.outputs
            .iter()
            .position(|profile| profile.selector.matches(sink))
    }

    fn update_button_icon(&self) -> Result<()> {
        if let Some(profile) = self.outputs.get(self.active_index) {
            self.hardware
                .update_button_icon(self.button_index, Some(profile.icon.clone()))
        } else {
            self.hardware.update_button_icon(self.button_index, None)
        }
    }

    #[cfg(test)]
    fn active_index(&self) -> usize {
        self.active_index
    }
}

impl<H> AudioToggleController<PulseAudioSwitch, H>
where
    H: DisplayPipeline,
{
    pub fn with_default_backend(
        config: AudioToggleConfig,
        hardware: H,
    ) -> Result<AudioToggleController<PulseAudioSwitch, H>> {
        AudioToggleController::new(config, PulseAudioSwitch::new(), hardware)
    }
}

impl OutputProfile {
    fn from_config(config: &AudioOutputConfig, index: usize) -> Result<Self> {
        let selector = config.selector()?;
        let fallback_icon = match index {
            0 => MaterialIcon::Monitor,
            _ => MaterialIcon::Headphones,
        };
        let icon = load_icon_from_config(config.icon.as_ref(), fallback_icon)?;
        let label = config.label();
        Ok(Self {
            selector,
            icon,
            label,
        })
    }
}

impl AudioOutputConfig {
    fn selector(&self) -> Result<SinkSelector> {
        if let Some(name) = &self.name {
            return Ok(SinkSelector::by_name(name.clone()));
        }

        if let Some(description) = &self.description {
            return Ok(SinkSelector::by_description(description.clone()));
        }

        bail!("audio toggle output entry must provide `name` or `description`");
    }

    fn label(&self) -> String {
        self.name
            .as_ref()
            .or(self.description.as_ref())
            .cloned()
            .unwrap_or_else(|| "unnamed sink".to_string())
    }
}

static ICON_CACHE: Lazy<Mutex<HashMap<PathBuf, Arc<RgbaImage>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn load_icon_from_config(icon: Option<&IconConfig>, fallback: MaterialIcon) -> Result<ButtonImage> {
    match icon {
        Some(IconConfig::Material { material }) => load_material_icon(*material),
        Some(IconConfig::Path { path }) => load_icon_from_path(Path::new(path), path, None),
        Some(IconConfig::Simple(material)) => load_material_icon(*material),
        None => load_material_icon(fallback),
    }
}

fn load_material_icon(icon: MaterialIcon) -> Result<ButtonImage> {
    let (path, id) = match icon {
        MaterialIcon::Monitor => (MATERIAL_MONITOR_ICON, "monitor"),
        MaterialIcon::Headphones => (MATERIAL_HEADPHONES_ICON, "headphones"),
    };
    load_icon_from_path(Path::new(path), id, Some(MATERIAL_ICON_TINT))
}

fn load_icon_from_path(
    path: &Path,
    id_hint: impl Into<String>,
    tint: Option<[u8; 3]>,
) -> Result<ButtonImage> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("icon not found at {}", path.display()))?;

    if let Some(image) = ICON_CACHE
        .lock()
        .expect("icon cache mutex poisoned")
        .get(&canonical)
        .map(Arc::clone)
    {
        return Ok(ButtonImage {
            id: id_hint.into(),
            image,
            tint,
        });
    }

    let decoded = decode_icon(&canonical)?;
    let image = Arc::new(decoded);
    ICON_CACHE
        .lock()
        .expect("icon cache mutex poisoned")
        .insert(canonical, Arc::clone(&image));

    Ok(ButtonImage {
        id: id_hint.into(),
        image,
        tint,
    })
}

fn decode_icon(path: &Path) -> Result<RgbaImage> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "svg" => render_svg_icon(path),
        "png" | "jpg" | "jpeg" | "bmp" | "gif" | "tiff" | "webp" => load_raster_icon(path),
        _ => load_raster_icon(path),
    }
}

fn load_raster_icon(path: &Path) -> Result<RgbaImage> {
    let reader = ImageReader::open(path)
        .with_context(|| format!("failed to open icon at {}", path.display()))?;
    let image = reader
        .with_guessed_format()
        .context("failed to guess icon image format")?
        .decode()
        .with_context(|| format!("failed to decode icon at {}", path.display()))?;
    Ok(image.to_rgba8())
}

fn render_svg_icon(path: &Path) -> Result<RgbaImage> {
    let data =
        fs::read(path).with_context(|| format!("failed to read svg icon at {}", path.display()))?;

    let mut options = UsvgOptions::default();
    options.resources_dir = path.parent().map(|dir| dir.to_path_buf());
    let tree = UsvgTree::from_data(&data, &options)
        .with_context(|| format!("failed to parse svg icon at {}", path.display()))?;

    let size = tree.size().to_int_size();
    let width = size.width().max(1);
    let height = size.height().max(1);

    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| anyhow!("failed to allocate pixmap for icon {}", path.display()))?;

    {
        let mut pixmap_mut = pixmap.as_mut();
        render_svg_tree(&tree, Transform::identity(), &mut pixmap_mut);
    }

    let mut buffer = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for chunk in pixmap.data().chunks_exact(4) {
        let alpha = chunk[3];
        let (red, green, blue) = if alpha == 0 {
            (0, 0, 0)
        } else {
            (
                unpremultiply_component(chunk[0], alpha),
                unpremultiply_component(chunk[1], alpha),
                unpremultiply_component(chunk[2], alpha),
            )
        };
        buffer.extend_from_slice(&[red, green, blue, alpha]);
    }

    RgbaImage::from_vec(width, height, buffer)
        .ok_or_else(|| anyhow!("failed to build rgba image for icon {}", path.display()))
}

fn unpremultiply_component(component: u8, alpha: u8) -> u8 {
    if alpha == 0 {
        0
    } else {
        let value = (component as u32 * 255 + (alpha as u32 / 2)) / alpha as u32;
        value.min(255) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::hardware::{ButtonImage, EncoderDisplay, EncoderId};
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
    }

    fn sample_config() -> AudioToggleConfig {
        AudioToggleConfig {
            button_index: 2,
            outputs: [
                AudioOutputConfig {
                    name: None,
                    description: Some("HDMI/DisplayPort - HDA NVidia".into()),
                    icon: Some(IconConfig::Material {
                        material: MaterialIcon::Monitor,
                    }),
                },
                AudioOutputConfig {
                    name: None,
                    description: Some("Digital Output - A50".into()),
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
        assert_eq!(config.button_index, 1);
        assert_eq!(config.outputs[0].description.as_deref(), Some("Output A"));
    }

    #[test]
    fn controller_initialises_with_current_sink() {
        let config = sample_config();
        let backend = FakeBackend {
            sinks: vec![
                SinkInfo {
                    name: "sink_a".into(),
                    description: Some("HDMI/DisplayPort - HDA NVidia".into()),
                },
                SinkInfo {
                    name: "sink_b".into(),
                    description: Some("Digital Output - A50".into()),
                },
            ],
            current: std::sync::Mutex::new(Some(SinkInfo {
                name: "sink_b".into(),
                description: Some("Digital Output - A50".into()),
            })),
            ..Default::default()
        };

        let hardware = RecordingHardware::new();
        let controller = AudioToggleController::new(config, backend, Arc::new(hardware)).unwrap();
        assert_eq!(controller.active_index(), 1);
    }

    #[test]
    fn toggles_between_outputs() {
        let config = sample_config();
        let backend = FakeBackend {
            sinks: vec![
                SinkInfo {
                    name: "sink_monitor".into(),
                    description: Some("HDMI/DisplayPort - HDA NVidia".into()),
                },
                SinkInfo {
                    name: "sink_headset".into(),
                    description: Some("Digital Output - A50".into()),
                },
            ],
            current: std::sync::Mutex::new(Some(SinkInfo {
                name: "sink_monitor".into(),
                description: Some("HDMI/DisplayPort - HDA NVidia".into()),
            })),
            ..Default::default()
        };

        let hardware = Arc::new(RecordingHardware::new());
        let mut controller =
            AudioToggleController::new(config, backend, Arc::clone(&hardware)).unwrap();
        controller.on_button_pressed(2).unwrap();
        assert_eq!(controller.active_index(), 1);
        let updates = hardware.updates();
        assert!(!updates.is_empty());
        assert_eq!(updates.last().unwrap().0, 2);
    }
}
