use anyhow::{Result, anyhow};
use std::{fs, path::PathBuf};

pub fn get_config_path() -> Result<PathBuf> {
    let config_path = dirs::config_dir();
    if config_path.is_none() {
        return Err(anyhow!("Failed to get user's config directory"));
    }

    let config_path = config_path.unwrap().join("ytm");
    fs::create_dir_all(&config_path)?;

    Ok(config_path)
}
