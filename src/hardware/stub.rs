#![allow(dead_code)]

use std::sync::Arc;

use anyhow::{Result, anyhow};
use crossbeam_channel::Receiver;
use image::RgbaImage;

#[derive(Clone, Debug)]
pub struct HardwareConfig {
    pub serial: Option<String>,
    pub device_brightness: u8,
}

impl Default for HardwareConfig {
    fn default() -> Self {
        Self {
            serial: None,
            device_brightness: 40,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EncoderId {
    One,
    Two,
    Three,
    Four,
}

#[derive(Debug, Clone)]
pub struct EncoderDisplay {
    pub title: String,
    pub value: String,
    pub status: Option<String>,
    pub progress: Option<f32>,
    pub progress_color: Option<[u8; 3]>,
    pub value_color: Option<[u8; 3]>,
}

impl EncoderDisplay {
    pub fn new(title: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            value: value.into(),
            status: None,
            progress: None,
            progress_color: None,
            value_color: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ButtonImage {
    pub id: String,
    pub image: Arc<RgbaImage>,
    pub tint: Option<[u8; 3]>,
}

#[derive(Debug)]
pub enum HardwareEvent {
    EncoderTurned { encoder: EncoderId, delta: i32 },
    EncoderPressed { encoder: EncoderId },
    EncoderReleased { encoder: EncoderId },
    ButtonPressed(u8),
    ButtonReleased(u8),
    Touch,
}

pub trait DisplayPipeline: Send + Sync {
    fn update_encoder(&self, _encoder: EncoderId, _display: EncoderDisplay) -> Result<()> {
        Ok(())
    }

    fn update_button_icon(&self, _index: u8, _icon: Option<ButtonImage>) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct HardwareHandle;

impl DisplayPipeline for HardwareHandle {}

pub fn start(_: HardwareConfig) -> Result<(HardwareHandle, Receiver<HardwareEvent>)> {
    Err(anyhow!(
        "hardware support disabled. Enable the `hardware` feature to connect to the Stream Deck."
    ))
}
