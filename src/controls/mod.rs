mod brightness;
mod timer;
mod volume;

pub use brightness::BrightnessController;
pub use timer::TimerController;
pub use volume::VolumeController;

use anyhow::Result;

pub trait EncoderController: Send {
    fn on_turn(&mut self, delta: i32) -> Result<()>;
    fn on_press(&mut self) -> Result<()>;
    fn on_release(&mut self) -> Result<()>;
}

pub trait Tickable: Send {
    fn on_tick(&mut self) -> Result<()>;
}
