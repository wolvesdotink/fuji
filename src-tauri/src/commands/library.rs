use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tauri::ipc::Channel;
use tauri::State;
use walkdir::WalkDir;

use crate::clip;
use crate::clip::engine::ClipEngine;
use crate::commands::thumbnails::{default_workers, generate_thumbnail_batch};
use crate::index;
use crate::models::{
    IndexFingerprint, IndexProgress, LibraryImage, MediaType, ModelDownloadProgress, SearchResult,
    ThumbnailBatch,
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

    // Newest first. Burst shots share a timestamp to the second, so ties fall
    // back to the camera's file numbering to keep the order they were shot in.
    images.sort_by(|a, b| {
        b.date_created
            .cmp(&a.date_created)
            .then_with(|| b.file_name.cmp(&a.file_name))
    });

    Ok((images, fingerprint.finish()))
}

/// Generate thumbnails for library images (`{id}.v3.jpg` in `cache_dir`).
/// Stills are thumbnailed in-process with ImageIO, movies through Quick Look;
/// see `thumbnails::generate_thumbnail_batch` for batching and concurrency.
#[tauri::command]
pub async fn generate_library_thumbnails(
    image_paths: Vec<String>,
    image_ids: Vec<String>,
    cache_dir: String,
    on_progress: Channel<ThumbnailBatch>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        generate_thumbnail_batch(
            &image_ids,
            &image_paths,
            Path::new(&cache_dir),
            default_workers(),
            &|batch| {
                let _ = on_progress.send(batch);
            },
        )
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
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
