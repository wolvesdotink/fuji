use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::UNIX_EPOCH;
use image::{DynamicImage, ImageDecoder, ImageReader};
use rayon::prelude::*;
use tauri::ipc::Channel;
use walkdir::WalkDir;

use crate::clip;
use crate::index;
use crate::models::{
    IndexProgress, LibraryImage, ModelDownloadProgress, SearchResult, ThumbnailProgress,
};

/// List images from a destination/library directory.
/// Uses a persistent index with fingerprint-based change detection.
/// Recursively walks for HIF, HEIF, HEIC, JPG, JPEG files.
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

        let extensions = &["HIF", "HEIF", "HEIC", "JPG", "JPEG"];

        // Compute fingerprint for change detection
        if let Ok(fingerprint) = index::compute_fingerprint(dir, extensions, None) {
            // Try cached index
            if let Some(cached) =
                index::try_cached::<LibraryImage>(&index_path, &dir_path, &fingerprint)
            {
                log::info!("Using cached library index ({} images)", cached.len());
                return Ok(cached);
            }

            // Cache miss: do full walk
            let images = scan_library_images(dir)?;

            // Save index for next time
            if let Err(e) = index::cache_images(&index_path, &dir_path, &fingerprint, &images) {
                log::warn!("Failed to save library index: {}", e);
            }

            Ok(images)
        } else {
            // Fingerprint failed, fall back to direct scan
            scan_library_images(dir)
        }
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// Full filesystem scan for library images.
fn scan_library_images(dir: &Path) -> Result<Vec<LibraryImage>, String> {
    let mut images: Vec<LibraryImage> = Vec::new();

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

        match ext.as_str() {
            "HIF" | "HEIF" | "HEIC" | "JPG" | "JPEG" => {}
            _ => continue,
        }

        let stem = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let metadata = entry.metadata().ok();
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
            id: stem,
            file_path: path.to_string_lossy().to_string(),
            file_name,
            file_size,
            date_created,
            date_modified,
        });
    }

    // Sort by creation time descending (newest first)
    images.sort_by(|a, b| b.date_created.cmp(&a.date_created));

    Ok(images)
}

