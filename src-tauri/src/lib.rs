pub mod core;
pub mod ipc;
pub mod services;

// The native entry point is not part of the unit-test harness. Keeping it out
// of that target also keeps native dialog code (which requires an executable
// Common-Controls manifest on Windows) out of pure core/IPC tests.
#[cfg(not(test))]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::{Emitter, Manager};

    let command_builder = ipc::command_builder();
    let context = tauri::generate_context!();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init());
    #[cfg(feature = "pdf-preview-e2e")]
    let builder = {
        use tauri::utils::config::CapabilityEntry;

        let dedicated_config_enabled = context.config().app.with_global_tauri
            && context
                .config()
                .app
                .security
                .capabilities
                .iter()
                .any(|entry| {
                    matches!(
                        entry,
                        CapabilityEntry::Inlined(capability)
                            if capability.identifier == "pdf-preview-e2e"
                    )
                });
        if dedicated_config_enabled {
            builder
                .plugin(tauri_plugin_wdio::init())
                .plugin(tauri_plugin_wdio_webdriver::init())
        } else {
            builder
        }
    };
    builder
        .invoke_handler(command_builder.invoke_handler())
        .setup(move |app| {
            command_builder.mount_events(app);
            let app_data_directory = app.path().app_data_dir()?;
            app.manage(ipc::DesktopState::open(app_data_directory)?);
            Ok(())
        })
        .on_window_event(|window, event| {
            let Some(state) = window.try_state::<ipc::DesktopState>() else {
                return;
            };
            match event {
                tauri::WindowEvent::CloseRequested { api, .. }
                    if state.window_has_dirty_project(window.label()) =>
                {
                    api.prevent_close();
                    let _ = window.emit(
                        "setwright-close-blocked",
                        "Save or discard the unsaved changes before closing.",
                    );
                }
                tauri::WindowEvent::Destroyed => state.close_window_sessions(window.label()),
                _ => {}
            }
        })
        .run(context)
        .expect("error while running Setwright");
}
