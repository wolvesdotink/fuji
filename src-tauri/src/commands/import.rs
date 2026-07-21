use std::fs;
use std::io::{Read, Write};
use std::path::Path;
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

        // Chunked copy with progress
        let mut src_file = fs::File::open(src)
            .map_err(|e| format!("Failed to open source file {}: {}", source_path, e))?;

        let mut dst_file = fs::File::create(&dst)
            .map_err(|e| format!("Failed to create destination file {}: {}", dest_filename, e))?;

        let mut buffer = vec![0u8; COPY_BUFFER_SIZE];
        loop {
            let bytes_read = src_file
                .read(&mut buffer)
                .map_err(|e| format!("Read error on {}: {}", source_path, e))?;

            if bytes_read == 0 {
                break;
            }

            dst_file
                .write_all(&buffer[..bytes_read])
                .map_err(|e| format!("Write error on {}: {}", dest_filename, e))?;

            bytes_copied += bytes_read as u64;

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
        }

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
/// Async wrapper so the blocking download + osascript calls run on a dedicated
/// thread instead of Tauri's main thread.
#[tauri::command]
pub async fn ptp_import_files(
    bridge: State<'_, Arc<ptp::PtpBridge>>,
    camera_name: String,
    selections: Vec<ImportSelection>,
    dest_dir: String,
    on_progress: Channel<ImportProgress>,
) -> Result<(), String> {
    let bridge = bridge.inner().clone();
    tokio::task::spawn_blocking(move || {
        ptp_import_files_blocking(bridge, camera_name, selections, dest_dir, on_progress)
    })
    .await
    .map_err(|e| format!("Import task join error: {}", e))?
}

fn ptp_import_files_blocking(
    bridge: Arc<ptp::PtpBridge>,
    camera_name: String,
    selections: Vec<ImportSelection>,
    dest_dir: String,
    on_progress: Channel<ImportProgress>,
) -> Result<(), String> {
    let dest = Path::new(&dest_dir);
    fs::create_dir_all(dest)
        .map_err(|e| format!("Failed to create destination directory: {}", e))?;

    // Collect all file names to download
    let mut files_to_download: Vec<String> = Vec::new();

    for selection in &selections {
        match selection.choice {
            SelectionChoice::Skip => continue,
            SelectionChoice::HeifOnly => {
                if let Some((_, file_name)) = ptp::parse_ptp_path(&selection.hif_path) {
                    files_to_download.push(file_name);
                }
            }
            SelectionChoice::HeifAndRaw => {
                if let Some((_, file_name)) = ptp::parse_ptp_path(&selection.hif_path) {
                    files_to_download.push(file_name);
                }
                if let Some(ref raf_path) = selection.raf_path {
                    if let Some((_, file_name)) = ptp::parse_ptp_path(raf_path) {
                        files_to_download.push(file_name);
                    }
                }
            }
        }
    }

    let files_total = files_to_download.len() as u32;

    // Report start
    let _ = on_progress.send(ImportProgress {
        current_file: "Downloading from camera...".to_string(),
        files_completed: 0,
        files_total,
        bytes_copied: 0,
        bytes_total: 0,
        phase: ImportPhase::CopyingToLaCie,
    });

    // Download all files in one batch via ptp-bridge. The daemon streams a
    // progress event as each file finishes, which we forward to the UI so the
    // counter climbs in real time instead of jumping from 0 to full at the end.
    let progress_channel = on_progress.clone();
    let result = bridge.download_with_progress(
        &camera_name,
        &dest_dir,
        &files_to_download,
        move |p| {
            let _ = progress_channel.send(ImportProgress {
                current_file: p.name,
                files_completed: p.completed,
                files_total,
                bytes_copied: 0,
                bytes_total: 0,
                phase: ImportPhase::CopyingToLaCie,
            });
        },
    )?;

    if !result.errors.is_empty() {
        log::warn!("PTP download errors: {:?}", result.errors);
    }

    let downloaded_count = result.downloaded.len() as u32;

    // Report download complete
    let _ = on_progress.send(ImportProgress {
        current_file: format!("Downloaded {} files", downloaded_count),
        files_completed: downloaded_count,
        files_total,
        bytes_copied: 0,
        bytes_total: 0,
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
    let photos_dest_paths: Vec<String> = result
        .downloaded
        .iter()
        .filter(|f| {
            let upper = f.name.to_uppercase();
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
        .map(|f| f.path.clone())
        .collect();

    if !photos_dest_paths.is_empty() {
        let _ = on_progress.send(ImportProgress {
            current_file: "Importing to Apple Photos...".to_string(),
            files_completed: files_total,
            files_total,
            bytes_copied: 0,
            bytes_total: 0,
            phase: ImportPhase::ImportingToPhotos,
        });

        import_to_apple_photos(&photos_dest_paths)?;
    }

    // Done
    let _ = on_progress.send(ImportProgress {
        current_file: "Complete!".to_string(),
        files_completed: files_total,
        files_total,
        bytes_copied: 0,
        bytes_total: 0,
        phase: ImportPhase::Complete,
    });

    Ok(())
}

/// Import HIF files to Apple Photos using osascript.
/// Batches files in groups to avoid AppleScript timeouts.
fn import_to_apple_photos(file_paths: &[String]) -> Result<(), String> {
    const BATCH_SIZE: usize = 15;

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
                activate
                delay 2
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
