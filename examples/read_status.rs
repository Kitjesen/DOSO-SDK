use doso_sdk::{Config, DosoClient};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() == 1 && matches!(arguments[0].as_str(), "--help" | "-h") {
        println!("Read-only DOSO status: read_status <http(s)://robot:5052>");
        return Ok(());
    }
    if arguments.len() != 1 {
        return Err("Expected one gRPC endpoint; use --help".into());
    }
    let mut config = Config::new(&arguments[0]);
    config.api_key = std::env::var("DOSO_API_KEY").ok();
    let robot = DosoClient::with_config(config).await?;
    let info = robot.system().info();
    println!("firmware_version: {:?}", info.firmware_version);
    println!("robot_version: {:?}", info.robot_version);
    println!("robot_id: {:?}", info.robot_id);
    println!("ping: {:?}", robot.system().ping().await?);
    println!("readiness: {:?}", robot.system().readiness_state().await?);
    let product = robot.system().product_status().await?;
    println!(
        "product: {:?}, status: {}, available: {:?}",
        product.product, product.status, product.available
    );
    println!("capabilities: {:?}", robot.system().capabilities().await?);
    println!("maps: {:?}", robot.maps().list().await?);
    println!("localization: {:?}", robot.localization().status().await?);
    Ok(())
}
