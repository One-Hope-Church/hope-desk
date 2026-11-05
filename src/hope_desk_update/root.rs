//! Production flow: root LaunchDaemon scheduler (plan 2.1.3).
//!
//! State machine (one thread, `hope-desk-updater`):
//!   start -> reconcile previous attempt -> sleep 5 min -> loop { check_once -> sleep(next) }
//!   check_once outcomes: Idle (6h + 0..30 min jitter) | Busy (session active, 10 min) |
//!                        Retry (manifest/network/lock error, 1 h) | Started (install script
//!                        detached; sleep 1 h, the daemon is normally replaced before then).
//! Never updates during a session: checked before download, after download, and (inside upstream's
//! `update_from_dmg_as_root`) after extraction and right before launching the swap script.

use super::{manifest, verify};
use hbb_common::{bail, log, ResultType};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const FIRST_CHECK_DELAY: Duration = Duration::from_secs(5 * 60);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const JITTER_SECS: u64 = 30 * 60;
const BUSY_RETRY: Duration = Duration::from_secs(10 * 60);
const ERROR_RETRY: Duration = Duration::from_secs(60 * 60);
const DOWNLOAD_FAILURES_BEFORE_GIVING_UP: u32 = 3;

const FAILED_FILE: &str = "/var/root/.hopedesk-update-failed";
const ATTEMPT_FILE: &str = "/var/root/.hopedesk-update-attempt";
/// Written by upstream's detached install script when it rolls back.
const UPSTREAM_FAILED_MARKER: &str = "/var/root/.rustdeskupdate_failed";

enum Outcome {
    Idle,
    Busy,
    Retry,
    Started,
}

pub fn start() {
    if !crate::hope_desk::is_production() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("hope-desk-updater".to_owned())
        .spawn(run);
    if let Err(err) = spawned {
        log::error!("[hope-desk-update] failed to start scheduler thread: {}", err);
    }
}

fn run() {
    log::info!("[hope-desk-update] production scheduler started");
    reconcile_previous_attempt();
    std::thread::sleep(FIRST_CHECK_DELAY);
    let mut download_failures: (u64, u32) = (0, 0);
    loop {
        let next = match check_once(&mut download_failures) {
            Ok(Outcome::Idle) => CHECK_INTERVAL + Duration::from_secs(jitter_secs()),
            Ok(Outcome::Busy) => BUSY_RETRY,
            Ok(Outcome::Retry) => ERROR_RETRY,
            Ok(Outcome::Started) => ERROR_RETRY,
            Err(err) => {
                log::warn!("[hope-desk-update] check failed: {}", err);
                ERROR_RETRY
            }
        };
        std::thread::sleep(next);
    }
}

/// 0..JITTER_SECS from the clock; avoids a new dependency just for fleet spreading.
fn jitter_secs() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 ^ d.as_secs())
        .unwrap_or(0);
    nanos % JITTER_SECS
}

fn read_failed_builds() -> Vec<u64> {
    std::fs::read_to_string(FAILED_FILE)
        .map(|s| s.lines().filter_map(|l| l.trim().parse().ok()).collect())
        .unwrap_or_default()
}

fn mark_failed(build: u64) {
    use std::io::Write;
    let res = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(FAILED_FILE)
        .and_then(|mut f| writeln!(f, "{}", build));
    match res {
        Ok(()) => log::warn!("[hope-desk-update] build {} marked failed; never retried", build),
        Err(err) => log::error!("[hope-desk-update] cannot record failed build {}: {}", build, err),
    }
}

