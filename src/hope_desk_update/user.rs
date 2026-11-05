//! Staff / Manager flow: user-level GUI process (plan 2.1.4).
//!
//! State machine (`STATE`), polled by Flutter through `hope-desk-update-status`:
//!   none --check finds newer eligible build--> available | needs-move (bundle in /Volumes or translocated)
//!   available|busy|failed --Update clicked--> busy (session open: nothing happens)
//!                                         --> installing --> relaunch + quit
//!                                                        --> failed (error) | available (admin prompt cancelled)
//! The check runs 20 s after the first status poll, then every 6 h. Manifest errors leave the state alone.
//! Production never shows anything: it updates silently from root.rs.

use super::{manifest, verify};
use hbb_common::{bail, log, ResultType};
use std::{
    ffi::CString,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, Once,
    },
    time::Duration,
};

const KEY_STATUS: &str = "hope-desk-update-status";
const KEY_INSTALL: &str = "hope-desk-update-install";
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const ADMIN_PROMPT: &str = "Hope Desk needs an administrator to install the update.";

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    None,
    Available,
    Installing,
    Busy,
    Failed,
    NeedsMove,
}

impl State {
    fn as_str(self) -> &'static str {
        match self {
            State::None => "none",
            State::Available => "available",
            State::Installing => "installing",
            State::Busy => "busy",
            State::Failed => "failed",
            State::NeedsMove => "needs-move",
        }
    }
}

struct Status {
    state: State,
    release: Option<manifest::AppRelease>,
    detail: String,
}

static STATE: Mutex<Status> = Mutex::new(Status {
    state: State::None,
    release: None,
    detail: String::new(),
});
static STARTED: Once = Once::new();
static INSTALLING: AtomicBool = AtomicBool::new(false);

/// Marker errors recognised by `install_requested` via `downcast_ref`.
#[derive(Debug)]
struct Cancelled;
#[derive(Debug)]
struct Deferred;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "administrator prompt cancelled")
    }
}
impl std::fmt::Display for Deferred {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a session is active")
    }
}
impl std::error::Error for Cancelled {}
impl std::error::Error for Deferred {}

pub fn get_common(key: &str) -> Option<String> {
    if key != KEY_STATUS {
        return None;
    }
    if crate::hope_desk::is_production() {
        return Some(r#"{"state":"none","build":0,"detail":""}"#.to_owned());
    }
    STARTED.call_once(start_checker);
    let s = STATE.lock().unwrap();
    Some(
        serde_json::json!({
            "state": s.state.as_str(),
            "build": s.release.as_ref().map(|r| r.build).unwrap_or(0),
            "detail": s.detail,
        })
        .to_string(),
    )
}

pub fn set_common(key: &str, _value: &str) -> bool {
    if key != KEY_INSTALL {
        return false;
    }
    if crate::hope_desk::is_production() {
        return true;
    }
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return true;
    }
    let spawned = std::thread::Builder::new()
        .name("hope-desk-install".to_owned())
        .spawn(|| {
            install_requested();
            INSTALLING.store(false, Ordering::SeqCst);
        });
    if spawned.is_err() {
        INSTALLING.store(false, Ordering::SeqCst);
    }
    true
}

fn set_state(state: State, detail: &str) {
    let mut s = STATE.lock().unwrap();
    s.state = state;
    s.detail = detail.to_owned();
}

fn start_checker() {
    // A local build (no CI build number) never auto-updates.
    if manifest::own_build().is_none() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("hope-desk-update-check".to_owned())
        .spawn(|| {
            std::thread::sleep(FIRST_CHECK_DELAY);
            loop {
                check();
                std::thread::sleep(CHECK_INTERVAL);
            }
        });
}

fn check() {
    if INSTALLING.load(Ordering::SeqCst) {
        return;
    }
    let m = match manifest::fetch_manifest() {
        Ok(m) => m,
        Err(err) => {
            log::info!("[hope-desk-update] manifest unavailable/invalid: {}", err);
            return;
        }
    };
    let rel = manifest::eligible_release(&m, &manifest::rollout_ids(), &[]);
    let rel = match (rel, manifest::own_build()) {
        (Some(r), Some(own)) if r.build > own => Some(r),
        _ => None,
    };
    let mut s = STATE.lock().unwrap();
    if INSTALLING.load(Ordering::SeqCst) {
        return;
    }
    match rel {
        None => {
            s.release = None;
            s.state = State::None;
            s.detail.clear();
        }
        Some(r) => {
            log::info!("[hope-desk-update] build {} is available", r.build);
            let same_build_failed =
                s.state == State::Failed && s.release.as_ref().map(|o| o.build) == Some(r.build);
            s.release = Some(r);
            if same_build_failed || s.state == State::Busy {
                return; // keep the message the person already saw
            }
            s.state = if needs_move() { State::NeedsMove } else { State::Available };
            s.detail.clear();
        }
    }
}

