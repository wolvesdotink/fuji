use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use serde::Serialize;
use tauri::State;
use walkdir::WalkDir;

use crate::camera::ptp;
use crate::index;
use crate::models::{CameraSourceType, CameraVolume, ImagePair};

/// One /Volumes entry + whether it looks like a camera (has DCIM).
#[derive(Debug, Serialize)]
pub struct VolumeInfo {
    pub name: String,
    pub path: String,
    pub has_dcim: bool,
}

/// Result of a camera diagnostic run. Aggregates everything the UI needs to
/// explain why camera detection is failing.
#[derive(Debug, Serialize)]
pub struct CameraDiagnostics {
    pub timestamp: String,
    pub ptp: ptp::PtpDiagnostics,
    pub codesign: String,
    pub volumes: Vec<VolumeInfo>,
}

/// Scan /Volumes/ for directories containing DCIM folders with Fuji-pattern subdirs.
pub fn scan_volumes_for_cameras() -> Result<Vec<CameraVolume>, String> {
    let volumes_path = Path::new("/Volumes");
    let mut cameras = Vec::new();

    let entries = fs::read_dir(volumes_path)
        .map_err(|e| format!("Failed to read /Volumes: {}", e))?;

    for entry in entries.flatten() {
        let volume_path = entry.path();
        if !volume_path.is_dir() {
            continue;
        }

        let dcim_path = volume_path.join("DCIM");
        if !dcim_path.exists() || !dcim_path.is_dir() {
            continue;
        }

        // Look for Fuji-pattern subdirs (e.g., 100FUJI, 100FUJIFILM)
        // or any DCIM subdirectory containing supported image files
        // (.HIF, .HEIF, .HEIC, .JPG, .JPEG, .RAF).
        if let Ok(dcim_entries) = fs::read_dir(&dcim_path) {
            for dcim_entry in dcim_entries.flatten() {
                let subdir = dcim_entry.path();
                if !subdir.is_dir() {
                    continue;
                }

                let subdir_name = dcim_entry.file_name().to_string_lossy().to_uppercase();

                // Check if it's a Fuji folder or contains supported image files
                let is_fuji_folder = subdir_name.contains("FUJI");
                let has_fuji_files = if !is_fuji_folder {
                    fs::read_dir(&subdir)
                        .map(|entries| {
                            entries.flatten().any(|e| {
                                let name = e.file_name().to_string_lossy().to_uppercase();
                                name.ends_with(".HIF")
                                    || name.ends_with(".HEIF")
                                    || name.ends_with(".HEIC")
                                    || name.ends_with(".JPG")
                                    || name.ends_with(".JPEG")
                                    || name.ends_with(".RAF")
                            })
                        })
                        .unwrap_or(false)
                } else {
                    true
                };

                if is_fuji_folder || has_fuji_files {
                    let volume_name = volume_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();

                    cameras.push(CameraVolume {
                        name: volume_name,
                        mount_path: volume_path.to_string_lossy().to_string(),
                        dcim_path: dcim_path.to_string_lossy().to_string(),
                        source_type: CameraSourceType::MassStorage,
                    });
                    break; // Found a valid DCIM subdir, no need to check more
                }
            }
        }
    }

    Ok(cameras)
}

/// Scan for PTP cameras via the persistent ptp-bridge daemon.
pub fn scan_ptp_cameras(bridge: &ptp::PtpBridge) -> Vec<CameraVolume> {
    match bridge.scan() {
        Ok(cameras) => cameras
            .into_iter()
            .map(|c| CameraVolume {
                name: c.name.clone(),
                mount_path: c.name.clone(), // camera name used as identifier
                dcim_path: String::new(),    // not applicable for PTP
                source_type: CameraSourceType::Ptp,
            })
            .collect(),
        Err(e) => {
            log::warn!("PTP scan failed (this is normal if no PTP camera connected): {}", e);
            Vec::new()
        }
    }
}

