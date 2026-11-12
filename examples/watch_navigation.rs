use doso_sdk::{Config, DosoClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() == 1 && matches!(arguments[0].as_str(), "--help" | "-h") {
        println!("Read-only map poses and routes: watch_navigation <http(s)://robot:5052>");
        return Ok(());
    }
    if arguments.len() != 1 {
        return Err("Expected one gRPC endpoint; use --help".into());
    }
    let mut config = Config::new(&arguments[0]);
    config.api_key = std::env::var("DOSO_API_KEY").ok();
    let robot = DosoClient::with_config(config).await?;
    let localization = robot.localization();
    let navigation = robot.navigation();
    let (mut poses, mut paths) =
        tokio::try_join!(localization.watch_pose(), navigation.watch_path(),)?;
    let read_poses = async {
        for _ in 0..100 {
            let Some(sample) = poses.message().await? else {
                break;
            };
            println!("pose: {sample:?}");
        }
        Ok::<(), tonic::Status>(())
    };
    let read_paths = async {
        for _ in 0..100 {
            let Some(path) = paths.message().await? else {
                break;
            };
            println!(
                "route: {} points, frame={}",
                path.points.len(),
                path.frame_id
            );
        }
        Ok::<(), tonic::Status>(())
    };
    tokio::try_join!(read_poses, read_paths)?;
    Ok(())
}
