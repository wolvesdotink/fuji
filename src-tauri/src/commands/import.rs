use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::State;

use crate::camera::ptp;
use crate::metadata;
use crate::models::{ImportPhase, ImportProgress, ImportSelection, SelectionChoice};

const COPY_BUFFER_SIZE: usize = 8 * 1024 * 1024; // 8MB buffer

/// Minimum interval between mid-copy progress messages. Chunk-level progress
/// is only for the UI's progress bar — unthrottled it means one IPC message
/// (deserialize + reactive update on the webview main thread) per 8MB chunk,
/// thousands over a big import. File boundaries always emit regardless.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Copy one file in `COPY_BUFFER_SIZE` chunks, reporting each chunk's size to
/// `on_bytes` as it lands. Chunked rather than `fs::copy` so the caller can keep
/// the progress bar moving through a multi-gigabyte file.
fn copy_file_chunked<F: FnMut(u64)>(
    src: &Path,
    dst: &Path,
    mut on_bytes: F,
) -> Result<(), String> {
    let mut src_file = fs::File::open(src)
        .map_err(|e| format!("Failed to open source file {}: {}", src.display(), e))?;
    let mut dst_file = fs::File::create(dst)
        .map_err(|e| format!("Failed to create destination file {}: {}", dst.display(), e))?;

    let mut buffer = vec![0u8; COPY_BUFFER_SIZE];
    loop {
        let bytes_read = src_file
            .read(&mut buffer)
            .map_err(|e| format!("Read error on {}: {}", src.display(), e))?;

        if bytes_read == 0 {
            return Ok(());
        }

        dst_file
            .write_all(&buffer[..bytes_read])
            .map_err(|e| format!("Write error on {}: {}", dst.display(), e))?;

        on_bytes(bytes_read as u64);
    }
}

/// Copy selected files to the destination directory with progress reporting.
///
/// Async wrapper so the blocking I/O runs on a dedicated thread and doesn't
/// freeze Tauri's main thread (which causes the macOS beach ball).
#[tauri::command]
pub async fn import_files(
    selections: Vec<ImportSelection>,
    dest_dir: String,
    on_progress: Channel<ImportProgress>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || import_files_blocking(selections, dest_dir, on_progress))
        .await
        .map_err(|e| format!("Import task join error: {}", e))?
}

