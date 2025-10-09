mod app;
mod controls;
mod hardware;
mod system;
mod util;

#[cfg(feature = "hardware")]
use anyhow::Result;

#[cfg(feature = "hardware")]
fn main() -> Result<()> {
    init_tracing();

    let config = app::AppConfig::default();
    let mut app = app::App::new(config)?;
    app.run()
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
