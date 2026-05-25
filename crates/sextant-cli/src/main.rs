use color_eyre::Result;
use std::fs::OpenOptions;

fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    color_eyre::install()?;

    let log_dir = dirs::config_dir()
        .map(|p| p.join("sextant"))
        .expect("unable to determine config directory");
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("sextant.log");

    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    let (non_blocking, _guard) = tracing_appender::non_blocking(log_file);

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(non_blocking)
        .init();

    tracing::info!("starting sextant; log file: {}", log_path.display());

    sextant_ui::run()?;
    Ok(())
}
