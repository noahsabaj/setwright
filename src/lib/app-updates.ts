import { invoke } from "@tauri-apps/api/core";

export interface AppUpdateStatus {
  currentVersion: string;
  phase: "idle" | "checking" | "current" | "available" | "downloading" | "ready" | "installing" | "error";
  version: string | null;
  notes: string | null;
  automatic: boolean;
  downloadedBytes: number;
  totalBytes: number | null;
  error: string | null;
}

export const appUpdates = {
  status: () => invoke<AppUpdateStatus>("app_update_status"),
  check: () => invoke<AppUpdateStatus>("check_app_update"),
  download: () => invoke<AppUpdateStatus>("download_app_update"),
  install: () => invoke<void>("install_app_update"),
  setAutomatic: (enabled: boolean) => invoke<AppUpdateStatus>("set_automatic_app_updates", { enabled }),
};
