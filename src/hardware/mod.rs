#[cfg_attr(not(feature = "hardware"), path = "stub.rs")]
mod backend;
#[cfg(feature = "hardware")]
mod render;

pub use backend::{
    ButtonImage, DisplayPipeline, EncoderDisplay, EncoderId, HardwareConfig, HardwareEvent,
    HardwareHandle, start,
};
