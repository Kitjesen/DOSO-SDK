use doso_sdk::{Config, DosoClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() == 1 && matches!(arguments[0].as_str(), "--help" | "-h") {
        println!(
            "Read-only motor voltage, IMU and joints: read_observations <http(s)://robot:5052>"
        );
        return Ok(());
    }
    if arguments.len() != 1 {
        return Err("Expected one gRPC endpoint; use --help".into());
    }
    let mut config = Config::new(&arguments[0]);
    config.api_key = std::env::var("DOSO_API_KEY").ok();
    let robot = DosoClient::with_config(config).await?;
    let observations = robot.observations();
    let voltages = observations.motor_voltages().await?;
    println!("motor_bus_voltage_v={:?}", voltages.volts);
    let (mut imu, mut joints) =
        tokio::try_join!(observations.watch_imu(), observations.watch_joints())?;

    let read_imu = async {
        for _ in 0..100 {
            let Some(sample) = imu.message().await? else {
                break;
            };
            println!("IMU: {sample:?}");
        }
        Ok::<(), tonic::Status>(())
    };
    let read_joints = async {
        for _ in 0..100 {
            let Some(sample) = joints.message().await? else {
                break;
            };
            println!("joints: {sample:?}");
        }
        Ok::<(), tonic::Status>(())
    };
    tokio::try_join!(read_imu, read_joints)?;
    Ok(())
}
