use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use tauri::ipc::Channel;
use rayon::prelude::*;

use crate::models::ThumbnailProgress;
use crate::raf::preview;

const THUMBNAIL_MAX_WIDTH: u32 = 600;

#[tauri::command]
pub async fn generate_thumbnails(
    _dcim_path: String,
    image_ids: Vec<String>,
    raf_paths: Vec<String>,
    cache_dir: String,
    on_progress: Channel<ThumbnailProgress>,
) -> Result<Vec<(String, String)>, String> {
    tokio::task::spawn_blocking(move || {
        generate_thumbnails_blocking(_dcim_path, image_ids, raf_paths, cache_dir, on_progress)
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

fn generate_thumbnails_blocking(
    _dcim_path: String,
    image_ids: Vec<String>,
    raf_paths: Vec<String>,
    cache_dir: String,
    on_progress: Channel<ThumbnailProgress>,
) -> Result<Vec<(String, String)>, String> {
    let cache = Path::new(&cache_dir);

    // Create cache directory if it doesn't exist
    fs::create_dir_all(cache)
        .map_err(|e| format!("Failed to create cache dir: {}", e))?;

    let total = image_ids.len() as u32;
    let completed = AtomicU32::new(0);

    let results: Vec<(String, String)> = image_ids
        .par_iter()
        .zip(raf_paths.par_iter())
        .filter_map(|(image_id, raf_path)| {
            // `.v2.` suffix marks thumbnails generated with EXIF-orientation
            // applied. Older un-suffixed `.webp`/`.jpg` files on disk had
            // portrait shots baked in sideways — we ignore them and let this
            // run regenerate correct ones.
            let webp_path = cache.join(format!("{}.v2.webp", image_id));
            let jpg_path = cache.join(format!("{}.v2.jpg", image_id));

            let thumb_path = if webp_path.exists() {
                webp_path
            } else if jpg_path.exists() {
                jpg_path
            } else {
                // Generate new WebP thumbnail from RAF
                let raf = Path::new(raf_path);
                match preview::extract_thumbnail(raf, THUMBNAIL_MAX_WIDTH) {
                    Ok(webp_bytes) => {
                        if let Err(e) = fs::write(&webp_path, &webp_bytes) {
                            log::error!("Failed to write thumbnail for {}: {}", image_id, e);
                            let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                            let _ = on_progress.send(ThumbnailProgress {
                                image_id: image_id.clone(),
                                thumbnail_path: String::new(),
                                completed: done,
                                total,
                            });
                            return None;
                        }
                        webp_path
                    }
                    Err(e) => {
                        log::error!("Failed to extract thumbnail for {}: {}", image_id, e);
                        let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                        let _ = on_progress.send(ThumbnailProgress {
                            image_id: image_id.clone(),
                            thumbnail_path: String::new(),
                            completed: done,
                            total,
                        });
                        return None;
                    }
                }
            };

            let thumb_str = thumb_path.to_string_lossy().to_string();
            let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
            let _ = on_progress.send(ThumbnailProgress {
                image_id: image_id.clone(),
                thumbnail_path: thumb_str.clone(),
                completed: done,
                total,
            });

            Some((image_id.clone(), thumb_str))
        })
        .collect();

    Ok(results)
}

/// Get a single thumbnail path, generating it if needed.
#[tauri::command]
pub async fn get_thumbnail(
    image_id: String,
    raf_path: String,
    cache_dir: String,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let cache = Path::new(&cache_dir);
        fs::create_dir_all(cache)
            .map_err(|e| format!("Failed to create cache dir: {}", e))?;

        // See orientation note in generate_thumbnails for the `.v2.` suffix.
        let webp_path = cache.join(format!("{}.v2.webp", image_id));
        let jpg_path = cache.join(format!("{}.v2.jpg", image_id));

        if webp_path.exists() {
            return Ok(webp_path.to_string_lossy().to_string());
        }
        if jpg_path.exists() {
            return Ok(jpg_path.to_string_lossy().to_string());
        }

        let raf = Path::new(&raf_path);
        let webp_bytes = preview::extract_thumbnail(raf, THUMBNAIL_MAX_WIDTH)?;

        fs::write(&webp_path, &webp_bytes)
            .map_err(|e| format!("Failed to write thumbnail: {}", e))?;

        Ok(webp_path.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}
