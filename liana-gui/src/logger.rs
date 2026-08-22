use crate::dir::LianaDirectory;
use std::{error::Error, str::FromStr};

const GUI_LOG_FILE_NAME: &str = "liana-gui.log";

/// Log targets that are noisy enough to bury our own records. Matched by prefix.
const MUTED_TARGETS: [&str; 17] = [
    "iced_core",
    "iced_glow",
    "iced_glutin",
    "iced_graphics",
    "iced_runtime",
    "iced_winit",
    "glow_glyph",
    "winit",
    "mio",
    "ledger_transport_hid",
    "cosmic_text",
    "polling",
    "calloop",
    "async_io",
    "hyper",
    "minreq",
    "tungstenite",
];

pub fn setup_logger(
    log_level: log::LevelFilter,
    datadir: LianaDirectory,
) -> Result<(), Box<dyn Error>> {
    let mut log_path = datadir.path().to_path_buf();
    log_path.push(GUI_LOG_FILE_NAME);

    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{} {}] {}",
                record.level(),
                record.target(),
                message
            ))
        })
        .level(log_level)
        .filter(|metadata| {
            !MUTED_TARGETS
                .iter()
                .any(|target| metadata.target().starts_with(target))
        })
        .chain(std::io::stdout())
        .chain(fern::log_file(log_path)?)
        .apply()?;

    Ok(())
}

/// Parse LOG_LEVEL environment variable.
pub fn parse_log_level() -> Result<Option<log::LevelFilter>, Box<dyn Error>> {
    if let Ok(l) = std::env::var("LOG_LEVEL") {
        Ok(Some(log::LevelFilter::from_str(&l)?))
    } else {
        Ok(None)
    }
}