#[tauri::command]
pub async fn scan_for_cameras(
    bridge: State<'_, Arc<ptp::PtpBridge>>,
) -> Result<Vec<CameraVolume>, String> {
    let bridge = bridge.inner().clone();
    tokio::task::spawn_blocking(move || {
        let mut cameras = scan_volumes_for_cameras()?;
        let ptp_cameras = scan_ptp_cameras(&bridge);
        cameras.extend(ptp_cameras);
        Ok(cameras)
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// Collect a diagnostic snapshot for the UI "Run Diagnostics" panel.
/// Never fails — returns whatever state it can observe.
#[tauri::command]
pub async fn camera_diagnostics() -> CameraDiagnostics {
    tokio::task::spawn_blocking(camera_diagnostics_blocking)
        .await
        .unwrap_or_else(|e| CameraDiagnostics {
            timestamp: format!("task join error: {}", e),
            ptp: ptp::diagnose(),
            codesign: String::new(),
            volumes: Vec::new(),
        })
}

fn camera_diagnostics_blocking() -> CameraDiagnostics {
    let ptp = ptp::diagnose();

    // Parse codesign output (macOS only; on other platforms we just skip)
    let codesign = if ptp.binary_exists {
        Command::new("codesign")
            .args(["-dvv", &ptp.binary_path])
            .output()
            .map(|o| {
                // codesign writes to stderr by convention
                let out = String::from_utf8_lossy(&o.stderr);
                if out.is_empty() {
                    String::from_utf8_lossy(&o.stdout).to_string()
                } else {
                    out.to_string()
                }
            })
            .unwrap_or_else(|e| format!("codesign failed: {}", e))
    } else {
        String::from("(binary missing, skipping codesign)")
    };

    // List /Volumes with DCIM presence. This mirrors the mass-storage detection path.
    let mut volumes: Vec<VolumeInfo> = Vec::new();
    if let Ok(entries) = fs::read_dir("/Volumes") {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let has_dcim = path.join("DCIM").is_dir();
            volumes.push(VolumeInfo {
                name,
                path: path.to_string_lossy().to_string(),
                has_dcim,
            });
        }
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{}s since epoch", d.as_secs()))
        .unwrap_or_else(|_| String::from("unknown"));

    CameraDiagnostics {
        timestamp,
        ptp,
        codesign,
        volumes,
    }
}

/// List images from a mass-storage DCIM path.
fn list_images_mass_storage(dcim_path: &str) -> Result<Vec<ImagePair>, String> {
    let dcim = Path::new(dcim_path);
    if !dcim.exists() {
        return Err(format!("DCIM path does not exist: {}", dcim_path));
    }

    // Collect all HIF and RAF files from DCIM subdirectories
    let mut hif_files: std::collections::HashMap<String, (String, u64)> =
        std::collections::HashMap::new();
    let mut raf_files: std::collections::HashMap<String, (String, u64)> =
        std::collections::HashMap::new();

    for entry in WalkDir::new(dcim)
        .min_depth(1)
        .max_depth(2)
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

        let stem = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);

        match ext.as_str() {
            "HIF" | "HEIF" | "HEIC" | "JPG" | "JPEG" => {
                hif_files.insert(stem, (path.to_string_lossy().to_string(), size));
            }
            "RAF" => {
                raf_files.insert(stem, (path.to_string_lossy().to_string(), size));
            }
            _ => {}
        }
    }

    // Build image pairs - start from HIF files, optionally pair with RAF
    let mut pairs: Vec<ImagePair> = hif_files
        .into_iter()
        .map(|(stem, (hif_path, hif_size))| {
            let (raf_path, raf_size) = raf_files
                .remove(&stem)
                .map(|(p, s)| (Some(p), Some(s)))
                .unwrap_or((None, None));

            ImagePair {
                id: stem,
                hif_path,
                raf_path,
                hif_size,
                raf_size,
            }
        })
        .collect();

    // Also include any RAF files without a matching HIF
    for (stem, (_raf_path, _raf_size)) in raf_files {
        log::warn!("RAF file without matching HIF: {}", stem);
    }

    // Sort by filename (which is chronological on Fuji cameras)
    pairs.sort_by(|a, b| a.id.cmp(&b.id));

    Ok(pairs)
}

/// List images from a PTP camera by running the catalog command.
fn list_images_ptp(
    bridge: &ptp::PtpBridge,
    camera_name: &str,
    thumb_cache_dir: &str,
) -> Result<Vec<ImagePair>, String> {
    let catalog = bridge.catalog(camera_name, thumb_cache_dir)?;

    // Group files by stem into HIF/RAF pairs
    let mut hif_files: std::collections::HashMap<String, (String, u64, Option<String>)> =
        std::collections::HashMap::new();
    let mut raf_files: std::collections::HashMap<String, (String, u64)> =
        std::collections::HashMap::new();

    for file in &catalog.files {
        let name_upper = file.name.to_uppercase();
        let stem = Path::new(&file.name)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        if name_upper.ends_with(".HIF") || name_upper.ends_with(".HEIF") || name_upper.ends_with(".HEIC") || name_upper.ends_with(".JPG") || name_upper.ends_with(".JPEG") {
            let ptp_path = ptp::make_ptp_path(camera_name, &file.name);
            hif_files.insert(stem, (ptp_path, file.size.max(0) as u64, file.thumbnail.clone()));
        } else if name_upper.ends_with(".RAF") {
            let ptp_path = ptp::make_ptp_path(camera_name, &file.name);
            raf_files.insert(stem, (ptp_path, file.size.max(0) as u64));
        }
    }

    let mut pairs: Vec<ImagePair> = hif_files
        .into_iter()
        .map(|(stem, (hif_path, hif_size, _thumbnail))| {
            let (raf_path, raf_size) = raf_files
                .remove(&stem)
                .map(|(p, s)| (Some(p), Some(s)))
                .unwrap_or((None, None));

            ImagePair {
                id: stem,
                hif_path,
                raf_path,
                hif_size,
                raf_size,
            }
        })
        .collect();

    pairs.sort_by(|a, b| a.id.cmp(&b.id));

    Ok(pairs)
}

#[tauri::command]
pub async fn list_images(dcim_path: String, cache_dir: String) -> Result<Vec<ImagePair>, String> {
    tokio::task::spawn_blocking(move || {
        let cache = Path::new(&cache_dir);
        fs::create_dir_all(cache)
            .map_err(|e| format!("Failed to create cache dir: {}", e))?;
        let index_path = cache.join("camera-index.json");

        // Compute fingerprint for change detection
        let dcim = Path::new(&dcim_path);
        let extensions = &["HIF", "HEIF", "HEIC", "JPG", "JPEG", "RAF"];
        if let Ok(fingerprint) = index::compute_fingerprint(dcim, extensions, Some(2)) {
            // Try cached index
            if let Some(cached) = index::try_cached::<ImagePair>(&index_path, &dcim_path, &fingerprint) {
                log::info!("Using cached camera index ({} images)", cached.len());
                return Ok(cached);
            }

            // Cache miss: do full walk
            let images = list_images_mass_storage(&dcim_path)?;

            // Save index for next time
            if let Err(e) = index::cache_images(&index_path, &dcim_path, &fingerprint, &images) {
                log::warn!("Failed to save camera index: {}", e);
            }

            Ok(images)
        } else {
            // Fingerprint failed, fall back to direct scan
            list_images_mass_storage(&dcim_path)
        }
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// List images from a PTP camera. Called when source_type is Ptp.
#[tauri::command]
pub async fn ptp_list_images(
    bridge: State<'_, Arc<ptp::PtpBridge>>,
    camera_name: String,
    thumb_cache_dir: String,
) -> Result<Vec<ImagePair>, String> {
    let bridge = bridge.inner().clone();
    tokio::task::spawn_blocking(move || list_images_ptp(&bridge, &camera_name, &thumb_cache_dir))
        .await
        .map_err(|e| format!("Task join error: {}", e))?
}

/// Download a file from a PTP camera to a local cache directory.
/// Returns the local file path.
#[tauri::command]
pub async fn ptp_download_file(
    bridge: State<'_, Arc<ptp::PtpBridge>>,
    camera_name: String,
    file_name: String,
    dest_dir: String,
) -> Result<String, String> {
    let bridge = bridge.inner().clone();
    tokio::task::spawn_blocking(move || {
        let result = bridge.download(&camera_name, &dest_dir, &[file_name.clone()])?;

        if let Some(downloaded) = result.downloaded.first() {
            Ok(downloaded.path.clone())
        } else {
            let error_msg = result
                .errors
                .first()
                .cloned()
                .unwrap_or_else(|| format!("Failed to download {}", file_name));
            Err(error_msg)
        }
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// Delete files from a PTP camera.
/// Accepts PTP paths (ptp://CameraName/FileName) and groups by camera.
#[tauri::command]
pub async fn ptp_delete_files(
    bridge: State<'_, Arc<ptp::PtpBridge>>,
    camera_name: String,
    file_names: Vec<String>,
) -> Result<u32, String> {
    let bridge = bridge.inner().clone();
    tokio::task::spawn_blocking(move || {
        let result = bridge.delete(&camera_name, &file_names)?;

        if !result.errors.is_empty() {
            log::warn!("PTP delete errors: {:?}", result.errors);
        }

        Ok(result.deleted)
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}
