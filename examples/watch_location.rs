use doso_sdk::{Config, DosoClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() == 1 && matches!(arguments[0].as_str(), "--help" | "-h") {
        println!("Read-only WGS84 location: watch_location <http(s)://robot:5052>");
        return Ok(());
    }
    if arguments.len() != 1 {
        return Err("Expected one gRPC endpoint; use --help".into());
    }
    let mut config = Config::new(&arguments[0]);
    config.api_key = std::env::var("DOSO_API_KEY").ok();
    let robot = DosoClient::with_config(config).await?;
    eprintln!("snapshot={:?}", robot.localization().location().await?);
    let mut locations = robot.localization().watch_location().await?;
    while let Some(location) = locations.message().await? {
        println!(
            "state={} fix={:?} age_s={:?}",
            location.state, location.fix, location.age_s
        );
    }
    Ok(())
}
