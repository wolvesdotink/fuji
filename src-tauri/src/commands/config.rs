use std::fs;
use std::path::Path;

use crate::models::AppConfig;

#[tauri::command]
pub fn load_config(config_path: String) -> Result<AppConfig, String> {
    let path = Path::new(&config_path);

    if !path.exists() {
        return Ok(AppConfig {
            destination_path: None,
        });
    }

    let contents = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read config: {}", e))?;

    serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse config: {}", e))
}

#[tauri::command]
pub fn save_config(config_path: String, config: AppConfig) -> Result<(), String> {
    let path = Path::new(&config_path);

    // Create parent directory if needed
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }

    let json = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;

    fs::write(path, json)
        .map_err(|e| format!("Failed to write config: {}", e))
}