fn import_files_blocking(
    selections: Vec<ImportSelection>,
    dest_dir: String,
    on_progress: Channel<ImportProgress>,
) -> Result<(), String> {
    let dest = Path::new(&dest_dir);

    // Create destination directory
    fs::create_dir_all(dest)
        .map_err(|e| format!("Failed to create destination directory: {}", e))?;

    // Calculate total work
    let mut files_to_copy: Vec<(String, String)> = Vec::new(); // (source, dest_filename)
    let mut total_bytes: u64 = 0;

    for selection in &selections {
        match selection.choice {
            SelectionChoice::Skip => continue,
            SelectionChoice::HeifOnly => {
                let src = Path::new(&selection.hif_path);
                let filename = src.file_name().unwrap().to_string_lossy().to_string();
                let size = fs::metadata(src).map(|m| m.len()).unwrap_or(0);
                files_to_copy.push((selection.hif_path.clone(), filename));
                total_bytes += size;
            }
            SelectionChoice::HeifAndRaw => {
                // Copy HIF
                let src = Path::new(&selection.hif_path);
                let filename = src.file_name().unwrap().to_string_lossy().to_string();
                let size = fs::metadata(src).map(|m| m.len()).unwrap_or(0);
                files_to_copy.push((selection.hif_path.clone(), filename));
                total_bytes += size;

                // Copy RAF if it exists
                if let Some(ref raf_path) = selection.raf_path {
                    let raf_src = Path::new(raf_path);
                    let raf_filename = raf_src.file_name().unwrap().to_string_lossy().to_string();
                    let raf_size = fs::metadata(raf_src).map(|m| m.len()).unwrap_or(0);
                    files_to_copy.push((raf_path.clone(), raf_filename));
                    total_bytes += raf_size;
                }
            }
        }
    }

    let files_total = files_to_copy.len() as u32;
    let mut bytes_copied: u64 = 0;
    let mut last_progress = Instant::now();

    // Phase 1: Copy files to LaCie
    for (i, (source_path, dest_filename)) in files_to_copy.iter().enumerate() {
        let src = Path::new(source_path);
        let dst = dest.join(dest_filename);

        // Report progress
        let _ = on_progress.send(ImportProgress {
            current_file: dest_filename.clone(),
            files_completed: i as u32,
            files_total,
            bytes_copied,
            bytes_total: total_bytes,
            phase: ImportPhase::CopyingToLaCie,
        });

        copy_file_chunked(src, &dst, |chunk| {
            bytes_copied += chunk;

            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                last_progress = Instant::now();
                let _ = on_progress.send(ImportProgress {
                    current_file: dest_filename.clone(),
                    files_completed: i as u32,
                    files_total,
                    bytes_copied,
                    bytes_total: total_bytes,
                    phase: ImportPhase::CopyingToLaCie,
                });
            }
        })?;

        // File boundary: always emit so the files counter and bar never lag
        // behind a completed file, even inside the throttle window.
        let _ = on_progress.send(ImportProgress {
            current_file: dest_filename.clone(),
            files_completed: (i + 1) as u32,
            files_total,
            bytes_copied,
            bytes_total: total_bytes,
            phase: ImportPhase::CopyingToLaCie,
        });
    }

    // Phase 1.5: Write XMP ratings to destination files
    let file_ratings: Vec<(String, u8)> = selections
        .iter()
        .filter(|s| !matches!(s.choice, SelectionChoice::Skip))
        .filter_map(|s| {
            s.rating.map(|r| {
                let filename = Path::new(&s.hif_path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                (dest.join(&filename).to_string_lossy().to_string(), r)
            })
        })
        .collect();

    if !file_ratings.is_empty() {
        let _ = on_progress.send(ImportProgress {
            current_file: "Writing ratings...".to_string(),
            files_completed: files_total,
            files_total,
            bytes_copied: total_bytes,
            bytes_total: total_bytes,
            phase: ImportPhase::CopyingToLaCie,
        });

        if let Err(e) = metadata::write_ratings_batch(&file_ratings) {
            log::warn!("Failed to write some ratings: {}", e);
        }
    }

    // Phase 2: Import HIF files to Apple Photos
    let hif_dest_paths: Vec<String> = selections
        .iter()
        .filter(|s| !matches!(s.choice, SelectionChoice::Skip))
        .map(|s| {
            let filename = Path::new(&s.hif_path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();
            dest.join(&filename).to_string_lossy().to_string()
        })
        .collect();

    if !hif_dest_paths.is_empty() {
        let _ = on_progress.send(ImportProgress {
            current_file: "Importing to Apple Photos...".to_string(),
            files_completed: files_total,
            files_total,
            bytes_copied: total_bytes,
            bytes_total: total_bytes,
            phase: ImportPhase::ImportingToPhotos,
        });

        import_to_apple_photos(&hif_dest_paths)?;
    }

    // Phase 3: Verify copies
    let _ = on_progress.send(ImportProgress {
        current_file: "Verifying copies...".to_string(),
        files_completed: files_total,
        files_total,
        bytes_copied: total_bytes,
        bytes_total: total_bytes,
        phase: ImportPhase::Verifying,
    });

    // Simple verification: check all destination files exist and have correct sizes
    for (source_path, dest_filename) in &files_to_copy {
        let src_size = fs::metadata(source_path).map(|m| m.len()).unwrap_or(0);
        let dst = dest.join(dest_filename);
        let dst_size = fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);

        if src_size != dst_size {
            return Err(format!(
                "Verification failed for {}: source size {} != dest size {}",
                dest_filename, src_size, dst_size
            ));
        }
    }

    // Done
    let _ = on_progress.send(ImportProgress {
        current_file: "Complete!".to_string(),
        files_completed: files_total,
        files_total,
        bytes_copied: total_bytes,
        bytes_total: total_bytes,
        phase: ImportPhase::Complete,
    });

    Ok(())
}

/// Import files from a PTP camera.
/// Downloads files via ptp-bridge, then imports to Apple Photos.
///
/// `preview_cache_dir` is the directory the viewer downloads PTP previews into
/// while culling. Anything already there is copied locally instead of being
/// pulled off the camera a second time.
///
/// Async wrapper so the blocking download + osascript calls run on a dedicated
/// thread instead of Tauri's main thread.
#[tauri::command]
pub async fn ptp_import_files(
    bridge: State<'_, Arc<ptp::PtpBridge>>,
    camera_name: String,
    selections: Vec<ImportSelection>,
    dest_dir: String,
    preview_cache_dir: Option<String>,
    on_progress: Channel<ImportProgress>,
) -> Result<(), String> {
    let bridge = bridge.inner().clone();
    tokio::task::spawn_blocking(move || {
        ptp_import_files_blocking(
            bridge,
            camera_name,
            selections,
            dest_dir,
            preview_cache_dir,
            on_progress,
        )
    })
    .await
    .map_err(|e| format!("Import task join error: {}", e))?
}

