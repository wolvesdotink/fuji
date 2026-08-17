use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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

/// Trim the PTP preview cache to `max_bytes`, deleting oldest-first.
/// Returns the number of bytes freed.
///
/// The cache is where the viewer parks files downloaded from the camera during
/// culling, including whole movies, so without a cap it grows without bound.
#[tauri::command]
pub async fn prune_preview_cache(cache_dir: String, max_bytes: u64) -> Result<u64, String> {
    tokio::task::spawn_blocking(move || prune_preview_cache_blocking(&cache_dir, max_bytes))
        .await
        .map_err(|e| format!("Task join error: {}", e))?
}

fn prune_preview_cache_blocking(cache_dir: &str, max_bytes: u64) -> Result<u64, String> {
    let dir = Path::new(cache_dir);
    // Nothing cached yet — the directory is created lazily on first download.
    if !dir.is_dir() {
        return Ok(0);
    }

    let entries = fs::read_dir(dir)
        .map_err(|e| format!("Failed to read preview cache {}: {}", cache_dir, e))?;

    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    let mut total: u64 = 0;

    for entry in entries.flatten() {
        let metadata = match entry.metadata() {
            Ok(m) if m.is_file() => m,
            _ => continue,
        };
        // Filesystems without mtime support sort as "oldest", which is the safe
        // side of the trade-off: those entries get evicted first.
        let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        total += metadata.len();
        files.push((mtime, metadata.len(), entry.path()));
    }

    if total <= max_bytes {
        return Ok(0);
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut freed: u64 = 0;
    for (_, size, path) in files {
        if total <= max_bytes {
            break;
        }
        match fs::remove_file(&path) {
            Ok(()) => {
                total -= size;
                freed += size;
            }
            Err(e) => log::warn!("Failed to prune {}: {}", path.display(), e),
        }
    }

    log::info!(
        "Pruned {} bytes from the preview cache (cap {} bytes)",
        freed,
        max_bytes
    );

    Ok(freed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fuji_{}_{}", tag, nanos));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(dir: &Path, name: &str, len: usize) {
        let mut f = fs::File::create(dir.join(name)).unwrap();
        f.write_all(&vec![b'x'; len]).unwrap();
    }

    // The frontend prunes on startup, before anything has been cached.
    #[test]
    fn prune_tolerates_a_missing_cache_dir() {
        let missing = unique_temp_dir("prune_missing").join("not-created");
        assert_eq!(
            prune_preview_cache_blocking(missing.to_str().unwrap(), 10).unwrap(),
            0
        );
        fs::remove_dir_all(missing.parent().unwrap()).ok();
    }

    #[test]
    fn prune_keeps_a_cache_under_the_cap_untouched() {
        let dir = unique_temp_dir("prune_under");
        write_file(&dir, "a.HIF", 100);
        write_file(&dir, "b.HIF", 100);

        assert_eq!(prune_preview_cache_blocking(dir.to_str().unwrap(), 200).unwrap(), 0);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);

        fs::remove_dir_all(&dir).ok();
    }

    // Deletion must stop as soon as the cache fits, so the caller can call this
    // on every download without it emptying the cache.
    #[test]
    fn prune_frees_only_what_the_cap_requires() {
        let dir = unique_temp_dir("prune_over");
        write_file(&dir, "a.HIF", 100);
        write_file(&dir, "b.HIF", 100);
        write_file(&dir, "c.HIF", 100);

        let freed = prune_preview_cache_blocking(dir.to_str().unwrap(), 150).unwrap();
        assert_eq!(freed, 200);

        let remaining: u64 = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.metadata().unwrap().len())
            .sum();
        assert_eq!(remaining, 100);

        fs::remove_dir_all(&dir).ok();
    }
}