fn running_bundle() -> ResultType<PathBuf> {
    let exe = std::env::current_exe()?;
    // <App>.app/Contents/MacOS/<exe>
    match exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
        Some(b) if b.extension().is_some_and(|e| e == "app") => Ok(b.to_path_buf()),
        _ => bail!("not running from an app bundle: {}", exe.display()),
    }
}

fn needs_move() -> bool {
    match running_bundle() {
        Ok(b) => {
            let s = b.to_string_lossy();
            s.starts_with("/Volumes/") || s.contains("/AppTranslocation/")
        }
        Err(_) => true,
    }
}

fn install_requested() {
    let release = {
        let s = STATE.lock().unwrap();
        match (s.state, s.release.clone()) {
            (State::Available | State::Busy | State::Failed, Some(r)) => r,
            _ => return,
        }
    };
    if needs_move() {
        set_state(State::NeedsMove, "");
        return;
    }
    if !crate::updater::has_no_active_conns() {
        set_state(State::Busy, "");
        return;
    }
    set_state(State::Installing, "");
    match install(&release) {
        Ok(()) => {} // relaunch helper is armed; quit_gui() was called
        Err(err) if err.downcast_ref::<Cancelled>().is_some() => {
            log::info!("[hope-desk-update] administrator prompt cancelled; nothing changed");
            set_state(State::Available, "");
        }
        Err(err) if err.downcast_ref::<Deferred>().is_some() => set_state(State::Busy, ""),
        Err(err) => {
            log::error!("[hope-desk-update] install of build {} failed: {}", release.build, err);
            set_state(State::Failed, "");
        }
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> ResultType<Self> {
        let template = std::env::temp_dir().join("hopedesk-update-XXXXXX");
        let out = Command::new("/usr/bin/mktemp")
            .arg("-d")
            .arg(&template)
            .output()?;
        if !out.status.success() {
            bail!("mktemp failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        if path.as_os_str().is_empty() {
            bail!("mktemp returned an empty path");
        }
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn writable(path: &Path) -> bool {
    CString::new(std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()))
        .map(|c| unsafe { hbb_common::libc::access(c.as_ptr(), hbb_common::libc::W_OK) == 0 })
        .unwrap_or(false)
}

fn ditto(src: &Path, dst: &Path) -> ResultType<()> {
    let out = Command::new("/usr/bin/ditto").arg(src).arg(dst).output()?;
    if !out.status.success() {
        bail!("ditto failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// The requirement verify.rs applies to the app, passed to the root script (path C).
fn app_requirement() -> String {
    verify::requirement_app(manifest::own_bundle_id())
}

fn install(rel: &manifest::AppRelease) -> ResultType<()> {
    let tmp = TempDir::new()?;
    let dmg = verify::download_verified(rel, &tmp.0)?;
    verify::verify_dmg(&dmg)?;
    let mount = verify::attach_readonly(&dmg, &tmp.0)?;
    let src_app = mount.mount_point.join(manifest::shipped_bundle_name());
    verify::verify_app(&src_app, rel)?;

    let target = running_bundle()?;
    let parent = target
        .parent()
        .ok_or_else(|| hbb_common::anyhow::anyhow!("bundle has no parent folder"))?
        .to_path_buf();
    let pid = std::process::id();

    // Session checkpoint: the download took time.
    if !crate::updater::has_no_active_conns() {
        return Err(Deferred.into());
    }

    if writable(&parent) {
        // Path A: parent writable. Stage beside the target (same volume), swap with rename(2).
        // The staged name ends in ".app" so codesign treats it as a bundle.
        let staged = parent.join(format!(".hopedesk-update-{pid}.app"));
        let old = parent.join(format!(".hopedesk-old-{pid}.app"));
        let _ = std::fs::remove_dir_all(&staged);
        ditto(&src_app, &staged)?;
        if let Err(err) = verify::verify_app(&staged, rel) {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(err.into());
        }
        swap_renames(&target, &staged, &old, rel)?;
    } else if writable(&target) {
        // Path B: only the bundle folder is ours. Swap Contents; the seal covers Contents only.
        let full = tmp.0.join("staged").join(manifest::shipped_bundle_name());
        std::fs::create_dir_all(full.parent().unwrap())?;
        ditto(&src_app, &full)?;
        verify::verify_app(&full, rel)?;
        let new_contents = target.join(".Contents.update");
        let old_contents = target.join(".Contents.old");
        let _ = std::fs::remove_dir_all(&new_contents);
        let _ = std::fs::remove_dir_all(&old_contents);
        ditto(&full.join("Contents"), &new_contents)?;
        let live = target.join("Contents");
        std::fs::rename(&live, &old_contents)?;
        if let Err(err) = std::fs::rename(&new_contents, &live) {
            let _ = std::fs::rename(&old_contents, &live);
            let _ = std::fs::remove_dir_all(&new_contents);
            return Err(err.into());
        }
        if let Err(err) = verify::verify_app(&target, rel) {
            let _ = std::fs::remove_dir_all(&live);
            let _ = std::fs::rename(&old_contents, &live);
            return Err(err.into());
        }
        let _ = std::fs::remove_dir_all(&old_contents);
    } else {
        // Path C: neither writable. Admin prompt; cancel changes nothing.
        let full = tmp.0.join("staged").join(manifest::shipped_bundle_name());
        std::fs::create_dir_all(full.parent().unwrap())?;
        ditto(&src_app, &full)?;
        verify::verify_app(&full, rel)?;
        run_privileged_swap(&full, &target)?;
        verify::verify_app(&target, rel)?;
    }

    arm_relaunch(&target)?;
    drop(mount);
    drop(tmp);
    crate::platform::quit_gui();
    Ok(())
}

/// target -> old, staged -> target; rolls back on any failure, including a failed post-swap check.
fn swap_renames(
    target: &Path,
    staged: &Path,
    old: &Path,
    rel: &manifest::AppRelease,
) -> ResultType<()> {
    let _ = std::fs::remove_dir_all(old);
    if let Err(err) = std::fs::rename(target, old) {
        let _ = std::fs::remove_dir_all(staged);
        return Err(err.into());
    }
    if let Err(err) = std::fs::rename(staged, target) {
        let _ = std::fs::rename(old, target);
        let _ = std::fs::remove_dir_all(staged);
        return Err(err.into());
    }
    if let Err(err) = verify::verify_app(target, rel) {
        let _ = std::fs::rename(target, staged);
        let _ = std::fs::rename(old, target);
        let _ = std::fs::remove_dir_all(staged);
        return Err(err.into());
    }
    let _ = std::fs::remove_dir_all(old);
    Ok(())
}

/// Fixed root script. It re-copies into a root-owned name beside the target and re-verifies the
/// copy before swapping, so a user-writable staging dir cannot be swapped under it.
const ROOT_SWAP: &str = r#"
src="$1"; target="$2"; req="$3"
parent=$(/usr/bin/dirname "$target") || exit 1
stage="$parent/.hopedesk-update-$$.app"
old="$parent/.hopedesk-old-$$.app"
owner=$(/usr/bin/stat -f '%u:%g' "$target") || exit 1
/bin/rm -rf "$stage" "$old"
/usr/bin/ditto "$src" "$stage" || { /bin/rm -rf "$stage"; exit 1; }
/usr/sbin/chown -R "$owner" "$stage" || { /bin/rm -rf "$stage"; exit 1; }
/usr/bin/codesign --verify --deep --strict "-R=$req" "$stage" || { /bin/rm -rf "$stage"; exit 1; }
/bin/mv "$target" "$old" || { /bin/rm -rf "$stage"; exit 1; }
if ! /bin/mv "$stage" "$target"; then
  /bin/mv "$old" "$target"; /bin/rm -rf "$stage"; exit 1
fi
/bin/rm -rf "$old"
"#;

/// Fixed AppleScript; every value reaches the shell through `quoted form of`.
const APPLESCRIPT: &str = r#"on run argv
set cmd to "/bin/sh -c " & quoted form of (item 1 of argv) & " sh " & quoted form of (item 2 of argv) & " " & quoted form of (item 3 of argv) & " " & quoted form of (item 4 of argv)
do shell script cmd with prompt (item 5 of argv) with administrator privileges
end run"#;

fn run_privileged_swap(staged: &Path, target: &Path) -> ResultType<()> {
    let out = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(APPLESCRIPT)
        .arg(ROOT_SWAP)
        .arg(staged)
        .arg(target)
        .arg(app_requirement())
        .arg(ADMIN_PROMPT)
        .output()?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if err.contains("-128") || err.contains("User canceled") {
        return Err(Cancelled.into());
    }
    bail!("privileged install failed: {}", err.trim())
}

/// Detached helper: wait for our PID to exit, then reopen the app. Mirrors macos.rs
/// `relaunch_after_update_exit` (private there) but takes the real bundle path.
fn arm_relaunch(target: &Path) -> ResultType<()> {
    const RELAUNCH: &str = r#"
while /bin/kill -0 "$1" 2>/dev/null; do /bin/sleep 0.1; done
/usr/bin/open -n "$2" 2>&1 || /usr/bin/logger -t hope-desk-update "relaunch failed"
"#;
    Command::new("/bin/sh")
        .args(["-c", RELAUNCH, "sh"])
        .arg(std::process::id().to_string())
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| {
            hbb_common::anyhow::anyhow!(
                "update installed but automatic restart failed; reopen the app: {}",
                e
            )
        })?;
    Ok(())
}
