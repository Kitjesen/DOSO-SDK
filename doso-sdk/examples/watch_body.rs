use doso_sdk::{BodyKind, BodyTransition, Config, DosoClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() == 1 && matches!(arguments[0].as_str(), "--help" | "-h") {
        println!("Read-only body state: watch_body <http(s)://robot:5052>");
        return Ok(());
    }
    if arguments.len() != 1 {
        return Err("Expected one gRPC endpoint; use --help".into());
    }
    let mut config = Config::new(&arguments[0]);
    config.api_key = std::env::var("DOSO_API_KEY").ok();
    let robot = DosoClient::with_config(config).await?;
    let mut states = robot.motion().watch_body_state().await?;
    while let Some(state) = states.message().await? {
        println!(
            "body={:?} transition={:?} gesture={:?}",
            BodyKind::try_from(state.kind),
            BodyTransition::try_from(state.transition),
            state.gesture_name
        );
    }
    Ok(())
}