fn ptp_import_files_blocking(
    bridge: Arc<ptp::PtpBridge>,
    camera_name: String,
    selections: Vec<ImportSelection>,
    dest_dir: String,
    preview_cache_dir: Option<String>,
    on_progress: Channel<ImportProgress>,
) -> Result<(), String> {
    let dest = Path::new(&dest_dir);
    fs::create_dir_all(dest)
        .map_err(|e| format!("Failed to create destination directory: {}", e))?;

    // Collect all file names to fetch, each with its catalog size (when the
    // frontend knew one) so the preview cache can be validated below.
    let mut wanted: Vec<(String, Option<u64>)> = Vec::new();

    for selection in &selections {
        match selection.choice {
            SelectionChoice::Skip => continue,
            SelectionChoice::HeifOnly => {
                if let Some((_, file_name)) = ptp::parse_ptp_path(&selection.hif_path) {
                    wanted.push((file_name, selection.hif_size));
                }
            }
            SelectionChoice::HeifAndRaw => {
                if let Some((_, file_name)) = ptp::parse_ptp_path(&selection.hif_path) {
                    wanted.push((file_name, selection.hif_size));
                }
                if let Some(ref raf_path) = selection.raf_path {
                    if let Some((_, file_name)) = ptp::parse_ptp_path(raf_path) {
                        wanted.push((file_name, selection.raf_size));
                    }
                }
            }
        }
    }

    let files_total = wanted.len() as u32;

    // Split the batch: files already in the preview cache from culling are
    // copied at disk speed, the rest come off the camera. Only a byte-exact size
    // match counts as the same file — an unknown size is never trusted, since a
    // truncated or stale cache entry would silently corrupt the import.
    let cache_dir = preview_cache_dir.as_deref().map(Path::new);
    let mut from_cache: Vec<(String, PathBuf)> = Vec::new();
    let mut from_camera: Vec<String> = Vec::new();
    let mut bytes_total: u64 = 0;

    for (file_name, size) in &wanted {
        bytes_total += size.unwrap_or(0);

        if let (Some(dir), Some(expected)) = (cache_dir, *size) {
            let cached = dir.join(file_name);
            let usable = fs::metadata(&cached)
                .map(|m| m.is_file() && m.len() == expected)
                .unwrap_or(false);
            if usable {
                from_cache.push((file_name.clone(), cached));
                continue;
            }
        }

        from_camera.push(file_name.clone());
    }

    if !from_cache.is_empty() {
        log::info!(
            "PTP import: {} of {} files served from the preview cache",
            from_cache.len(),
            wanted.len()
        );
    }

    // Report start
    let _ = on_progress.send(ImportProgress {
        current_file: "Downloading from camera...".to_string(),
        files_completed: 0,
        files_total,
        bytes_copied: 0,
        bytes_total,
        phase: ImportPhase::CopyingToLaCie,
    });

    // (camera file name, path in dest) for everything that reaches the
    // destination, cache copies first and then camera downloads, so the Photos
    // hand-off below sees the full set.
    let mut imported: Vec<(String, String)> = Vec::new();
    let mut bytes_copied: u64 = 0;
    let mut last_progress = Instant::now();

    for (file_name, cached_path) in &from_cache {
        let dst = dest.join(file_name);
        let files_completed = imported.len() as u32;

        let _ = on_progress.send(ImportProgress {
            current_file: file_name.clone(),
            files_completed,
            files_total,
            bytes_copied,
            bytes_total,
            phase: ImportPhase::CopyingToLaCie,
        });

        let bytes_before = bytes_copied;
        let copied = copy_file_chunked(cached_path, &dst, |chunk| {
            bytes_copied += chunk;
            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                last_progress = Instant::now();
                let _ = on_progress.send(ImportProgress {
                    current_file: file_name.clone(),
                    files_completed,
                    files_total,
                    bytes_copied,
                    bytes_total,
                    phase: ImportPhase::CopyingToLaCie,
                });
            }
        });

        match copied {
            Ok(()) => imported.push((file_name.clone(), dst.to_string_lossy().to_string())),
            Err(e) => {
                // The cache is an optimisation, never a requirement: fall back
                // to the camera and roll the byte counter back so progress
                // doesn't double-count this file.
                log::warn!("Preview cache copy failed for {}, using the camera: {}", file_name, e);
                bytes_copied = bytes_before;
                from_camera.push(file_name.clone());
            }
        }
    }

    // Download the remainder in one batch via ptp-bridge. The daemon streams
    // cumulative byte progress plus a line per finished file, which we offset by
    // what the cache already provided so the UI's rate and ETA stay honest.
    if !from_camera.is_empty() {
        let base_files = imported.len() as u32;
        let base_bytes = bytes_copied;
        let progress_channel = on_progress.clone();
        let result = bridge.download_with_progress(
            &camera_name,
            &dest_dir,
            &from_camera,
            move |p| {
                let _ = progress_channel.send(ImportProgress {
                    current_file: p.name,
                    files_completed: base_files + p.completed,
                    files_total,
                    bytes_copied: base_bytes + p.bytes_done.unwrap_or(0),
                    // The daemon's total is authoritative for its own batch; the
                    // selection sizes are only a seed for the first line.
                    bytes_total: p
                        .bytes_total
                        .map(|t| base_bytes + t)
                        .unwrap_or(bytes_total),
                    phase: ImportPhase::CopyingToLaCie,
                });
            },
        )?;

        if !result.errors.is_empty() {
            log::warn!("PTP download errors: {:?}", result.errors);
        }

        imported.extend(
            result
                .downloaded
                .into_iter()
                .map(|f| (f.name, f.path)),
        );
    }

    let downloaded_count = imported.len() as u32;

    // Report download complete
    let _ = on_progress.send(ImportProgress {
        current_file: format!("Downloaded {} files", downloaded_count),
        files_completed: downloaded_count,
        files_total,
        bytes_copied: bytes_total,
        bytes_total,
        phase: ImportPhase::CopyingToLaCie,
    });

    // Write XMP ratings to downloaded files
    let file_ratings: Vec<(String, u8)> = selections
        .iter()
        .filter(|s| !matches!(s.choice, SelectionChoice::Skip))
        .filter_map(|s| {
            s.rating.map(|r| {
                let filename = Path::new(&s.hif_path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                // For PTP, the filename from the ptp:// path
                let actual_name = if filename.contains("://") {
                    s.hif_path.rsplit('/').next().unwrap_or(&filename).to_string()
                } else {
                    filename
                };
                (dest.join(&actual_name).to_string_lossy().to_string(), r)
            })
        })
        .collect();

    if !file_ratings.is_empty() {
        if let Err(e) = metadata::write_ratings_batch(&file_ratings) {
            log::warn!("Failed to write some ratings: {}", e);
        }
    }

    // Import rendered stills and original movies to Apple Photos. RAF files
    // remain in the destination library but Photos receives only media it can
    // display directly.
    let photos_dest_paths: Vec<String> = imported
        .iter()
        .filter(|(name, _)| {
            let upper = name.to_uppercase();
            upper.ends_with(".HIF")
                || upper.ends_with(".HEIF")
                || upper.ends_with(".HEIC")
                || upper.ends_with(".JPG")
                || upper.ends_with(".JPEG")
                || upper.ends_with(".MOV")
                || upper.ends_with(".MP4")
                || upper.ends_with(".M4V")
                || upper.ends_with(".AVI")
        })
        .map(|(_, path)| path.clone())
        .collect();

    if !photos_dest_paths.is_empty() {
        let _ = on_progress.send(ImportProgress {
            current_file: "Importing to Apple Photos...".to_string(),
            files_completed: files_total,
            files_total,
            bytes_copied: bytes_total,
            bytes_total,
            phase: ImportPhase::ImportingToPhotos,
        });

        import_to_apple_photos(&photos_dest_paths)?;
    }

    // Done
    let _ = on_progress.send(ImportProgress {
        current_file: "Complete!".to_string(),
        files_completed: files_total,
        files_total,
        bytes_copied: bytes_total,
        bytes_total,
        phase: ImportPhase::Complete,
    });

    Ok(())
}

