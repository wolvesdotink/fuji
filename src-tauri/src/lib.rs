mod camera;
mod clip;
mod commands;
mod index;
mod metadata;
mod models;
mod raf;

use std::sync::Arc;

use tauri::Manager;

use commands::{camera as camera_cmd, cleanup, config, import, library, ratings, thumbnails};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            // Spawn the persistent ptp-bridge daemon. One ICDeviceBrowser stays
            // alive for the life of the app so catalog/download no longer race
            // a fresh discovery window against scan. See camera/ptp.rs.
            let bridge = Arc::new(camera::ptp::PtpBridge::new());
            app.manage(bridge.clone());

            // Start the volume mount watcher (also polls the bridge for PTP)
            let handle = app.handle().clone();
            camera::detection::start_volume_watcher(handle, bridge);

            Ok(())
        });

    // Updater + process + custom application menu are desktop-only. The updater
    // plugin verifies signed payloads against the public key in tauri.conf.json
    // (plugins.updater.pubkey) and fetches manifests from the configured
    // endpoints (latest.json on GitHub Releases). The custom menu mirrors the
    // standard predefined macOS items and adds a "Check for Updates…" entry
    // that emits an event the frontend composable consumes to trigger a manual
    // check — keeping the available/downloading/ready state machine in one
    // place (the Vue side) instead of duplicating it in Rust.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        use tauri::menu::{AboutMetadataBuilder, Menu, MenuItem, SubmenuBuilder};
        use tauri::Emitter;

        builder = builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_process::init())
            .menu(|handle| {
                let pkg = handle.package_info();
                let app_name = handle
                    .config()
                    .product_name
                    .clone()
                    .unwrap_or_else(|| pkg.name.clone());
                let about = AboutMetadataBuilder::new()
                    .name(Some(app_name.clone()))
                    .version(Some(pkg.version.to_string()))
                    .build();
                let check_updates = MenuItem::with_id(
                    handle,
                    "check_for_updates",
                    "Check for Updates…",
                    true,
                    None::<&str>,
                )?;
                let app_submenu = SubmenuBuilder::new(handle, &app_name)
                    .about(Some(about))
                    .separator()
                    .item(&check_updates)
                    .separator()
                    .services()
                    .separator()
                    .hide()
                    .hide_others()
                    .show_all()
                    .separator()
                    .quit()
                    .build()?;
                let file_submenu = SubmenuBuilder::new(handle, "File")
                    .close_window()
                    .build()?;
                let edit_submenu = SubmenuBuilder::new(handle, "Edit")
                    .undo()
                    .redo()
                    .separator()
                    .cut()
                    .copy()
                    .paste()
                    .select_all()
                    .build()?;
                let window_submenu = SubmenuBuilder::new(handle, "Window")
                    .minimize()
                    .maximize()
                    .separator()
                    .close_window()
                    .build()?;
                Menu::with_items(
                    handle,
                    &[&app_submenu, &file_submenu, &edit_submenu, &window_submenu],
                )
            })
            .on_menu_event(|app, event| {
                if event.id() == "check_for_updates" {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.emit("menu://check-for-updates", ());
                    }
                }
            });
    }

    builder
        .invoke_handler(tauri::generate_handler![
            camera_cmd::scan_for_cameras,
            camera_cmd::camera_diagnostics,
            camera_cmd::list_images,
            camera_cmd::ptp_list_images,
            camera_cmd::ptp_download_file,
            camera_cmd::ptp_delete_files,
            thumbnails::generate_thumbnails,
            thumbnails::get_thumbnail,
            import::import_files,
            import::ptp_import_files,
            cleanup::get_files_to_delete,
            cleanup::delete_from_camera,
            config::load_config,
            config::save_config,
            library::list_library_images,
            library::generate_library_thumbnails,
            library::ensure_clip_models,
            library::index_library,
            library::search_library,
            ratings::read_file_ratings,
            ratings::write_file_rating,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
