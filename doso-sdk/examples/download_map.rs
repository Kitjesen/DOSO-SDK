use doso_sdk::{rpc::map_content::Value, DosoClient};
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        println!(
            "Read a saved PCD map: download_map <http(s)://robot:5052> <map_name> <output.pcd>"
        );
        return Ok(());
    }
    if args.len() != 3 {
        return Err("Expected endpoint, map name and output path; use --help".into());
    }
    let robot = DosoClient::connect(&args[0]).await?;
    let mut stream = robot.maps().point_cloud(&args[1]).await?;
    let info = match stream.message().await?.and_then(|message| message.value) {
        Some(Value::Info(info)) if info.map_id == args[1] => info,
        _ => return Err("Missing or mismatched map metadata".into()),
    };
    let mut file = std::fs::File::create_new(&args[2])?;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let mut received = 0_u64;
        while let Some(message) = stream.message().await? {
            let Some(Value::Chunk(chunk)) = message.value else {
                return Err("Unexpected map stream message".into());
            };
            received = received
                .checked_add(chunk.len() as u64)
                .ok_or("Map size overflow")?;
            if received > info.size_bytes {
                return Err("Map is larger than its declared size".into());
            }
            file.write_all(&chunk)?;
        }
        if received != info.size_bytes {
            return Err("Map ended before its declared size".into());
        }
        file.flush()?;
        Ok(())
    }
    .await;
    drop(file);
    if let Err(error) = result {
        if let Err(cleanup) = std::fs::remove_file(&args[2]) {
            return Err(format!("{error}; failed to remove incomplete file: {cleanup}").into());
        }
        return Err(error);
    }
    println!(
        "Saved {} bytes for {} to {} (frame {:?}, version {:?})",
        info.size_bytes, info.map_id, args[2], info.frame_id, info.content_epoch
    );
    Ok(())
}
