use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use tauri::ipc::Channel;
use rayon::prelude::*;

use crate::models::ThumbnailProgress;
use crate::raf::preview;

const THUMBNAIL_MAX_WIDTH: u32 = 600;

pub fn is_video_path(path: &str) -> bool {
    matches!(
        Path::new(path)
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_uppercase()
            .as_str(),
        "MOV" | "MP4" | "M4V" | "AVI"
    )
}

/// Generate a poster frame without decoding the movie in the webview. macOS
/// Quick Look understands the codecs produced by Fuji cameras; `sips` then
/// converts its PNG output to the same compact JPEG format as image thumbs.
pub fn generate_video_thumbnail(
    video_path: &str,
    thumb_path: &Path,
    max_width: u32,
) -> Result<(), String> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp_dir = thumb_path.with_extension(format!("qlthumb-{}-{}", std::process::id(), nonce));
    fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("Failed to create Quick Look temp directory: {}", e))?;

    let result = (|| {
        let max_width_arg = max_width.to_string();
        let output = Command::new("qlmanage")
            .args(["-t", "-s", max_width_arg.as_str(), "-o"])
            .arg(&temp_dir)
            .arg(video_path)
            .output()
            .map_err(|e| format!("Failed to run qlmanage: {}", e))?;
        if !output.status.success() {
            return Err(format!(
                "Quick Look failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        let quicklook_png = fs::read_dir(&temp_dir)
            .map_err(|e| format!("Failed to read Quick Look output: {}", e))?
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.extension()
                    .map(|ext| ext.eq_ignore_ascii_case("png"))
                    .unwrap_or(false)
            })
            .ok_or_else(|| "Quick Look did not produce a video thumbnail".to_string())?;

        let output = Command::new("sips")
            .args(["-s", "format", "jpeg", "-s", "formatOptions", "80"])
            .arg(&quicklook_png)
            .arg("--out")
            .arg(thumb_path)
            .output()
            .map_err(|e| format!("Failed to convert video thumbnail: {}", e))?;
        if !output.status.success() {
            return Err(format!(
                "sips failed for video thumbnail: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(())
    })();

    let _ = fs::remove_dir_all(&temp_dir);
    result
}

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

    // Bound movie/RAW decode concurrency so opening a large card does not
    // spawn one Quick Look process per CPU and starve the webview.
    let workers = std::thread::available_parallelism()
        .map(|n| n.get().min(4))
        .unwrap_or(2);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .map_err(|e| format!("Failed to create thumbnail worker pool: {}", e))?;

    let results: Vec<(String, String)> = pool.install(|| {
        image_ids
        .par_iter()
        .zip(raf_paths.par_iter())
        .filter_map(|(image_id, raf_path)| {
            // `thumb_file_name` yields `{id}.v3.jpg`. The `.v3` generation marks
            // lossy-JPEG thumbnails with EXIF orientation baked into the pixels;
            // older `.v2.*` (and un-suffixed) files are ignored and regenerated.
            let thumb_path = cache.join(thumb_file_name(image_id));

            if !thumb_path.exists() {
                let generated = if is_video_path(raf_path) {
                    generate_video_thumbnail(raf_path, &thumb_path, THUMBNAIL_MAX_WIDTH)
                } else {
                    preview::extract_thumbnail(Path::new(raf_path), THUMBNAIL_MAX_WIDTH)
                        .and_then(|jpeg_bytes| {
                            fs::write(&thumb_path, &jpeg_bytes)
                                .map_err(|e| format!("Failed to write thumbnail: {}", e))
                        })
                };

                if let Err(e) = generated {
                    log::error!("Failed to generate thumbnail for {}: {}", image_id, e);
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
        .collect()
    });

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

    #[test]
    fn recognizes_supported_video_extensions_case_insensitively() {
        assert!(is_video_path("/card/DCIM/DSCF0001.MOV"));
        assert!(is_video_path("/library/clip.mp4"));
        assert!(is_video_path("movie.m4v"));
        assert!(!is_video_path("DSCF0001.HIF"));
    }
}
