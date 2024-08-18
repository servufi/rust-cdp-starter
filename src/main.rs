mod cdp;
use anyhow::Result;
use cdp::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logger
    env_logger::Builder::from_default_env()
        .filter(None, log::LevelFilter::Debug) // Global
        .filter_module("reqwest", log::LevelFilter::Info) // Set reqwest to info level
        .filter_module("tungstenite", log::LevelFilter::Info) // Set tungstenite to info level
        .format_timestamp_millis()
        .init();

    // Start here, GL!

    // Keep running
    loop {
        sleep(Duration::from_secs(30)).await;
    }
}
