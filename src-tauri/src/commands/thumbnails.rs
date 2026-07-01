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
            // `thumb_file_name` yields `{id}.v3.jpg`. The `.v3` generation marks
            // lossy-JPEG thumbnails with EXIF orientation baked into the pixels;
            // older `.v2.*` (and un-suffixed) files are ignored and regenerated.
            let thumb_path = cache.join(thumb_file_name(image_id));

            if !thumb_path.exists() {
                // Generate a new JPEG thumbnail from the RAF preview.
                let raf = Path::new(raf_path);
                match preview::extract_thumbnail(raf, THUMBNAIL_MAX_WIDTH) {
                    Ok(jpeg_bytes) => {
                        if let Err(e) = fs::write(&thumb_path, &jpeg_bytes) {
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
            }

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

        // See the `.v3.jpg` cache-key note in generate_thumbnails.
        let thumb_path = cache.join(thumb_file_name(&image_id));

        if thumb_path.exists() {
            return Ok(thumb_path.to_string_lossy().to_string());
        }

        let raf = Path::new(&raf_path);
        let jpeg_bytes = preview::extract_thumbnail(raf, THUMBNAIL_MAX_WIDTH)?;

        fs::write(&thumb_path, &jpeg_bytes)
            .map_err(|e| format!("Failed to write thumbnail: {}", e))?;

        Ok(thumb_path.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// Cache file name for a thumbnail: `{id}.v3.jpg`.
///
/// The `.v3` generation marks lossy-JPEG thumbnails with EXIF orientation baked
/// into the pixels. Both the RAF pipeline (here) and the sips/HEIF library
/// pipeline write this exact name, so a single existence check per id decides
/// hit vs. miss. Bumping the key invalidates the older `.v2` WebP/JPEG files.
pub(crate) fn thumb_file_name(id: &str) -> String {
    format!("{}.v3.jpg", id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_file_name_uses_v3_jpg_key() {
        assert_eq!(thumb_file_name("DSCF1234"), "DSCF1234.v3.jpg");
        assert_eq!(thumb_file_name("100_0001"), "100_0001.v3.jpg");
    }
}
