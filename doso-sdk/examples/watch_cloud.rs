use doso_sdk::{CloudKind, DosoClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        println!("Watch point clouds: watch_cloud <http(s)://robot:5052> <scan|map>");
        return Ok(());
    }
    if args.len() != 2 {
        return Err("Expected endpoint and scan or map; use --help".into());
    }
    let kind = match args[1].as_str() {
        "scan" => CloudKind::Scan,
        "map" => CloudKind::Map,
        _ => return Err("Point-cloud kind must be scan or map".into()),
    };
    let robot = DosoClient::connect(&args[0]).await?;
    let mut frames = robot.observations().watch_point_cloud(kind).await?;
    while let Some(frame) = frames.message().await? {
        let cloud = frame.decode()?;
        println!(
            "{:?}: {} points, frame={}, epoch={}, sequence={}, stamp_s={}",
            cloud.stream_kind,
            cloud.points.len(),
            cloud.frame_id,
            cloud.epoch,
            cloud.sequence,
            cloud.stamp_s
        );
    }
    Ok(())
}
