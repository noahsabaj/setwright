//! Native, fail-closed sandbox launchers.
//!
//! Each implementation accepts only the authorization minted by
//! [`crate::services::sandbox::AttestedSandboxBroker`]. There is deliberately
//! no generic process-spawning fallback in this module.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{BubblewrapLauncher, BubblewrapSidecar};
#[cfg(target_os = "macos")]
pub use macos::{MacosXpcLauncher, MacosXpcService};
#[cfg(target_os = "windows")]
pub use windows::WindowsAppContainerLauncher;