/// Import HIF files to Apple Photos using osascript.
/// Batches files in groups to avoid AppleScript timeouts.
fn import_to_apple_photos(file_paths: &[String]) -> Result<(), String> {
    const BATCH_SIZE: usize = 15;

    if file_paths.is_empty() {
        return Ok(());
    }

    // Launch Photos once and give it time to open its library. This used to be
    // an `activate` + `delay 2` inside every batch script, which cost 2s per 15
    // files — 40s of pure waiting on a 300-file import.
    activate_apple_photos();

    for batch in file_paths.chunks(BATCH_SIZE) {
        let file_refs: Vec<String> = batch
            .iter()
            .map(|p| format!("POSIX file \"{}\"", p))
            .collect();

        let file_list = file_refs.join(", ");

        let script = format!(
            r#"
            set fileList to {{{}}}
            tell application "Photos"
                import fileList
            end tell
            "#,
            file_list
        );

        let output = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .map_err(|e| format!("Failed to run osascript: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::error!("Apple Photos import error: {}", stderr);
            // Don't fail the entire import, just log the error
            // The user can manually import later
        }
    }

    Ok(())
}

/// Bring Photos up before the first import batch. A failure here is not fatal —
/// the import scripts that follow will launch Photos implicitly, they just may
/// have to wait for it.
fn activate_apple_photos() {
    let script = r#"
        tell application "Photos"
            activate
            delay 2
        end tell
    "#;

    match Command::new("osascript").arg("-e").arg(script).output() {
        Ok(output) if !output.status.success() => {
            log::warn!(
                "Failed to activate Apple Photos: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Err(e) => log::warn!("Failed to run osascript to activate Apple Photos: {}", e),
        _ => {}
    }
}
