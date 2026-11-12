use doso_sdk::DosoClient;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        println!("Read a JPEG snapshot: read_camera <http(s)://robot:5052> <output.jpg>");
        return Ok(());
    }
    if args.len() != 2 {
        return Err("Expected endpoint and output path; use --help".into());
    }
    let token = std::env::var("DOSO_MEDIA_VIEWER_TOKEN").ok();
    let robot = DosoClient::connect(&args[0]).await?;
    let frame = robot.observations().camera_frame(token.as_deref()).await?;
    let mut file = std::fs::File::create_new(&args[1])?;
    let result = file.write_all(&frame.jpeg);
    drop(file);
    if let Err(error) = result {
        if let Err(cleanup) = std::fs::remove_file(&args[1]) {
            return Err(format!("{error}; failed to remove incomplete file: {cleanup}").into());
        }
        return Err(error.into());
    }
    println!("Saved {} bytes to {}", frame.jpeg.len(), args[1]);
    Ok(())
}
