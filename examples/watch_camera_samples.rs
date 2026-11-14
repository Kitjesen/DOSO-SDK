use doso_sdk::DosoClient;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        println!("Watch JPEG samples: watch_camera_samples <http(s)://robot:5052> [count]");
        return Ok(());
    }
    if !(1..=2).contains(&args.len()) {
        return Err("Expected endpoint and optional frame count; use --help".into());
    }
    let count = args
        .get(1)
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(10);
    if count == 0 {
        return Err("Frame count must be greater than zero".into());
    }

    let token = std::env::var("DOSO_MEDIA_VIEWER_TOKEN").ok();
    let robot = DosoClient::connect(&args[0]).await?;
    let mut stream = robot
        .observations()
        .watch_camera_samples(token.as_deref())
        .await?;
    let mut fingerprints = HashSet::with_capacity(count);
    for index in 0..count {
        let sample = tokio::time::timeout(Duration::from_secs(10), stream.message())
            .await
            .map_err(|_| format!("camera stream stalled before sample {}", index + 1))??
            .ok_or("camera stream ended before requested sample count")?;
        let jpeg = &sample
            .frame
            .as_ref()
            .ok_or("camera sample has no frame")?
            .jpeg;
        if jpeg.len() < 4 || jpeg[0..2] != [0xff, 0xd8] || jpeg[jpeg.len() - 2..] != [0xff, 0xd9] {
            return Err(format!("sample {} is not a complete JPEG", index + 1).into());
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        jpeg.hash(&mut hasher);
        fingerprints.insert(hasher.finish());
        println!(
            "sample={} sequence={:?} bytes={} encoding={} unique={}",
            index + 1,
            sample.sequence,
            jpeg.len(),
            sample
                .info
                .as_ref()
                .map(|info| info.encoding.as_str())
                .unwrap_or(""),
            fingerprints.len()
        );
    }
    println!("received={} unique={}", count, fingerprints.len());
    Ok(())
}
