//! Engine-agnostic core logic and data structures for Shoji.
//!
//! Settings, tab state, the typed IPC protocol between the browser UI and the host,
//! environment detection, and (on Windows) shell integration helpers shared by the
//! browser and its installer.

pub mod bounce;
pub mod breach;
pub mod env;
pub mod import;
pub mod ipc;
pub mod launches;
pub mod notes;
pub mod settings;
#[cfg(windows)]
pub mod shell;
pub mod tabs;
pub mod userdata;
pub mod vault;

/// Product name shown to users.
pub const APP_NAME: &str = "Shoji";
/// Scheme prefix for internal pages (`shoji://newtab`, `shoji://settings`, `shoji://welcome`).
pub const INTERNAL_SCHEME: &str = "shoji://";
/// Folder name under `%APPDATA%` holding settings, the WebView2 profile, and generated icons.
pub const DATA_DIR_NAME: &str = "shoji";
