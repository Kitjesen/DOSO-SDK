use doso_sdk::{Config, DosoClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() == 1 && matches!(arguments[0].as_str(), "--help" | "-h") {
        println!("Read a saved map grid: read_grid <http(s)://robot:5052> <map-name>");
        return Ok(());
    }
    if arguments.len() != 2 {
        return Err("Expected a gRPC endpoint and map name; use --help".into());
    }
    let mut config = Config::new(&arguments[0]);
    config.api_key = std::env::var("DOSO_API_KEY").ok();
    let robot = DosoClient::with_config(config).await?;
    let grid = robot.maps().grid(&arguments[1]).await?;
    println!(
        "map={} epoch={:?} frame={} size={}x{} resolution={}m unknown={}",
        grid.map_id,
        grid.content_epoch,
        grid.frame_id,
        grid.width,
        grid.height,
        grid.resolution_m,
        grid.cells.iter().filter(|cell| **cell == 255).count()
    );
    Ok(())
}
