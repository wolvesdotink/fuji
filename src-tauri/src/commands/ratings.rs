use std::collections::HashMap;

use crate::metadata;

#[tauri::command]
pub async fn read_file_ratings(file_paths: Vec<String>) -> Result<HashMap<String, u8>, String> {
    tokio::task::spawn_blocking(move || metadata::read_ratings(&file_paths))
        .await
        .map_err(|e| format!("Join error: {}", e))?
}

#[tauri::command]
pub async fn write_file_rating(file_path: String, rating: u8) -> Result<(), String> {
    tokio::task::spawn_blocking(move || metadata::write_rating(&file_path, rating))
        .await
        .map_err(|e| format!("Join error: {}", e))?
}
