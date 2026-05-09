use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use crate::commands::camera::scan_volumes_for_cameras;
use crate::camera::ptp;
use crate::models::{CameraSourceType, CameraVolume};

/// Start watching /Volumes/ for mount/unmount events.
/// Also periodically checks for PTP camera connections via the persistent bridge.
/// Emits "camera-mounted" and "camera-unmounted" events to the frontend.
pub fn start_volume_watcher(app_handle: AppHandle, bridge: Arc<ptp::PtpBridge>) {
    let handle = app_handle.clone();

    // Volume watcher thread (existing behavior)
    std::thread::spawn(move || {
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();

        let mut watcher = match RecommendedWatcher::new(tx, notify::Config::default()) {
            Ok(w) => w,
            Err(e) => {
                log::error!("Failed to create volume watcher: {}", e);
                return;
            }
        };

        if let Err(e) = watcher.watch(Path::new("/Volumes"), RecursiveMode::NonRecursive) {
            log::error!("Failed to watch /Volumes: {}", e);
            return;
        }

        log::info!("Volume watcher started on /Volumes/");

        for result in rx {
            match result {
                Ok(event) => {
                    match event.kind {
                        EventKind::Create(_) => {
                            // Small delay to let the volume fully mount
                            std::thread::sleep(std::time::Duration::from_secs(2));

                            // Check if any new camera volumes appeared
                            match scan_volumes_for_cameras() {
                                Ok(cameras) => {
                                    for camera in cameras {
                                        log::info!("Camera detected: {} at {}", camera.name, camera.mount_path);
                                        let _ = handle.emit("camera-mounted", &camera);
                                    }
                                }
                                Err(e) => log::error!("Failed to scan for cameras: {}", e),
                            }
                        }
                        EventKind::Remove(_) => {
                            // A volume was unmounted
                            for path in &event.paths {
                                if let Some(name) = path.file_name() {
                                    let name = name.to_string_lossy().to_string();
                                    log::info!("Volume unmounted: {}", name);
                                    let _ = handle.emit("camera-unmounted", serde_json::json!({ "name": name }));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    log::error!("Volume watcher error: {}", e);
                }
            }
        }
    });

    // PTP watcher thread — polls for PTP cameras periodically.
    // Runs the first scan immediately so a camera plugged in before app launch
    // is detected within seconds, not after a 10s sleep.
    std::thread::spawn(move || {
        log::info!("PTP camera watcher started (immediate first scan, then every 10s)");
        let mut known_ptp_cameras: Vec<String> = Vec::new();
        let mut first_iter = true;

        loop {
            if !first_iter {
                std::thread::sleep(std::time::Duration::from_secs(10));
            }
            first_iter = false;

            match bridge.scan() {
                Ok(cameras) => {
                    let current_names: Vec<String> = cameras.iter().map(|c| c.name.clone()).collect();

                    // Check for newly connected cameras
                    for camera in &cameras {
                        if !known_ptp_cameras.contains(&camera.name) {
                            log::info!("PTP camera connected: {}", camera.name);
                            let vol = CameraVolume {
                                name: camera.name.clone(),
                                mount_path: camera.name.clone(),
                                dcim_path: String::new(),
                                source_type: CameraSourceType::Ptp,
                            };
                            let _ = app_handle.emit("camera-mounted", &vol);
                        }
                    }

                    // Check for disconnected cameras — only trust on a successful scan
                    for name in &known_ptp_cameras {
                        if !current_names.contains(name) {
                            log::info!("PTP camera disconnected: {}", name);
                            let _ = app_handle.emit("camera-unmounted", serde_json::json!({ "name": name }));
                        }
                    }

                    known_ptp_cameras = current_names;
                }
                Err(e) => {
                    // Transient scan error: do NOT drop known cameras. A single
                    // failed poll (sidecar hiccup, USB glitch, icdd restart) would
                    // otherwise flash the UI back to the empty state.
                    log::warn!("PTP scan failed (keeping known cameras): {}", e);
                }
            }
        }
    });
}