/// Generate thumbnails for library images.
/// Two-step process: sips decodes HEIF→JPEG (temp), then Rust re-encodes to WebP.
/// Falls back to existing .jpg thumbnails for backward compatibility.
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

    let results: Vec<(String, String)> = image_ids
        .par_iter()
        .zip(image_paths.par_iter())
        .filter_map(|(image_id, image_path)| {
            // `.v2.` suffix marks thumbnails generated with EXIF-orientation
            // applied (see reencode_to_webp). Older un-suffixed files had
            // portrait shots baked in sideways; they are ignored on disk.
            let webp_path = cache.join(format!("{}.v2.webp", image_id));
            let jpg_path = cache.join(format!("{}.v2.jpg", image_id));

            // Check for existing thumbnails (webp first, then jpg backward compat)
            let thumb_path = if webp_path.exists() {
                webp_path
            } else if jpg_path.exists() {
                jpg_path
            } else {
                // Generate new WebP thumbnail via sips + image crate
                let tmp_path = cache.join(format!("{}.tmp.jpg", image_id));

                // Step 1: sips decodes HEIF/HIF to JPEG and resizes to 600px
                let sips_result = Command::new("sips")
                    .args([
                        "--resampleWidth",
                        "600",
                        "--setProperty",
                        "format",
                        "jpeg",
                        image_path,
                        "--out",
                        &tmp_path.to_string_lossy(),
                    ])
                    .output();

                match sips_result {
                    Ok(result) if result.status.success() => {
                        // Step 2: Re-encode temp JPEG to WebP
                        match reencode_to_webp(&tmp_path, &webp_path) {
                            Ok(()) => {
                                // Clean up temp file
                                let _ = fs::remove_file(&tmp_path);
                                webp_path
                            }
                            Err(e) => {
                                log::error!("Failed to re-encode to WebP for {}: {}", image_id, e);
                                // Fall back to keeping the JPEG
                                if tmp_path.exists() {
                                    let _ = fs::rename(&tmp_path, &jpg_path);
                                    jpg_path
                                } else {
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
                    }
                    Ok(result) => {
                        let stderr = String::from_utf8_lossy(&result.stderr);
                        log::error!("sips failed for {}: {}", image_id, stderr);
                        let _ = fs::remove_file(&tmp_path);
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
                        log::error!("Failed to run sips for {}: {}", image_id, e);
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

/// Re-encode a JPEG file to WebP, applying EXIF orientation to the pixels.
///
/// sips copies the EXIF orientation tag into the temp JPEG but does not
/// physically rotate pixels. WebP encoding strips EXIF — so if we don't
/// apply the orientation in-place here, portrait shots end up sideways in
/// the cached thumbnail.
fn reencode_to_webp(jpeg_path: &Path, webp_path: &Path) -> Result<(), String> {
    let reader = ImageReader::open(jpeg_path)
        .map_err(|e| format!("Failed to open JPEG: {}", e))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| format!("Failed to build decoder: {}", e))?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)
        .map_err(|e| format!("Failed to decode JPEG: {}", e))?;
    img.apply_orientation(orientation);

    let mut output = Cursor::new(Vec::new());
    img.write_to(&mut output, image::ImageFormat::WebP)
        .map_err(|e| format!("Failed to encode WebP: {}", e))?;

    fs::write(webp_path, output.into_inner())
        .map_err(|e| format!("Failed to write WebP: {}", e))
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
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        index_library_blocking(image_ids, thumb_paths, model_dir, index_path, on_progress)
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

fn index_library_blocking(
    image_ids: Vec<String>,
    thumb_paths: Vec<String>,
    model_dir: String,
    index_path: String,
    on_progress: Channel<IndexProgress>,
) -> Result<(), String> {
    // Load existing index for incremental updates
    let mut index = clip::index::SearchIndex::load(&index_path)?;
    let already_indexed = index.indexed_ids();

    // Filter to only new images
    let new_items: Vec<(String, String)> = image_ids
        .into_iter()
        .zip(thumb_paths.into_iter())
        .filter(|(id, path)| !already_indexed.contains_key(id) && !path.is_empty())
        .collect();

    if new_items.is_empty() {
        let _ = on_progress.send(IndexProgress {
            completed: 0,
            total: 0,
        });
        return Ok(());
    }

    let total = new_items.len() as u32;

    // Load the vision model
    let vision_path = clip::model::vision_model_path(&model_dir);
    let mut session = ort::session::Session::builder()
        .map_err(|e| format!("Failed to create session builder: {}", e))?
        .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
        .map_err(|e| format!("Failed to set optimization level: {}", e))?
        .commit_from_file(&vision_path)
        .map_err(|e| format!("Failed to load vision model: {}", e))?;

    // Process each image
    for (i, (image_id, thumb_path)) in new_items.iter().enumerate() {
        match clip::image::embed_image(&mut session, thumb_path) {
            Ok(embedding) => {
                index.add(image_id.clone(), embedding);
            }
            Err(e) => {
                log::error!("Failed to embed image {}: {}", image_id, e);
            }
        }

        let _ = on_progress.send(IndexProgress {
            completed: (i + 1) as u32,
            total,
        });
    }

    // Save updated index
    index.save(&index_path)?;

    log::info!("Search index updated: {} total entries", index.entries.len());
    Ok(())
}

/// Search the library using a text query.
/// Returns matching images sorted by relevance score.
#[tauri::command]
pub async fn search_library(
    query: String,
    model_dir: String,
    index_path: String,
) -> Result<Vec<SearchResult>, String> {
    tokio::task::spawn_blocking(move || search_library_blocking(query, model_dir, index_path))
        .await
        .map_err(|e| format!("Task join error: {}", e))?
}

fn search_library_blocking(
    query: String,
    model_dir: String,
    index_path: String,
) -> Result<Vec<SearchResult>, String> {
    log::info!("Searching for: \"{}\"", query);
    let index = clip::index::SearchIndex::load(&index_path)?;
    log::info!("Loaded index with {} entries", index.entries.len());
    if index.entries.is_empty() {
        return Ok(Vec::new());
    }

    // Load text model and tokenizer
    let text_path = clip::model::text_model_path(&model_dir);
    let tok_path = clip::model::tokenizer_path(&model_dir);

    let mut text_session = ort::session::Session::builder()
        .map_err(|e| format!("Failed to create session builder: {}", e))?
        .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
        .map_err(|e| format!("Failed to set optimization level: {}", e))?
        .commit_from_file(&text_path)
        .map_err(|e| format!("Failed to load text model: {}", e))?;

    let tokenizer = tokenizers::Tokenizer::from_file(&tok_path)
        .map_err(|e| format!("Failed to load tokenizer: {}", e))?;

    // Encode the query
    let query_embedding = clip::text::embed_text(&mut text_session, &tokenizer, &query)?;

    // Search the index (min score threshold to filter irrelevant results)
    let results = index.search(&query_embedding, 0.15);
    log::info!("Search returned {} results", results.len());

    Ok(results
        .into_iter()
        .map(|(image_id, score)| SearchResult { image_id, score })
        .collect())
}
