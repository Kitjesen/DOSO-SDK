#[path = "support/brainstem.rs"]
mod brainstem;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:13145".to_owned());
    let client = brainstem::connect(&endpoint).await?;

    println!("control: {:#?}", client.control_status().await?);
    println!("cms_state: {:#?}", client.cms_state().await?);
    println!("profile: {:#?}", client.profile().await?);
    println!("gestures: {:#?}", client.list_gestures().await?);
    println!("voltage: {:#?}", client.voltage().await?);
    println!("motors: {:#?}", client.motor_status().await?);
    Ok(())
}
