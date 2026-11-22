use doso_sdk::internal::motion_board::{
    Certificate, ClientTlsConfig, Identity, MotionBoardClient, MotionBoardConfig,
};
use std::{env, fs, path::PathBuf};

pub async fn connect(endpoint: &str) -> Result<MotionBoardClient, Box<dyn std::error::Error>> {
    let mut config = MotionBoardConfig::new(endpoint);
    if endpoint.starts_with("https://") {
        if let Some(directory) = env::var_os("DOSO_BRAINSTEM_CERT_DIR") {
            let directory = PathBuf::from(directory);
            let ca = fs::read(directory.join("ca.cert.pem"))?;
            let certificate = fs::read(directory.join("client.cert.pem"))?;
            let key = fs::read(directory.join("client.key.pem"))?;
            config = config.with_tls(
                ClientTlsConfig::new()
                    .ca_certificate(Certificate::from_pem(ca))
                    .identity(Identity::from_pem(certificate, key)),
            );
        }
    }
    Ok(MotionBoardClient::with_config(config).await?)
}
