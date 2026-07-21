use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use rayon::prelude::*;
use tauri::ipc::Channel;
use tauri::State;
use walkdir::WalkDir;

use crate::clip;
use crate::clip::engine::ClipEngine;
use crate::commands::thumbnails::{generate_video_thumbnail, is_video_path, thumb_file_name};
use crate::index;
use crate::models::{
    IndexFingerprint, IndexProgress, LibraryImage, MediaType, ModelDownloadProgress, SearchResult,
    ThumbnailProgress,
};

/// List photos and videos from a destination/library directory.
/// Uses a persistent index with fingerprint-based change detection.
/// Recursively walks for supported still-image and movie files.
/// Returns sorted by modification time descending (newest first).
#[tauri::command]
pub async fn list_library_images(
    dir_path: String,
    cache_dir: String,
) -> Result<Vec<LibraryImage>, String> {
    tokio::task::spawn_blocking(move || {
        let dir = Path::new(&dir_path);
        if !dir.exists() {
            return Err(format!("Directory does not exist: {}", dir_path));
        }

        let cache = Path::new(&cache_dir);
        fs::create_dir_all(cache)
            .map_err(|e| format!("Failed to create cache dir: {}", e))?;
        let index_path = cache.join("library-index.json");

        let extensions = &[
            "HIF", "HEIF", "HEIC", "JPG", "JPEG", "MOV", "MP4", "M4V", "AVI",
        ];

        // Fast path: only when a cached index already exists do we pay the cheap
        // fingerprint walk to validate it. On a match we skip the full scan,
        // keeping repeat loads (HIT) at a single directory walk.
        if index_path.exists() {
            if let Ok(fingerprint) = index::compute_fingerprint(dir, extensions, None) {
                if let Some(cached) =
                    index::try_cached::<LibraryImage>(&index_path, &dir_path, &fingerprint)
                {
                    log::info!("Using cached library index ({} images)", cached.len());
                    return Ok(cached);
                }
            }
        }

        // Miss / first run: a single walk yields both the images and the
        // fingerprint we persist for next time.
        let (images, fingerprint) = scan_library_images(dir)?;
        if let Err(e) = index::cache_images(&index_path, &dir_path, &fingerprint, &images) {
            log::warn!("Failed to save library index: {}", e);
        }
        Ok(images)
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// Full filesystem scan for library images.
///
/// Accumulates the index fingerprint during the same walk, so a cache miss
/// costs one directory traversal instead of a separate fingerprint pass.
fn scan_library_images(dir: &Path) -> Result<(Vec<LibraryImage>, IndexFingerprint), String> {
    let mut images: Vec<LibraryImage> = Vec::new();
    let mut fingerprint = index::FingerprintAccumulator::default();

    for entry in WalkDir::new(dir)
        .min_depth(1)
        .into_iter()
        .flatten()
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let ext = path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_uppercase();

        let media_type = match ext.as_str() {
            "HIF" | "HEIF" | "HEIC" | "JPG" | "JPEG" => MediaType::Image,
            "MOV" | "MP4" | "M4V" | "AVI" => MediaType::Video,
            _ => continue,
        };

        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let id = if media_type == MediaType::Video {
            // Movie ids retain the extension to avoid colliding with a still
            // bearing the same camera-generated stem.
            file_name.clone()
        } else {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        };

        let metadata = entry.metadata().ok();
        if let Some(m) = &metadata {
            fingerprint.add(m);
        }
        let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let date_created = metadata
            .as_ref()
            .and_then(|m| m.created().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let date_modified = metadata
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        images.push(LibraryImage {
            id,
            file_path: path.to_string_lossy().to_string(),
            file_name,
            file_size,
            date_created,
            date_modified,
            media_type,
        });
    }

    // Sort by creation time descending (newest first)
    images.sort_by(|a, b| b.date_created.cmp(&a.date_created));

    Ok((images, fingerprint.finish()))
}

/// Generate thumbnails for library images.
/// Single pass: sips decodes HEIF/HIF, resizes to 600px and writes the final
/// lossy-JPEG `{id}.v3.jpg` thumbnail directly (no temp file, no re-encode).
/// sips preserves the EXIF orientation tag, so portrait shots render upright.
/// Uses rayon for parallel processing.
#[tauri::command]
pub async fn generate_library_thumbnails(
    image_paths: Vec<String>,
    image_ids: Vec<String>,
    cache_dir: String,
    on_progress: Channel<ThumbnailProgress>,
) -> Result<Vec<(String, String)>, String> {
    tokio::task::spawn_blocking(move || {
        generate_library_thumbnails_blocking(image_paths, image_ids, cache_dir, on_progress)
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

fn generate_library_thumbnails_blocking(
    image_paths: Vec<String>,
    image_ids: Vec<String>,
    cache_dir: String,
    on_progress: Channel<ThumbnailProgress>,
) -> Result<Vec<(String, String)>, String> {
    let cache = Path::new(&cache_dir);

    fs::create_dir_all(cache)
        .map_err(|e| format!("Failed to create cache dir: {}", e))?;

    let total = image_ids.len() as u32;
    let completed = AtomicU32::new(0);

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
        .zip(image_paths.par_iter())
        .filter_map(|(image_id, image_path)| {
            // Both pipelines write `{id}.v3.jpg` (see thumb_file_name), so a
            // single existence check decides hit vs. miss. `.v2.*` thumbnails
            // from the old two-step WebP path are ignored and regenerated.
            let thumb_path = cache.join(thumb_file_name(image_id));

            if !thumb_path.exists() {
                let sips_result = if is_video_path(image_path) {
                    generate_video_thumbnail(image_path, &thumb_path, 600).map(|_| None)
                } else {
                    // One sips pass: decode HEIF/HIF, resize to 600px and write
                    // the final lossy JPEG (quality 80). Orientation is baked in.
                    Command::new("sips")
                        .args([
                            "-Z",
                            "600",
                            "-s",
                            "format",
                            "jpeg",
                            "-s",
                            "formatOptions",
                            "80",
                            image_path,
                            "--out",
                            &thumb_path.to_string_lossy(),
                        ])
                        .output()
                        .map(Some)
                        .map_err(|e| format!("Failed to run sips: {}", e))
                };

                match sips_result {
                    Ok(None) => {}
                    Ok(Some(result)) if result.status.success() => {}
                    Ok(Some(result)) => {
                        let stderr = String::from_utf8_lossy(&result.stderr);
                        log::error!("sips failed for {}: {}", image_id, stderr);
                        // Remove any partial output sips may have left behind.
                        let _ = fs::remove_file(&thumb_path);
                        let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                        let _ = on_progress.send(ThumbnailProgress {
                            image_id: image_id.clone(),
                            thumbnail_path: String::new(),
                            completed: done,
                            total,
                        });
                        return None;
                    }
                    Err(e) => {
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

// --- CLIP Search Commands ---

/// Ensure CLIP models are downloaded to the cache directory.
#[tauri::command]
pub async fn ensure_clip_models(
    cache_dir: String,
    on_progress: Channel<ModelDownloadProgress>,
) -> Result<(), String> {
    clip::model::ensure_models(&cache_dir, on_progress).await
}

/// Build a CLIP search index from library thumbnails.
/// Generates embeddings for images not already in the index.
#[tauri::command]
pub async fn index_library(
    image_ids: Vec<String>,
    thumb_paths: Vec<String>,
    model_dir: String,
    index_path: String,
    on_progress: Channel<IndexProgress>,
    state: State<'_, Arc<ClipEngine>>,
) -> Result<(), String> {
    let engine = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        engine.index(image_ids, thumb_paths, model_dir, index_path, on_progress)
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// Search the library using a text query.
/// Returns matching images sorted by relevance score.
#[tauri::command]
pub async fn search_library(
    query: String,
    model_dir: String,
    index_path: String,
    state: State<'_, Arc<ClipEngine>>,
) -> Result<Vec<SearchResult>, String> {
    let engine = state.inner().clone();
    tokio::task::spawn_blocking(move || engine.search(query, model_dir, index_path))
        .await
        .map_err(|e| format!("Task join error: {}", e))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fuji_{}_{}", tag, nanos));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(dir: &Path, name: &str, bytes: &[u8]) {
        let mut f = fs::File::create(dir.join(name)).unwrap();
        f.write_all(bytes).unwrap();
    }

    // The fingerprint folded into scan_library_images must be identical to the
    // one compute_fingerprint produces on its own walk — otherwise a fresh scan
    // would cache a fingerprint that never matches on the next load.
    #[test]
    fn scan_fingerprint_matches_compute_fingerprint() {
        let dir = unique_temp_dir("libscan");
        write_file(&dir, "a.HIF", b"aaaa"); // 4 bytes
        write_file(&dir, "b.jpg", b"bbbbbbbb"); // 8, lowercase ext
        write_file(&dir, "c.JPEG", b"cc"); // 2
        write_file(&dir, "ignore.txt", b"nope"); // excluded extension
        let sub = dir.join("sub"); // library walk is unbounded depth
        fs::create_dir_all(&sub).unwrap();
        write_file(&sub, "d.heic", b"dddddd"); // 6, nested + lowercase
        write_file(&sub, "clip.MOV", b"movie"); // 5, nested video

        let extensions = &[
            "HIF", "HEIF", "HEIC", "JPG", "JPEG", "MOV", "MP4", "M4V", "AVI",
        ];
        let expected = index::compute_fingerprint(&dir, extensions, None).unwrap();
        let (images, folded) = scan_library_images(&dir).unwrap();

        assert_eq!(folded, expected);
        assert_eq!(folded.file_count, 5);
        assert_eq!(folded.total_bytes, 25);
        assert_eq!(images.len(), 5);
        let movie = images.iter().find(|item| item.file_name == "clip.MOV").unwrap();
        assert_eq!(movie.media_type, MediaType::Video);
        assert_eq!(movie.id, "clip.MOV");

        fs::remove_dir_all(&dir).ok();
    }
}
