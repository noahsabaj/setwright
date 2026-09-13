//! Signed application updates. Installation is forbidden while any paper is open,
//! including clean Rust sessions that can still have unacknowledged browser drafts.
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub current_version: String,
    pub phase: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub automatic: bool,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Preferences {
    automatic: bool,
}

struct Inner {
    status: UpdateStatus,
    update: Option<Update>,
    bytes: Option<Vec<u8>>,
}

pub struct UpdateState {
    inner: Mutex<Inner>,
    preferences_path: PathBuf,
}

impl UpdateState {
    pub fn open(directory: PathBuf) -> Self {
        let preferences_path = directory.join("app-updates.json");
        let preferences: Preferences = std::fs::read(&preferences_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            inner: Mutex::new(Inner {
                status: UpdateStatus {
                    current_version: env!("CARGO_PKG_VERSION").into(),
                    phase: "idle".into(),
                    version: None,
                    notes: None,
                    automatic: preferences.automatic,
                    downloaded_bytes: 0,
                    total_bytes: None,
                    error: None,
                },
                update: None,
                bytes: None,
            }),
            preferences_path,
        }
    }

    fn status(&self) -> UpdateStatus {
        self.inner.lock().status.clone()
    }
    fn emit(&self, app: &AppHandle) {
        let _ = app.emit("setwright-update-status", self.status());
    }
    fn begin(&self, phase: &str) -> Result<(), String> {
        let mut inner = self.inner.lock();
        if matches!(
            inner.status.phase.as_str(),
            "checking" | "downloading" | "installing"
        ) {
            return Err("An update operation is already running.".into());
        }
        inner.status.phase = phase.into();
        inner.status.error = None;
        Ok(())
    }
    fn fail(&self, app: &AppHandle, error: String) -> String {
        {
            let mut inner = self.inner.lock();
            inner.status.phase = "error".into();
            inner.status.error = Some(error.clone());
        }
        self.emit(app);
        error
    }
}

#[tauri::command]
#[specta::specta]
pub fn app_update_status(state: State<'_, UpdateState>) -> UpdateStatus {
    state.status()
}

#[tauri::command]
#[specta::specta]
pub fn set_automatic_app_updates(
    app: AppHandle,
    state: State<'_, UpdateState>,
    enabled: bool,
) -> Result<UpdateStatus, String> {
    let bytes =
        serde_json::to_vec(&Preferences { automatic: enabled }).map_err(|e| e.to_string())?;
    // Publish the preference only after it has been persisted successfully.
    std::fs::write(&state.preferences_path, bytes).map_err(|e| e.to_string())?;
    state.inner.lock().status.automatic = enabled;
    state.emit(&app);
    Ok(state.status())
}

#[tauri::command]
#[specta::specta]
pub async fn check_app_update(
    app: AppHandle,
    state: State<'_, UpdateState>,
) -> Result<UpdateStatus, String> {
    if state.status().phase == "ready" {
        return Ok(state.status());
    }
    state.begin("checking")?;
    state.emit(&app);
    let result = async {
        app.updater_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?
            .check()
            .await
            .map_err(|e| e.to_string())
    }
    .await;
    match result {
        Ok(update) => {
            let mut inner = state.inner.lock();
            inner.status.phase = if update.is_some() {
                "available"
            } else {
                "current"
            }
            .into();
            inner.status.version = update.as_ref().map(|u| u.version.clone());
            inner.status.notes = update.as_ref().and_then(|u| u.body.clone());
            inner.update = update;
            inner.bytes = None;
        }
        Err(error) => {
            return Err(state.fail(&app, format!("Could not check for updates. Check your connection or try again after a release is published. {error}")));
        }
    }
    state.emit(&app);
    Ok(state.status())
}

#[tauri::command]
#[specta::specta]
pub async fn download_app_update(
    app: AppHandle,
    state: State<'_, UpdateState>,
) -> Result<UpdateStatus, String> {
    if state.status().phase == "ready" {
        return Ok(state.status());
    }
    let update = state
        .inner
        .lock()
        .update
        .clone()
        .ok_or("Check for an update first.")?;
    state.begin("downloading")?;
    {
        let mut inner = state.inner.lock();
        inner.status.downloaded_bytes = 0;
        inner.status.total_bytes = None;
    }
    state.emit(&app);
    let bytes = update
        .download(
            |chunk, total| {
                {
                    let mut inner = state.inner.lock();
                    inner.status.downloaded_bytes += chunk as u64;
                    inner.status.total_bytes = total;
                }
                state.emit(&app);
            },
            || {},
        )
        .await
        .map_err(|e| {
            state.fail(
                &app,
                format!("The update could not be downloaded and verified. {e}"),
            )
        })?;
    {
        let mut inner = state.inner.lock();
        inner.bytes = Some(bytes);
        inner.status.phase = "ready".into();
    }
    state.emit(&app);
    Ok(state.status())
}

#[tauri::command]
#[specta::specta]
pub async fn install_app_update(
    app: AppHandle,
    state: State<'_, UpdateState>,
) -> Result<(), String> {
    let (update, bytes) = {
        let inner = state.inner.lock();
        (
            inner.update.clone().ok_or("No update is available.")?,
            inner.bytes.clone().ok_or("Download the update first.")?,
        )
    };
    // This lock is coordinated with project registration, so another window
    // cannot open a paper between this check and the installer exiting the app.
    let desktop = app.state::<crate::ipc::DesktopState>();
    desktop.begin_application_update()?;
    if let Err(error) = state.begin("installing") {
        desktop.end_application_update();
        return Err(error);
    }
    state.emit(&app);
    if let Err(error) = update.install(bytes) {
        desktop.end_application_update();
        return Err(state.fail(&app, format!("The update could not be installed. {error}")));
    }
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_to_manual_and_reads_persisted_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!UpdateState::open(dir.path().into()).status().automatic);
        std::fs::write(
            dir.path().join("app-updates.json"),
            br#"{"automatic":true}"#,
        )
        .unwrap();
        assert!(UpdateState::open(dir.path().into()).status().automatic);
    }
    #[test]
    fn concurrent_operations_cannot_replace_active_download() {
        let dir = tempfile::tempdir().unwrap();
        let state = UpdateState::open(dir.path().into());
        state.begin("downloading").unwrap();
        assert!(state.begin("checking").is_err());
        assert!(state.begin("installing").is_err());
        assert_eq!(state.status().phase, "downloading");
    }
}