/// Decide at scheduler start whether the previous install attempt worked.
fn reconcile_previous_attempt() {
    let Some(attempt) = std::fs::read_to_string(ATTEMPT_FILE)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
    else {
        return;
    };
    let Some(own) = manifest::own_build() else {
        return;
    };
    if own == attempt {
        log::info!("[hope-desk-update] updated to build {}", attempt);
    } else if Path::new(UPSTREAM_FAILED_MARKER).exists() || own < attempt {
        mark_failed(attempt);
    } else {
        // own > attempt: an even newer build is already running; nothing failed.
        log::info!("[hope-desk-update] attempt for build {} superseded by build {}", attempt, own);
    }
    let _ = std::fs::remove_file(ATTEMPT_FILE);
    let _ = std::fs::remove_file(UPSTREAM_FAILED_MARKER);
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> ResultType<Self> {
        let out = std::process::Command::new("/usr/bin/mktemp")
            .args(["-d", "/tmp/.hopedesk-update-XXXXXX"])
            .output()?;
        if !out.status.success() {
            bail!("mktemp failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        if path.as_os_str().is_empty() {
            bail!("mktemp returned an empty path");
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_dir_all(&self.0) {
            log::warn!("[hope-desk-update] cannot remove {}: {}", self.0.display(), err);
        }
    }
}

/// Remove our own leftover temp dirs older than an hour (crashed checks).
fn clean_stale_temp_dirs() {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = std::fs::read_dir("/tmp") else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(".hopedesk-update-") {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age >= Duration::from_secs(3600));
        if meta.file_type().is_dir() && meta.uid() == 0 && old {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

fn check_once(download_failures: &mut (u64, u32)) -> ResultType<Outcome> {
    let _lock = match crate::updater::acquire_mac_update_lock() {
        Ok(l) => l,
        Err(err) => {
            log::info!("[hope-desk-update] update lock unavailable: {}", err);
            return Ok(Outcome::Busy);
        }
    };
    clean_stale_temp_dirs();

    let m = match manifest::fetch_manifest() {
        Ok(m) => m,
        Err(err) => {
            log::info!("[hope-desk-update] manifest unavailable/invalid: {}", err);
            return Ok(Outcome::Retry);
        }
    };
    let ids = manifest::rollout_ids();
    let failed = read_failed_builds();
    let Some(rel) = manifest::eligible_release(&m, &ids, &failed) else {
        log::info!("[hope-desk-update] no eligible update");
        return Ok(Outcome::Idle);
    };
    match manifest::own_build() {
        Some(own) if rel.build > own => {}
        _ => return Ok(Outcome::Idle),
    }
    log::info!("[hope-desk-update] build {} is available", rel.build);

    // Session checkpoint 1: before download.
    if !crate::updater::has_no_active_conns_ipc() {
        log::info!("[hope-desk-update] session active; retrying in 10 min");
        return Ok(Outcome::Busy);
    }

    let tmp = TempDir::new()?;
    let dmg = match verify::download_verified(&rel, &tmp.0) {
        Ok(p) => p,
        Err(err) => {
            log::warn!("[hope-desk-update] download of build {} failed: {}", rel.build, err);
            if download_failures.0 == rel.build {
                download_failures.1 += 1;
            } else {
                *download_failures = (rel.build, 1);
            }
            if download_failures.1 >= DOWNLOAD_FAILURES_BEFORE_GIVING_UP {
                mark_failed(rel.build);
            }
            return Ok(Outcome::Retry);
        }
    };
    *download_failures = (0, 0);

    // Signature, notarization, and the app inside. Any failure: never retry this build.
    let verified = (|| -> ResultType<()> {
        verify::verify_dmg(&dmg)?;
        let mount = verify::attach_readonly(&dmg, &tmp.0)?;
        verify::verify_app(&mount.mount_point.join(manifest::shipped_bundle_name()), &rel)?;
        Ok(())
    })();
    if let Err(err) = verified {
        log::error!("[hope-desk-update] build {} refused: {}", rel.build, err);
        mark_failed(rel.build);
        return Ok(Outcome::Retry);
    }

    // Session checkpoint 2: after download and verification (download can take minutes).
    if !crate::updater::has_no_active_conns_ipc() {
        log::info!("[hope-desk-update] session started during download; deferring");
        return Ok(Outcome::Busy);
    }

    if let Err(err) = std::fs::write(ATTEMPT_FILE, rel.build.to_string()) {
        bail!("cannot record update attempt: {}", err);
    }
    // Checkpoints 3 and 4 (after extraction, 3 s before launch) are inside upstream.
    match crate::platform::update_from_dmg_as_root(&dmg.to_string_lossy(), &rel.short_version) {
        Ok(()) => {
            log::info!("[hope-desk-update] install of build {} launched", rel.build);
            Ok(Outcome::Started)
        }
        Err(err) => {
            let _ = std::fs::remove_file(ATTEMPT_FILE);
            let msg = err.to_string();
            log::warn!("[hope-desk-update] install of build {} did not start: {}", rel.build, msg);
            let lower = msg.to_ascii_lowercase();
            if lower.contains("active session") || lower.contains("session set changed") {
                // Upstream words these both "Active session ..." and "active session ...".
                Ok(Outcome::Busy)
            } else if lower.contains("mismatch")
                || lower.contains("symlink")
                || lower.contains("unsupported file")
                || lower.contains("staged service binary is missing")
            {
                // The DMG's content is wrong (identity, version, tree): never retry this build.
                mark_failed(rel.build);
                Ok(Outcome::Retry)
            } else {
                // Machine-side trouble (stale .bak recovery path, mktemp, hdiutil, plist backup):
                // not the build's fault, so retry later without blocklisting it.
                Ok(Outcome::Retry)
            }
        }
    }
}
