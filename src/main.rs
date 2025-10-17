mod app;
mod controls;
mod hardware;
mod system;
mod util;

#[cfg(feature = "hardware")]
use anyhow::Result;
#[cfg(feature = "hardware")]
use crossbeam_channel;
#[cfg(feature = "hardware")]
use signal_hook::consts::TERM_SIGNALS;
#[cfg(feature = "hardware")]
use signal_hook::iterator::Signals;
#[cfg(feature = "hardware")]
use std::thread;
#[cfg(feature = "hardware")]
use tracing::warn;

#[cfg(feature = "hardware")]
fn main() -> Result<()> {
    init_tracing();

    let config = app::AppConfig::default();
    let mut app = app::App::new(config)?;
    let hardware = app.hardware_handle();

    let (shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(1);
    let signals = Signals::new(TERM_SIGNALS)?;
    let signal_handle = signals.handle();
    let signal_thread = thread::spawn({
        let mut signals = signals;
        let hardware = hardware.clone();
        move || {
            for signal in signals.forever() {
                warn!(signal = signal, "termination signal received");
                let _ = hardware.clear_all_displays();
                let _ = shutdown_tx.send(());
                break;
            }
        }
    });

    app.set_shutdown_channel(shutdown_rx);
    let result = app.run();
    signal_handle.close();
    let _ = signal_thread.join();
    result
}

#[cfg(not(feature = "hardware"))]
fn main() {
    init_tracing();
    eprintln!(
        "streamdeck_ctrl was built without the `hardware` feature. Enable it to control a Stream Deck Plus."
    );
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_level(true)
        .compact()
        .try_init();
}
