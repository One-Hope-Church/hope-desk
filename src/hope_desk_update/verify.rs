//! Download and Developer ID verification of an update DMG and the app inside it.
//!
//! Every check shells out to base-macOS tools by absolute path. Any failure is an `Err`: the caller
//! refuses the update, deletes the download and keeps the running version. Upstream RustDesk checks
//! only SHA-256 and the bundle id; the codesign/notarization checks here are ours.

use super::manifest::{self, AppRelease};
use hbb_common::{bail, log, ResultType};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

const CODESIGN: &str = "/usr/bin/codesign";
const SPCTL: &str = "/usr/sbin/spctl";
const HDIUTIL: &str = "/usr/bin/hdiutil";
const PLISTBUDDY: &str = "/usr/libexec/PlistBuddy";
const TEAM_ID: &str = "UBH6CJDNMJ";
const NOTARIZED: &str = "source=Notarized Developer ID";

/// Requirement for the DMG: Apple-anchored, Developer ID CA, Developer ID Application leaf, our team.
fn requirement_team() -> String {
    format!(
        "anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"{}\"",
        TEAM_ID
    )
}

/// Same, plus the exact bundle id of this app variant. Shared with `user.rs` (root swap script).
pub(crate) fn requirement_app(bundle_id: &str) -> String {
    format!(
        "anchor apple generic and identifier \"{}\" and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"{}\"",
        bundle_id, TEAM_ID
    )
}

struct Out {
    ok: bool,
    text: String,
}

fn run(bin: &str, args: &[&str]) -> ResultType<Out> {
    let o = Command::new(bin).args(args).output()?;
    let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&o.stderr));
    Ok(Out {
        ok: o.status.success(),
        text,
    })
}

fn run_ok(bin: &str, args: &[&str], what: &str) -> ResultType<Out> {
    let out = run(bin, args)?;
    if !out.ok {
        bail!("{} failed: {}", what, out.text.trim());
    }
    Ok(out)
}

fn path_str(p: &Path) -> ResultType<&str> {
    match p.to_str() {
        Some(s) => Ok(s),
        None => bail!("non-UTF-8 path: {}", p.display()),
    }
}

/// Download the DMG (size-capped, SHA-256 pinned, via upstream's strict downloader) into `dir`.
pub fn download_verified(rel: &AppRelease, dir: &Path) -> ResultType<PathBuf> {
    manifest::validate_dmg_url(&rel.url)?;
    let path = dir.join(format!("hope-desk-update-{}.dmg", rel.build));
    crate::updater::download::ensure_verified_update_artifact(
        &rel.url,
        &path,
        rel.size,
        &rel.sha256,
    )?;
    Ok(path)
}

/// DMG-level check: Developer ID signature for our team, and Gatekeeper says notarized.
pub fn verify_dmg(dmg: &Path) -> ResultType<()> {
    let d = path_str(dmg)?;
    let req = format!("-R={}", requirement_team());
    run_ok(
        CODESIGN,
        &["--verify", "--strict", &req, d],
        "codesign (dmg)",
    )?;
    let out = run_ok(
        SPCTL,
        &["-a", "-t", "open", "--context", "context:primary-signature", "-vv", d],
        "spctl (dmg)",
    )?;
    if !out.text.contains(NOTARIZED) {
        bail!("dmg is not notarized: {}", out.text.trim());
    }
    Ok(())
}

pub struct Mounted {
    pub mount_point: PathBuf,
}

impl Drop for Mounted {
    fn drop(&mut self) {
        if let Some(mp) = self.mount_point.to_str() {
            match run(HDIUTIL, &["detach", "-force", mp]) {
                Ok(o) if o.ok => {}
                Ok(o) => log::warn!("hdiutil detach {}: {}", mp, o.text.trim()),
                Err(e) => log::warn!("hdiutil detach {}: {}", mp, e),
            }
        }
    }
}

/// Mount read-only at `<dir>/mnt`, hidden from Finder, no auto-open.
pub fn attach_readonly(dmg: &Path, dir: &Path) -> ResultType<Mounted> {
    let mp = dir.join("mnt");
    std::fs::create_dir_all(&mp)?;
    let out = run(
        HDIUTIL,
        &[
            "attach",
            "-readonly",
            "-nobrowse",
            "-noautoopen",
            "-mountpoint",
            path_str(&mp)?,
            path_str(dmg)?,
        ],
    )?;
    if !out.ok {
        bail!("hdiutil attach failed: {}", out.text.trim());
    }
    Ok(Mounted { mount_point: mp })
}

fn plist_value(app: &Path, key: &str) -> ResultType<String> {
    let plist = app.join("Contents/Info.plist");
    let out = run_ok(
        PLISTBUDDY,
        &["-c", &format!("Print :{}", key), path_str(&plist)?],
        "PlistBuddy",
    )?;
    Ok(out.text.trim().to_owned())
}

/// App-level check, run on the mounted app and again on any staged copy: strict deep signature
/// against our requirement, Gatekeeper notarized, ticket stapled, and plist identity matches the
/// manifest entry.
pub fn verify_app(app: &Path, rel: &AppRelease) -> ResultType<()> {
    let meta = std::fs::symlink_metadata(app)?;
    if !meta.file_type().is_dir() {
        bail!("{} is not a real directory", app.display());
    }
    let a = path_str(app)?;
    let bundle_id = manifest::own_bundle_id();
    let req = format!("-R={}", requirement_app(bundle_id));
    run_ok(
        CODESIGN,
        &["--verify", "--deep", "--strict", &req, a],
        "codesign (app)",
    )?;
    let out = run_ok(SPCTL, &["-a", "-t", "exec", "-vv", a], "spctl (app)")?;
    if !out.text.contains(NOTARIZED) {
        bail!("app is not notarized: {}", out.text.trim());
    }
    // stapler isn't on staff Macs; a stapled ticket is stored here.
    if !app.join("Contents/CodeResources").exists() {
        bail!("notarization ticket is not stapled");
    }
    let id = plist_value(app, "CFBundleIdentifier")?;
    if id != bundle_id {
        bail!("bundle id {} does not match {}", id, bundle_id);
    }
    let bv = plist_value(app, "CFBundleVersion")?;
    if bv != rel.bundle_version {
        bail!(
            "CFBundleVersion {} does not match manifest {}",
            bv,
            rel.bundle_version
        );
    }
    let tail = bv.rsplit('.').next().unwrap_or("");
    if tail != rel.build.to_string() {
        bail!("CFBundleVersion {} does not end in build {}", bv, rel.build);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirement_strings() {
        let t = requirement_team();
        assert!(t.starts_with("anchor apple generic and certificate 1["));
        assert!(t.contains("subject.OU] = \"UBH6CJDNMJ\""));
        assert!(!t.contains("identifier"));
        let a = requirement_app("org.onehopechurch.hopedesk.manager");
        assert!(a.contains("identifier \"org.onehopechurch.hopedesk.manager\""));
        assert!(a.contains("field.1.2.840.113635.100.6.2.6"));
        assert!(a.contains("field.1.2.840.113635.100.6.1.13"));
    }
}
