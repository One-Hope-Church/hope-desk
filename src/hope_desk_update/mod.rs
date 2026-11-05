//! Hope Desk automatic updates (v2.1). See active-projects/hope-desk/technical-plan.md, 2.1.x.
//!
//! A thin front end over upstream's download and root-install primitives:
//! `manifest` (S3 manifest + rollout), `verify` (Developer ID / notarization checks),
//! `root` (production LaunchDaemon scheduler), `user` (staff / Manager GUI flow).
//! macOS only; other platforms get inert stubs so shared code (`flutter_ffi.rs`) compiles.

#[cfg(target_os = "macos")]
pub mod manifest;
#[cfg(target_os = "macos")]
mod root;
#[cfg(target_os = "macos")]
mod user;
#[cfg(target_os = "macos")]
pub mod verify;

/// Called from `start_os_service` (root LaunchDaemon). Production only.
#[cfg(target_os = "macos")]
pub fn start_production_updater() {
    root::start();
}

/// Key `hope-desk-update-status` -> JSON status for Flutter. `None` = not our key.
#[cfg(target_os = "macos")]
pub fn get_common(key: &str) -> Option<String> {
    user::get_common(key)
}

/// Key `hope-desk-update-install` starts the install. Returns true if the key was ours.
#[cfg(target_os = "macos")]
pub fn set_common(key: &str, value: &str) -> bool {
    user::set_common(key, value)
}

#[cfg(not(target_os = "macos"))]
pub fn get_common(key: &str) -> Option<String> {
    if key == "hope-desk-update-status" {
        Some(r#"{"state":"none","build":0,"detail":""}"#.to_owned())
    } else {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_common(key: &str, _value: &str) -> bool {
    key == "hope-desk-update-install"
}
