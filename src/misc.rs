use anyhow::{Result, anyhow};
use std::path::PathBuf;
use tokio::fs;

pub async fn get_config_path() -> Result<PathBuf> {
    let config_path = dirs::config_dir()
        .ok_or_else(|| anyhow!("Failed to get user's config directory"))?
        .join("ytm");

    fs::create_dir_all(&config_path).await?;

    Ok(config_path)
}
