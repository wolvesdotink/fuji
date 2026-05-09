use std::fs;
use std::path::Path;

use crate::models::{ImportSelection, SelectionChoice};

/// Get the list of file paths that should be deleted from the camera.
#[tauri::command]
pub fn get_files_to_delete(selections: Vec<ImportSelection>) -> Vec<String> {
    let mut paths = Vec::new();

    for selection in &selections {
        match selection.choice {
            SelectionChoice::Skip => continue,
            SelectionChoice::HeifOnly => {
                paths.push(selection.hif_path.clone());
                // Also delete the RAF since user doesn't want it
                if let Some(ref raf_path) = selection.raf_path {
                    paths.push(raf_path.clone());
                }
            }
            SelectionChoice::HeifAndRaw => {
                paths.push(selection.hif_path.clone());
                if let Some(ref raf_path) = selection.raf_path {
                    paths.push(raf_path.clone());
                }
            }
        }
    }

    paths
}

/// Delete specified files from the camera.
/// Returns the number of successfully deleted files.
#[tauri::command]
pub async fn delete_from_camera(file_paths: Vec<String>) -> Result<u32, String> {
    tokio::task::spawn_blocking(move || {
        let mut deleted = 0u32;
        let mut errors = Vec::new();

        for path_str in &file_paths {
            let path = Path::new(path_str);
            if !path.exists() {
                // File already gone, count it as success
                deleted += 1;
                continue;
            }

            match fs::remove_file(path) {
                Ok(()) => {
                    deleted += 1;
                    log::info!("Deleted: {}", path_str);
                }
                Err(e) => {
                    let msg = format!("Failed to delete {}: {}", path_str, e);
                    log::error!("{}", msg);
                    errors.push(msg);
                }
            }
        }

        if !errors.is_empty() && deleted == 0 {
            return Err(format!(
                "Failed to delete any files. Errors:\n{}",
                errors.join("\n")
            ));
        }

        if !errors.is_empty() {
            log::warn!(
                "Deleted {} of {} files. {} errors occurred.",
                deleted,
                file_paths.len(),
                errors.len()
            );
        }

        Ok(deleted)
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}
